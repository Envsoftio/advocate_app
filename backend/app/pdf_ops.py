from __future__ import annotations
import io
import os
from pathlib import Path
import tempfile
import zipfile

import pymupdf


class ProcessingError(Exception):
    pass


def normalize_to_pdf(source: Path, destination: Path):
    destination.parent.mkdir(parents=True, exist_ok=True)
    try:
        if source.suffix.lower() == '.pdf':
            with pymupdf.open(source) as doc:
                if doc.needs_pass:
                    raise ProcessingError('Password-protected PDFs are not supported in this POC; decrypt a lawful working copy first')
                if doc.page_count == 0:
                    raise ProcessingError('PDF has no pages')
                doc.save(destination, garbage=3, deflate=True)
        else:
            with pymupdf.open(source) as image_doc:
                content = image_doc.convert_to_pdf()
            with pymupdf.open(stream=content, filetype='pdf') as pdf:
                if not pdf.page_count:
                    raise ProcessingError('Image does not contain a page')
                pdf.save(destination)
        with pymupdf.open(destination) as doc:
            return doc.page_count
    except ProcessingError:
        raise
    except Exception as exc:
        raise ProcessingError(f'Unable to read or convert input: {exc}') from exc


def replace_pdf_atomically(path: Path, doc: pymupdf.Document):
    fd, candidate = tempfile.mkstemp(prefix='.pdf-edit-', suffix='.pdf', dir=path.parent)
    os.close(fd)
    try:
        doc.save(candidate, garbage=4, deflate=True)
        os.replace(candidate, path)
    finally:
        Path(candidate).unlink(missing_ok=True)


def reorder_pages(path: Path, order: list[int]):
    with pymupdf.open(path) as src:
        if len(order) != src.page_count or sorted(order) != list(range(src.page_count)):
            raise ProcessingError('Page order must be a permutation of every page index')
        with pymupdf.open() as output:
            for index in order:
                output.insert_pdf(src, from_page=index, to_page=index, annots=True, links=True)
            replace_pdf_atomically(path, output)
    return len(order)


def rotate_page(path: Path, page_index: int, degrees: int):
    if degrees not in (90, 180, 270):
        raise ProcessingError('Rotation must be 90, 180 or 270 degrees clockwise')
    with pymupdf.open(path) as doc:
        if not 0 <= page_index < doc.page_count:
            raise ProcessingError('Invalid page number')
        page = doc[page_index]
        page.set_rotation((page.rotation + degrees) % 360)
        replace_pdf_atomically(path, doc)
    return True


def delete_page(path: Path, page_index: int):
    with pymupdf.open(path) as doc:
        if not 0 <= page_index < doc.page_count:
            raise ProcessingError('Invalid page number')
        if doc.page_count == 1:
            raise ProcessingError('Cannot delete the only remaining page')
        doc.delete_page(page_index)
        count = doc.page_count
        replace_pdf_atomically(path, doc)
        return count


def _checked_pages(indices: list[int], count: int) -> list[int]:
    if not indices or len(indices) > 2000 or any(not 0 <= index < count for index in indices):
        raise ProcessingError('Select existing pages using zero-based page indexes')
    return indices


def insert_pages(path: Path, source_path: Path, source_pages: list[int], before_index: int) -> int:
    """Insert selected source pages before a target index; count means append."""
    with pymupdf.open(path) as target, pymupdf.open(source_path) as source:
        _checked_pages(source_pages, source.page_count)
        if not 0 <= before_index <= target.page_count:
            raise ProcessingError('Invalid insertion position')
        with pymupdf.open() as output:
            for index in range(before_index):
                output.insert_pdf(target, from_page=index, to_page=index, annots=True, links=True)
            for index in source_pages:
                output.insert_pdf(source, from_page=index, to_page=index, annots=True, links=True)
            for index in range(before_index, target.page_count):
                output.insert_pdf(target, from_page=index, to_page=index, annots=True, links=True)
            count = output.page_count
            replace_pdf_atomically(path, output)
            return count


def replace_pages(path: Path, source_path: Path, target_pages: list[int], source_pages: list[int]) -> int:
    """Replace target slots one-for-one, preserving the remaining page order."""
    with pymupdf.open(path) as target, pymupdf.open(source_path) as source:
        _checked_pages(target_pages, target.page_count)
        _checked_pages(source_pages, source.page_count)
        if len(target_pages) != len(source_pages) or len(set(target_pages)) != len(target_pages):
            raise ProcessingError('Replacement needs one source page for each distinct target page')
        replacements = dict(zip(target_pages, source_pages))
        with pymupdf.open() as output:
            for index in range(target.page_count):
                selected = source if index in replacements else target
                page_index = replacements.get(index, index)
                output.insert_pdf(selected, from_page=page_index, to_page=page_index,
                                  annots=True, links=True)
            replace_pdf_atomically(path, output)
            return output.page_count


def crop_pages(path: Path, pages: list[int], left: float, top: float, right: float, bottom: float) -> int:
    """Set crop boxes using normalized fractions of each page's current crop box."""
    if not (0 <= left < right <= 1 and 0 <= top < bottom <= 1):
        raise ProcessingError('Crop edges must be ordered fractions between 0 and 1')
    with pymupdf.open(path) as doc:
        _checked_pages(pages, doc.page_count)
        for index in set(pages):
            page = doc[index]
            box = page.cropbox
            rect = pymupdf.Rect(box.x0 + box.width * left, box.y0 + box.height * top,
                                box.x0 + box.width * right, box.y0 + box.height * bottom)
            page.set_cropbox(rect)
        replace_pdf_atomically(path, doc)
        return doc.page_count


def add_text_overlay(path: Path, page_index: int, x: float, y: float,
                     text: str, font_size: float) -> None:
    """Place visible text on a working-copy page; source content is not replaced."""
    if not text.strip() or any(ord(char) < 32 and char != '\n' or ord(char) > 126 for char in text):
        raise ProcessingError('Text overlay supports printable ASCII and line breaks only')
    if not (0 <= x < 1 and 0 <= y < 1 and 6 <= font_size <= 36):
        raise ProcessingError('Invalid text placement or font size')
    with pymupdf.open(path) as doc:
        if not 0 <= page_index < doc.page_count:
            raise ProcessingError('Invalid page number')
        page = doc[page_index]
        if page.rotation:
            raise ProcessingError('Text placement on rotated pages is not supported yet')
        rect = page.rect
        left = rect.x0 + rect.width * x
        top = rect.y0 + rect.height * y
        box = pymupdf.Rect(left, top, rect.x1 - 12, rect.y1 - 12)
        if box.width < font_size * 2 or box.height < font_size * 1.5:
            raise ProcessingError('Choose a point farther from the right or bottom edge')
        remaining = page.insert_textbox(box, text, fontsize=font_size, fontname='helv',
                                        color=(0, 0, 0), overlay=True)
        if remaining < 0:
            raise ProcessingError('Text does not fit from this point; move it or reduce the font size')
        replace_pdf_atomically(path, doc)


def editable_text_runs(path: Path, page_index: int) -> list[dict]:
    """List horizontal text spans with PDF and displayed coordinates."""
    with pymupdf.open(path) as doc:
        if not 0 <= page_index < doc.page_count:
            raise ProcessingError('Invalid page number')
        page = doc[page_index]
        unrotated_rect = page.rect * page.derotation_matrix
        runs = []
        for block in page.get_text('dict')['blocks']:
            if block.get('type') != 0:
                continue
            for line in block.get('lines', []):
                if tuple(round(value, 3) for value in line.get('dir', ())) != (1.0, 0.0):
                    continue
                for span in line.get('spans', []):
                    if not span['text'].strip():
                        continue
                    # PyMuPDF OCR stores recognized words in an invisible
                    # GlyphLessFont layer. Replacing that layer would leave the
                    # scanned pixels unchanged and create a misleading edit.
                    if 'glyphless' in span['font'].lower():
                        continue
                    box = pymupdf.Rect(span['bbox'])
                    if box.is_empty or not unrotated_rect.contains(box):
                        continue
                    displayed = box * page.rotation_matrix
                    runs.append({'index': len(runs), 'text': span['text'],
                                 'bbox': [box.x0, box.y0, box.x1, box.y1],
                                 'relative_bbox': [(displayed.x0 - page.rect.x0) / page.rect.width,
                                                   (displayed.y0 - page.rect.y0) / page.rect.height,
                                                   (displayed.x1 - page.rect.x0) / page.rect.width,
                                                   (displayed.y1 - page.rect.y0) / page.rect.height],
                                 'origin': list(span['origin']), 'size': span['size'],
                                 'font': span['font'], 'color': span['color']})
        return runs


def replace_text_run(path: Path, page_index: int, span_index: int,
                     expected_text: str, replacement: str) -> dict:
    """Replace one selectable run, fitting new lines into adjacent empty space."""
    if (not replacement.strip() or
            any((ord(char) < 32 and char != '\n') or ord(char) > 126 for char in replacement)):
        raise ProcessingError('Replacement supports printable ASCII and line breaks only')
    runs = editable_text_runs(path, page_index)
    if not 0 <= span_index < len(runs) or runs[span_index]['text'] != expected_text:
        raise ProcessingError('Selected text changed; reload the page and select it again')
    selected = runs[span_index]
    box = pymupdf.Rect(selected['bbox'])
    # Redaction removes every glyph touching the rectangle. Refuse ambiguous runs.
    if any(index != span_index and box.intersects(pymupdf.Rect(run['bbox']))
           for index, run in enumerate(runs)):
        raise ProcessingError('Selected text overlaps another run; cannot replace it safely')
    font = selected['font'].lower()
    fontname = ('hebi' if 'bold' in font and ('italic' in font or 'oblique' in font)
                else 'hebo' if 'bold' in font
                else 'heit' if 'italic' in font or 'oblique' in font else 'helv')
    with pymupdf.open(path) as doc:
        page = doc[page_index]
        unrotated_rect = page.rect * page.derotation_matrix
        if any(box.intersects(pymupdf.Rect(link['from'])) for link in page.get_links()):
            raise ProcessingError('Selected text contains a link; edit it outside this tool')
        obstacles = [pymupdf.Rect(run['bbox']) for index, run in enumerate(runs)
                     if index != span_index]
        obstacles.extend(pymupdf.Rect(block['bbox']) for block in page.get_text('dict')['blocks']
                         if block.get('type') == 1)
        right = unrotated_rect.x1 - 12
        for other in obstacles:
            if other.y0 < box.y1 + 1 and other.y1 > box.y0 - 1 and other.x0 >= box.x1 - 1:
                right = min(right, other.x0 - 2)
        if right <= selected['origin'][0]:
            raise ProcessingError('No clear space to the right of the selected text')
        bottom = unrotated_rect.y1 - 12
        for other in obstacles:
            if other.y0 >= box.y1 - 1 and other.x0 < right and other.x1 > box.x0:
                bottom = min(bottom, other.y0 - 2)
        available_width = right - selected['origin'][0]
        source_size = selected['size']
        minimum_size = min(source_size, max(6, source_size * .75))
        layout = None
        for step in range(11):
            size = source_size - (source_size - minimum_size) * step / 10
            lines = _wrap_replacement(replacement, available_width, fontname, size)
            if lines is None:
                continue
            last_baseline = selected['origin'][1] + (len(lines) - 1) * size * 1.2
            if last_baseline + size * .3 <= bottom:
                layout = (size, lines)
                break
        if layout is None:
            raise ProcessingError('Text cannot fit in clear space on this page. Shorten it or use Add text elsewhere.')
        size, lines = layout
        page.add_redact_annot(box, fill=False, cross_out=False)
        page.apply_redactions(images=pymupdf.PDF_REDACT_IMAGE_NONE,
                              graphics=pymupdf.PDF_REDACT_LINE_ART_NONE,
                              text=pymupdf.PDF_REDACT_TEXT_REMOVE)
        color = selected['color']
        rgb = ((color >> 16 & 255) / 255, (color >> 8 & 255) / 255, (color & 255) / 255)
        for line_index, line in enumerate(lines):
            if line:
                page.insert_text((selected['origin'][0],
                                  selected['origin'][1] + line_index * size * 1.2), line,
                                 fontname=fontname, fontsize=size, color=rgb, overlay=True)
        replace_pdf_atomically(path, doc)
        return {'font_size': round(size, 2), 'line_count': len(lines),
                'font_size_reduced': size < source_size - .01}


def _wrap_replacement(value: str, max_width: float, fontname: str,
                      size: float) -> list[str] | None:
    """Wrap at spaces; reject a word that cannot fit without breaking letters."""
    lines = []
    for paragraph in value.split('\n'):
        if not paragraph.strip():
            lines.append('')
            continue
        current = ''
        for word in paragraph.split():
            candidate = f'{current} {word}' if current else word
            if pymupdf.get_text_length(candidate, fontname=fontname, fontsize=size) <= max_width:
                current = candidate
            elif current and pymupdf.get_text_length(word, fontname=fontname, fontsize=size) <= max_width:
                lines.append(current)
                current = word
            else:
                return None
        lines.append(current)
    return lines


def extract_pages(path: Path, pages: list[int]) -> bytes:
    """Return a separate PDF in the requested order without changing the working copy."""
    with pymupdf.open(path) as source, pymupdf.open() as output:
        _checked_pages(pages, source.page_count)
        for index in pages:
            output.insert_pdf(source, from_page=index, to_page=index, annots=True, links=True)
        return output.tobytes(garbage=4, deflate=True)


def split_pages(path: Path, after_pages: list[int]) -> bytes:
    """Return sequential parts split after zero-based page indexes in a ZIP."""
    with pymupdf.open(path) as source:
        count = source.page_count
        if (not after_pages or len(after_pages) > 1999 or
                sorted(set(after_pages)) != after_pages or
                any(not 0 <= index < count - 1 for index in after_pages)):
            raise ProcessingError('Split points must be distinct ascending pages before the last page')
        output = io.BytesIO()
        starts = [0] + [index + 1 for index in after_pages]
        ends = after_pages + [count - 1]
        with zipfile.ZipFile(output, 'w', compression=zipfile.ZIP_DEFLATED) as archive:
            for part, (start, end) in enumerate(zip(starts, ends), 1):
                with pymupdf.open() as result:
                    result.insert_pdf(source, from_page=start, to_page=end,
                                      annots=True, links=True)
                    archive.writestr(f'part-{part:03}.pdf', result.tobytes(garbage=4, deflate=True))
        return output.getvalue()


def ocr_pdf(path: Path, *, language: str='eng', progress=None, cancelled=None) -> tuple[int, int]:
    """OCR image-only pages; render 240 DPI, preserving a distinct untouched original.

    WARNING: rasterizing an image-only page may alter appearance/metadata. Signature
    validation is outside this POC. POC does not OCR handwriting reliably.
    """
    with pymupdf.open(path) as source, pymupdf.open() as result:
        changed = 0
        total = source.page_count
        for index in range(total):
            if cancelled and cancelled():
                raise ProcessingError('Operation cancelled')
            page = source[index]
            if page.get_text('text').strip():
                result.insert_pdf(source, from_page=index, to_page=index, annots=True, links=True)
            else:
                try:
                    pix = page.get_pixmap(matrix=pymupdf.Matrix(240 / 72, 240 / 72), colorspace=pymupdf.csRGB, alpha=False)
                    ocr_bytes = pix.pdfocr_tobytes(language=language)
                    with pymupdf.open(stream=ocr_bytes, filetype='pdf') as new_page:
                        result.insert_pdf(new_page)
                    changed += 1
                except Exception as exc:
                    raise ProcessingError('OCR failed. Check that Tesseract and English traineddata are installed. ' + str(exc)) from exc
            if progress:
                progress(int((index + 1) / total * 95), f'OCR page {index + 1} of {total}')
        if changed:
            replace_pdf_atomically(path, result)
        return total, changed


def make_cover_pdf(destination: Path, matter: dict) -> int:
    destination.parent.mkdir(parents=True, exist_ok=True)
    with pymupdf.open() as doc:
        page = doc.new_page(width=595, height=842)
        page.draw_rect(pymupdf.Rect(40, 40, 555, 802), color=(0.12,0.2,0.33), width=1.2)
        page.insert_text((64, 95), 'CASE FILE - DRAFT COVER SHEET', fontsize=17, fontname='hebo', color=(0.12,0.2,0.33))
        page.insert_text((64, 132), 'For preparation only - not an approved court form', fontsize=10, color=(0.5,0.15,0.15))
        cursor = 200
        items = [
            ('Matter', matter['name']), ('Court profile', matter['court_profile']),
            ('Case type', matter['case_type']), ('Petitioner', matter['petitioner']),
            ('Respondent', matter['respondent']), ('Advocate', matter['advocate']),
            ('Case number', matter['case_number']),
        ]
        for heading, value in items:
            page.insert_text((65, cursor), heading.upper(), fontsize=9, color=(0.4,0.46,0.52))
            page.insert_textbox(pymupdf.Rect(65, cursor + 8, 530, cursor + 43), str(value or '--'), fontsize=12, fontname='helv')
            cursor += 64
        doc.save(destination)
    return 1
