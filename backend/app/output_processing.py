"""Explicit processing of unpublished generated files, never source revisions."""
from __future__ import annotations
import os
from pathlib import Path
from .file_io import open_pdf, save_pdf, size as file_size
from .pdf_ops import ProcessingError, compress_pdf


def content_contract(path: Path) -> dict:
    # Deliberately independent of xref/object numbering, which optimization changes.
    with open_pdf(path) as pdf:
        return {'toc': pdf.get_toc(), 'pages': [
            {'text': page.get_text(), 'rect': tuple(page.rect), 'rotation': page.rotation,
             'label': page.get_label(),
             'links': [{k: (tuple(v) if k in ('from', 'to') else v)
                        for k, v in link.items() if k not in ('xref', 'id')}
                       for link in page.get_links()]}
            for page in pdf]}


def process_generated_pdf(path: Path, settings: dict, cancelled) -> dict:
    """Caller owns an unpublished staging path. Failure aborts that entire build."""
    if cancelled():
        raise ProcessingError('Operation cancelled')
    before_bytes = file_size(path)
    before = content_contract(path)
    quality = settings.get('processing_quality', 'original')
    compression = None
    if quality != 'original':
        compression = compress_pdf(path, quality, cancelled=cancelled)
    if settings.get('cleanup_metadata', False):
        candidate = path.with_suffix('.metadata.pdf')
        try:
            with open_pdf(path) as pdf:
                pdf.set_metadata({})
                pdf.del_xml_metadata()
                save_pdf(pdf, candidate, garbage=4, deflate=True)
            os.replace(candidate, path)
        finally:
            candidate.unlink(missing_ok=True)
    if cancelled():
        raise ProcessingError('Operation cancelled')
    if content_contract(path) != before:
        raise ProcessingError('Output processing changed text, page geometry, labels or navigation; no build published')
    with open_pdf(path) as pdf:
        metadata_cleared = not any(v for k, v in pdf.metadata.items() if k not in ('format', 'encryption')) and not pdf.get_xml_metadata()
        if settings.get('cleanup_metadata') and not metadata_cleared:
            raise ProcessingError('Output metadata cleanup verification failed')
    return dict(filename=path.name, quality=quality, before_bytes=before_bytes,
                after_bytes=file_size(path), compression=compression,
                metadata_cleared=metadata_cleared,
                verified=['extracted_text', 'page_geometry', 'page_labels', 'bookmarks', 'links'])
