from __future__ import annotations
import os
from pathlib import Path
import tempfile

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
    fd, candidate = tempfile.mkstemp(suffix='.pdf', dir=path.parent)
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
