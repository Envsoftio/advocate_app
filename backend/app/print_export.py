"""Create a temporary, user-selected print copy of a completed PDF export."""
from __future__ import annotations

import re
from pathlib import Path

import pymupdf

from .file_io import open_pdf


def parse_pages(spec: str, count: int) -> list[int]:
    if not isinstance(spec, str):
        raise ValueError('Enter all or page numbers such as 1,3-5')
    if spec.strip().lower() == 'all':
        return list(range(count))
    if not spec or len(spec) > 2048 or not re.fullmatch(r'[\d,\s-]+', spec):
        raise ValueError('Enter all or page numbers such as 1,3-5')
    selected: list[int] = []
    for part in spec.split(','):
        part = part.strip()
        match = re.fullmatch(r'(\d+)(?:\s*-\s*(\d+))?', part)
        if not match:
            raise ValueError('Enter all or page numbers such as 1,3-5')
        start = int(match.group(1))
        end = int(match.group(2) or start)
        if start < 1 or end < start or end > count:
            raise ValueError(f'Page range must stay between 1 and {count}')
        selected.extend(range(start - 1, end))
    if not selected or len(selected) > 500 or len(set(selected)) != len(selected):
        raise ValueError('Select 1–500 distinct pages for one print job')
    return selected


def print_copy(path: Path | bytes, page_range: str, orientation: str, scale: str) -> bytes:
    if orientation not in {'source', 'portrait', 'landscape'}:
        raise ValueError('Choose source, portrait or landscape paper')
    if scale not in {'fit', 'actual'}:
        raise ValueError('Choose fit or actual size')
    source_file = (pymupdf.open(stream=path, filetype='pdf') if isinstance(path, bytes)
                   else open_pdf(path))
    with source_file as source, pymupdf.open() as output:
        pages = parse_pages(page_range, len(source))
        # Reflowing onto A4 with show_pdf_page omits live annotation/widget
        # appearances. Bake them into this temporary copy for both layouts.
        source.bake(annots=True, widgets=True)
        if orientation == 'source':
            for index in pages:
                output.insert_pdf(source, from_page=index, to_page=index,
                                  links=False, annots=False)
        else:
            paper = pymupdf.Rect(0, 0, 595, 842) if orientation == 'portrait' else pymupdf.Rect(0, 0, 842, 595)
            for index in pages:
                page = output.new_page(width=paper.width, height=paper.height)
                source_rect = source[index].rect
                if scale == 'actual':
                    width, height = source_rect.width, source_rect.height
                    target = pymupdf.Rect((paper.width - width) / 2, (paper.height - height) / 2,
                                          (paper.width + width) / 2, (paper.height + height) / 2)
                else:
                    target = pymupdf.Rect(18, 18, paper.width - 18, paper.height - 18)
                page.show_pdf_page(target, source, index, keep_proportion=True)
        return output.tobytes(garbage=4, deflate=True)
