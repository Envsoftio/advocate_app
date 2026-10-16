from __future__ import annotations
import io
import math
import os
import re
from pathlib import Path
import tempfile
import zipfile

import pymupdf

from .file_io import copy_file, open_pdf, protected, save_pdf, size as file_size


class ProcessingError(Exception):
    pass


def normalize_to_pdf(source: Path, destination: Path):
    destination.parent.mkdir(parents=True, exist_ok=True)
    try:
        if source.suffix.lower() == '.pdf':
            with open_pdf(source) as doc:
                if doc.needs_pass:
                    raise ProcessingError('Password-protected PDFs are not supported in this POC; decrypt a lawful working copy first')
                if doc.page_count == 0:
                    raise ProcessingError('PDF has no pages')
                # Keep the initial working PDF byte-identical to the imported PDF.
                # Re-saving here could disturb certificate-signature byte ranges
                # before the user has seen and acknowledged the warning.
            copy_file(source, destination)
        else:
            with open_pdf(source) as image_doc:
                content = image_doc.convert_to_pdf()
            with pymupdf.open(stream=content, filetype='pdf') as pdf:
                if not pdf.page_count:
                    raise ProcessingError('Image does not contain a page')
                save_pdf(pdf, destination)
        with open_pdf(destination) as doc:
            return doc.page_count
    except ProcessingError:
        raise
    except Exception as exc:
        raise ProcessingError(f'Unable to read or convert input: {exc}') from exc


def replace_pdf_atomically(path: Path, doc: pymupdf.Document):
    if protected(path):
        candidate = path.with_name('.pdf-edit-' + os.urandom(12).hex() + '.pdf')
        try:
            save_pdf(doc, candidate, garbage=4, deflate=True)
            validate_pdf(candidate, doc.page_count)
            os.replace(candidate, path)
        finally:
            candidate.unlink(missing_ok=True)
        return
    fd, candidate = tempfile.mkstemp(prefix='.pdf-edit-', suffix='.pdf', dir=path.parent)
    os.close(fd)
    try:
        doc.save(candidate, garbage=4, deflate=True)
        validate_pdf(Path(candidate), doc.page_count)
        with open(candidate, 'rb') as stream:
            os.fsync(stream.fileno())
        os.replace(candidate, path)
    finally:
        Path(candidate).unlink(missing_ok=True)


def validate_pdf(path: Path, expected_pages: int) -> None:
    """Reopen a completed candidate before publishing it as a revision."""
    with open_pdf(path) as check:
        if check.needs_pass or check.page_count != expected_pages or expected_pages < 1:
            raise ProcessingError('Written PDF failed page-count or encryption validation')
        for page in check:
            if page.rect.is_empty or not all(math.isfinite(v) for v in page.rect):
                raise ProcessingError('Written PDF contains invalid page dimensions')
            # Parse content and annotations without allocating a full-page bitmap.
            page.get_displaylist()
            for link in page.get_links():
                if link['kind'] == pymupdf.LINK_GOTO and not 0 <= link.get('page', -1) < expected_pages:
                    raise ProcessingError('Written PDF contains an invalid internal link')


def assemble_pages(output: pymupdf.Document, selections: list[tuple]) -> None:
    """Copy pages, then remap navigation after every destination exists.

    For duplicated targets, links resolve to the first selected occurrence (or
    the same occurrence for a self-link). Links to excluded pages are omitted.
    """
    destinations = {}
    sources = {}
    for position, (source, index) in enumerate(selections):
        sources[id(source)] = source
        destinations.setdefault((id(source), index), position)
        output.insert_pdf(source, from_page=index, to_page=index, annots=True, links=False)
    for position, (source, index) in enumerate(selections):
        for original in source[index].get_links():
            link = {key: value for key, value in original.items() if key not in ('xref', 'id')}
            link['from'] = link['from'] * source[index].derotation_matrix
            if link['kind'] == pymupdf.LINK_GOTO:
                target = link.get('page', -1)
                mapped = position if target == index else destinations.get((id(source), target))
                if mapped is None:
                    continue
                if 'to' in link:
                    link['to'] = link['to'] * source[target].derotation_matrix
                link['page'] = mapped
            output[position].insert_link(link)
    toc = []
    for source in sources.values():
        previous_level = 0
        for level, title, page in source.get_toc():
            mapped = destinations.get((id(source), page - 1))
            if mapped is not None:
                level = min(level, previous_level + 1)
                toc.append([level, title, mapped + 1])
                previous_level = level
    if toc:
        output.set_toc(toc)
    if sources:
        output.set_metadata(next(iter(sources.values())).metadata)


def reorder_pages(path: Path, order: list[int]):
    with open_pdf(path) as src:
        if len(order) != src.page_count or sorted(order) != list(range(src.page_count)):
            raise ProcessingError('Page order must be a permutation of every page index')
        with pymupdf.open() as output:
            assemble_pages(output, [(src, index) for index in order])
            replace_pdf_atomically(path, output)
    return len(order)


def rotate_page(path: Path, page_index: int, degrees: int):
    if degrees not in (90, 180, 270):
        raise ProcessingError('Rotation must be 90, 180 or 270 degrees clockwise')
    with open_pdf(path) as doc:
        if not 0 <= page_index < doc.page_count:
            raise ProcessingError('Invalid page number')
        page = doc[page_index]
        page.set_rotation((page.rotation + degrees) % 360)
        replace_pdf_atomically(path, doc)
    return True


def delete_page(path: Path, page_index: int):
    with open_pdf(path) as doc:
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
    with open_pdf(path) as target, open_pdf(source_path) as source:
        _checked_pages(source_pages, source.page_count)
        if not 0 <= before_index <= target.page_count:
            raise ProcessingError('Invalid insertion position')
        with pymupdf.open() as output:
            assemble_pages(output, [(target, index) for index in range(before_index)] +
                           [(source, index) for index in source_pages] +
                           [(target, index) for index in range(before_index, target.page_count)])
            count = output.page_count
            replace_pdf_atomically(path, output)
            return count


def replace_pages(path: Path, source_path: Path, target_pages: list[int], source_pages: list[int]) -> int:
    """Replace target slots one-for-one, preserving the remaining page order."""
    with open_pdf(path) as target, open_pdf(source_path) as source:
        _checked_pages(target_pages, target.page_count)
        _checked_pages(source_pages, source.page_count)
        if len(target_pages) != len(source_pages) or len(set(target_pages)) != len(target_pages):
            raise ProcessingError('Replacement needs one source page for each distinct target page')
        replacements = dict(zip(target_pages, source_pages))
        with pymupdf.open() as output:
            assemble_pages(output, [(source, replacements[index]) if index in replacements
                           else (target, index) for index in range(target.page_count)])
            replace_pdf_atomically(path, output)
            return output.page_count


def replace_document(path: Path, source_path: Path) -> int:
    """Replace all working pages with another document's current working pages."""
    with open_pdf(source_path) as source:
        if source.needs_pass or source.page_count < 1:
            raise ProcessingError('Replacement source must be an open PDF with pages')
        with pymupdf.open() as output:
            assemble_pages(output, [(source, index) for index in range(source.page_count)])
            replace_pdf_atomically(path, output)
            return output.page_count


def crop_pages(path: Path, pages: list[int], left: float, top: float, right: float, bottom: float) -> int:
    """Set crop boxes using normalized fractions of each page's current crop box."""
    if not (0 <= left < right <= 1 and 0 <= top < bottom <= 1):
        raise ProcessingError('Crop edges must be ordered fractions between 0 and 1')
    with open_pdf(path) as doc:
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
    with open_pdf(path) as doc:
        if not 0 <= page_index < doc.page_count:
            raise ProcessingError('Invalid page number')
        page = doc[page_index]
        rect = page.rect
        left = rect.x0 + rect.width * x
        top = rect.y0 + rect.height * y
        box = pymupdf.Rect(left, top, rect.x1 - 12, rect.y1 - 12)
        if box.width < font_size * 2 or box.height < font_size * 1.5:
            raise ProcessingError('Choose a point farther from the right or bottom edge')
        box = box * page.derotation_matrix
        remaining = page.insert_textbox(box, text, fontsize=font_size, fontname='helv', rotate=page.rotation,
                                        color=(0, 0, 0), overlay=True)
        if remaining < 0:
            raise ProcessingError('Text does not fit from this point; move it or reduce the font size')
        replace_pdf_atomically(path, doc)


def editable_text_runs(path: Path, page_index: int) -> list[dict]:
    """List horizontal text spans with PDF and displayed coordinates."""
    with open_pdf(path) as doc:
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
    with open_pdf(path) as doc:
        page = doc[page_index]
        if any(annot.type[0] == pymupdf.PDF_ANNOT_REDACT for annot in (page.annots() or ())):
            raise ProcessingError('This page has pending redaction marks. Text replacement would apply them; review them outside this tool first.')
        overlaps = [span for block in page.get_text('dict')['blocks'] if block.get('type') == 0
                    for line in block['lines'] for span in line['spans']
                    if span['text'].strip() and box.intersects(pymupdf.Rect(span['bbox']))]
        if len(overlaps) != 1:
            raise ProcessingError('Selected text overlaps other text, including a non-editable layer; cannot replace it safely')
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


def move_text_run(path: Path, page_index: int, span_index: int,
                  expected_text: str, vertical_offset: float) -> None:
    """Move one native horizontal span vertically in displayed page coordinates."""
    if not math.isfinite(vertical_offset) or abs(vertical_offset) < .0001:
        raise ProcessingError('Drag the text up or down before applying the move')
    runs = editable_text_runs(path, page_index)
    if not 0 <= span_index < len(runs) or runs[span_index]['text'] != expected_text:
        raise ProcessingError('Selected text changed; reload the page and select it again')
    selected = runs[span_index]
    if any(ord(char) < 32 or ord(char) > 126 for char in expected_text):
        raise ProcessingError('Moving text currently supports single-line printable ASCII only')
    source = pymupdf.Rect(selected['bbox'])
    with open_pdf(path) as doc:
        page = doc[page_index]
        if any(annot.type[0] == pymupdf.PDF_ANNOT_REDACT for annot in (page.annots() or ())):
            raise ProcessingError('This page has pending redaction marks. Review them before moving text.')
        blocks = page.get_text('dict')['blocks']
        spans = [span for block in blocks if block.get('type') == 0
                 for line in block['lines'] for span in line['spans'] if span['text'].strip()]
        source_matches = [span for span in spans if source.intersects(pymupdf.Rect(span['bbox']))]
        if len(source_matches) != 1:
            raise ProcessingError('Selected text overlaps other text; cannot move it safely')
        display_delta = page.rect.height * vertical_offset
        zero = pymupdf.Point(0, 0) * page.derotation_matrix
        shifted = pymupdf.Point(0, display_delta) * page.derotation_matrix
        dx, dy = shifted.x - zero.x, shifted.y - zero.y
        target = source + (dx, dy, dx, dy)
        page_bounds = page.rect * page.derotation_matrix
        if not page_bounds.contains(target):
            raise ProcessingError('The text would leave the page; drag it closer to the centre')
        links = [pymupdf.Rect(link['from']) for link in page.get_links()]
        if any(source.intersects(link) or target.intersects(link) for link in links):
            raise ProcessingError('Text inside or over a link cannot be moved with this tool')
        obstacles = [pymupdf.Rect(span['bbox']) for span in spans if span is not source_matches[0]]
        obstacles.extend(pymupdf.Rect(block['bbox']) for block in blocks if block.get('type') == 1)
        obstacles.extend(pymupdf.Rect(item['rect']) for item in page.get_drawings())
        if any(target.intersects(obstacle) for obstacle in obstacles):
            raise ProcessingError('The new position overlaps text or page graphics; choose a clear gap')
        font = selected['font'].lower()
        fontname = ('hebi' if 'bold' in font and ('italic' in font or 'oblique' in font)
                    else 'hebo' if 'bold' in font
                    else 'heit' if 'italic' in font or 'oblique' in font else 'helv')
        color = selected['color']
        rgb = ((color >> 16 & 255) / 255, (color >> 8 & 255) / 255, (color & 255) / 255)
        page.add_redact_annot(source, fill=False, cross_out=False)
        page.apply_redactions(images=pymupdf.PDF_REDACT_IMAGE_NONE,
                              graphics=pymupdf.PDF_REDACT_LINE_ART_NONE,
                              text=pymupdf.PDF_REDACT_TEXT_REMOVE)
        origin = selected['origin']
        page.insert_text((origin[0] + dx, origin[1] + dy), expected_text,
                         fontname=fontname, fontsize=selected['size'], color=rgb, overlay=True)
        replace_pdf_atomically(path, doc)


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
    with open_pdf(path) as source, pymupdf.open() as output:
        _checked_pages(pages, source.page_count)
        assemble_pages(output, [(source, index) for index in pages])
        return output.tobytes(garbage=4, deflate=True)


def split_pages(path: Path, after_pages: list[int]) -> bytes:
    """Return sequential parts split after zero-based page indexes in a ZIP."""
    with open_pdf(path) as source:
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
                    assemble_pages(result, [(source, index) for index in range(start, end + 1)])
                    archive.writestr(f'part-{part:03}.pdf', result.tobytes(garbage=4, deflate=True))
        return output.getvalue()


COMPRESSION_PRESETS = {'lossless': None, 'balanced': (240, 85), 'small': (150, 70)}


def compress_pdf(path: Path, preset: str, progress=None, cancelled=None) -> dict:
    """Reduce image resolution explicitly; never publish a larger result.

    Transparent/masked and bitonal images are retained. Text, outlines, page
    geometry and annotations are not rasterized. No target size is promised.
    """
    if preset not in COMPRESSION_PRESETS:
        raise ProcessingError('Unknown compression preset')
    before = file_size(path)
    fd, name = tempfile.mkstemp(prefix='.pdf-edit-', suffix='.pdf', dir=path.parent)
    os.close(fd)
    candidate = Path(name)
    rewritten = 0
    skipped = 0
    try:
        with open_pdf(path) as doc:
            images = {}
            for page in doc:
                if cancelled and cancelled():
                    raise ProcessingError('Operation cancelled')
                for info in page.get_images(full=True):
                    xref, mask, width, height, bpc = info[:5]
                    if xref not in images:
                        images[xref] = {'page': page.number, 'mask': mask, 'width': width,
                                        'height': height, 'bpc': bpc, 'placements': []}
                    images[xref]['placements'].extend(page.get_image_rects(xref))
            if COMPRESSION_PRESETS[preset]:
                dpi, quality = COMPRESSION_PRESETS[preset]
                for number, (xref, info) in enumerate(images.items()):
                    if cancelled and cancelled():
                        raise ProcessingError('Operation cancelled')
                    placements = [rect for rect in info['placements'] if not rect.is_empty]
                    if (info['mask'] or info['bpc'] == 1 or not placements or
                            info['width'] * info['height'] > 40_000_000):
                        skipped += 1
                        continue
                    # A shared image must remain adequate at its largest placement.
                    ratio = min(1.0, max(max(rect.width * dpi / 72 / info['width'],
                                             rect.height * dpi / 72 / info['height'])
                                         for rect in placements))
                    pix = pymupdf.Pixmap(doc, xref)
                    if pix.alpha or pix.colorspace is None:
                        skipped += 1
                        continue
                    if pix.colorspace.n not in (1, 3):
                        pix = pymupdf.Pixmap(pymupdf.csRGB, pix)
                    if ratio < .95:
                        pix = pymupdf.Pixmap(pix, max(1, int(pix.width * ratio)),
                                             max(1, int(pix.height * ratio)), None)
                    data = pix.tobytes('jpeg', jpg_quality=quality)
                    if len(data) < len(doc.xref_stream_raw(xref)):
                        doc[info['page']].replace_image(xref, stream=data)
                        rewritten += 1
                    if progress:
                        progress(int((number + 1) / max(1, len(images)) * 90),
                                 f'Compressing image {number + 1} of {len(images)}')
            save_pdf(doc, candidate, garbage=4, deflate=True)
            validate_pdf(candidate, doc.page_count)
        after = file_size(candidate)
        if cancelled and cancelled():
            raise ProcessingError('Operation cancelled')
        changed = after < before
        if changed:
            with candidate.open('rb') as stream:
                os.fsync(stream.fileno())
            os.replace(candidate, path)
        return {'preset': preset, 'before_bytes': before, 'after_bytes': after if changed else before,
                'changed': changed, 'images_rewritten': rewritten if changed else 0,
                'images_skipped': skipped,
                'message': 'Compressed working copy; review visual quality.' if changed
                           else 'No smaller result; working copy retained.'}
    finally:
        candidate.unlink(missing_ok=True)


def ocr_pdf(path: Path, *, language: str='eng', progress=None, cancelled=None) -> tuple[int, int]:
    """Add an invisible OCR layer while retaining original page content/navigation.

    Mixed pages are recognized too. Existing text regions are removed from the
    generated layer to avoid duplicate searchable text. OCR remains unverified.
    """
    with open_pdf(path) as source:
        changed = 0
        total = source.page_count
        for index in range(total):
            if cancelled and cancelled():
                raise ProcessingError('Operation cancelled')
            page = source[index]
            blocks = page.get_text('dict')['blocks']
            text_blocks = [block for block in blocks if block.get('type') == 0]
            has_ocr = any('glyphless' in span['font'].lower()
                          for block in text_blocks for line in block['lines'] for span in line['spans'])
            needs_ocr = not has_ocr and (not page.get_text().strip() or bool(page.get_images()))
            if needs_ocr:
                rotation = page.rotation
                try:
                    page.set_rotation(0)
                    # Refuse pathological page sizes before allocating hundreds of MB.
                    pixels = math.ceil(page.rect.width * 240 / 72) * math.ceil(page.rect.height * 240 / 72)
                    if pixels > 40_000_000:
                        raise ProcessingError('OCR page exceeds the 40-megapixel processing limit; crop or resize a working copy first')
                    pix = page.get_pixmap(dpi=240, colorspace=pymupdf.csRGB, alpha=False, annots=False)
                    ocr_bytes = pix.pdfocr_tobytes(language=language)
                    if cancelled and cancelled():
                        raise ProcessingError('Operation cancelled')
                    with pymupdf.open(stream=ocr_bytes, filetype='pdf') as layer:
                        layer_page = layer[0]
                        # This is our freshly generated OCR PDF, whose image
                        # invocations are simple named XObjects. Remove drawing
                        # commands rather than substituting a transparent pixel:
                        # even that pixel can change compositing/rounding below.
                        names = [image[7] for image in layer_page.get_images()]
                        for xref in layer_page.get_contents():
                            content = layer.xref_stream(xref)
                            for name in names:
                                content = re.sub(rb'/' + re.escape(name.encode()) + rb'\s+Do\b',
                                                 b'', content)
                            layer.update_stream(xref, content)
                        transform = page.rect.torect(layer_page.rect)
                        for block in text_blocks:
                            for line in block['lines']:
                                for span in line['spans']:
                                    layer_page.add_redact_annot(pymupdf.Rect(span['bbox']) * transform,
                                                               fill=False, cross_out=False)
                        if text_blocks:
                            layer_page.apply_redactions(images=pymupdf.PDF_REDACT_IMAGE_NONE,
                                                        graphics=pymupdf.PDF_REDACT_LINE_ART_NONE)
                        if layer_page.get_text().strip():
                            page.show_pdf_page(page.rect, layer, 0)
                            changed += 1
                except ProcessingError:
                    raise
                except Exception as exc:
                    raise ProcessingError('OCR failed; check the installed language data. No working revision was published. ' + str(exc)) from exc
                finally:
                    page.set_rotation(rotation)
            if progress:
                progress(int((index + 1) / total * 95), f'OCR page {index + 1} of {total}')
        if cancelled and cancelled():
            raise ProcessingError('Operation cancelled')
        if changed:
            replace_pdf_atomically(path, source)
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
        save_pdf(doc, destination)
    return 1
