"""Render revision-anchored editable PDF marks into a detached output PDF."""
from __future__ import annotations

import json
import math

import pymupdf

from .pdf_ops import ProcessingError

PDF_MARK_KINDS = frozenset(('draw', 'box', 'ellipse', 'line', 'arrow',
                            'check', 'cross', 'date', 'text', 'redaction'))
MARK_COLORS = {'yellow': (0.68, 0.48, 0), 'green': (0.15, 0.48, 0.22),
               'blue': (0.09, 0.41, 0.67), 'pink': (0.7, 0.29, 0.47)}


def shown_rect(page: pymupdf.Page, item: dict) -> pymupdf.Rect:
    rect = page.rect
    return pymupdf.Rect(rect.x0 + rect.width * item['rect_x'],
                        rect.y0 + rect.height * item['rect_y'],
                        rect.x0 + rect.width * (item['rect_x'] + item['rect_width']),
                        rect.y0 + rect.height * (item['rect_y'] + item['rect_height']))


def display_rect(page: pymupdf.Page, item: dict) -> pymupdf.Rect:
    return shown_rect(page, item) * page.derotation_matrix


def shown_point(page: pymupdf.Page, item: dict, x: float, y: float) -> pymupdf.Point:
    box = shown_rect(page, item)
    return pymupdf.Point(box.x0 + box.width * x,
                         box.y0 + box.height * y) * page.derotation_matrix


def apply_redaction_marks(pdf: pymupdf.Document, items: list[tuple[int, dict]]) -> int:
    count = 0
    pages: set[int] = set()
    for page_index, item in items:
        if item['kind'] != 'redaction':
            continue
        page = pdf[page_index]
        page.add_redact_annot(display_rect(page, item), fill=(0, 0, 0), cross_out=False)
        pages.add(page_index)
        count += 1
    for page_index in pages:
        pdf[page_index].apply_redactions(images=pymupdf.PDF_REDACT_IMAGE_PIXELS,
            graphics=pymupdf.PDF_REDACT_LINE_ART_REMOVE_IF_COVERED,
            text=pymupdf.PDF_REDACT_TEXT_REMOVE)
    return count


def draw_pdf_mark(page: pymupdf.Page, item: dict) -> None:
    kind = item['kind']
    box = display_rect(page, item)
    color = MARK_COLORS[item['color']]
    stroke = max(1, min(3, min(box.width, box.height) * .04))
    if kind == 'box':
        page.draw_rect(box, color=color, width=stroke, overlay=True)
    elif kind == 'ellipse':
        page.draw_oval(box, color=color, width=stroke, overlay=True)
    elif kind in ('line', 'arrow'):
        start = shown_point(page, item, 0, 1)
        end = shown_point(page, item, 1, 0)
        page.draw_line(start, end, color=color, width=stroke, overlay=True)
        if kind == 'arrow':
            shown = shown_rect(page, item)
            length = max(5, min(16, min(shown.width, shown.height) * .35))
            angle = math.atan2(-shown.height, shown.width)
            for direction in (-1, 1):
                theta = angle + math.pi + direction * .55
                wing = pymupdf.Point(shown.tr.x + length * math.cos(theta),
                    shown.tr.y + length * math.sin(theta)) * page.derotation_matrix
                page.draw_line(end, wing, color=color, width=stroke, overlay=True)
    elif kind == 'draw':
        points = json.loads(item['body'])['points']
        for before, after in zip(points, points[1:]):
            p = shown_point(page, item, before[0], before[1])
            q = shown_point(page, item, after[0], after[1])
            page.draw_line(p, q, color=color, width=stroke, overlay=True)
    elif kind in ('check', 'cross', 'date', 'text'):
        label = {'check': 'v', 'cross': 'X'}.get(kind, item['body'])
        size = min(36, max(6, min(box.height / max(1, len(label.splitlines())) * .45,
                               box.width / max(1, max(map(len, label.splitlines()))) * 1.6)))
        while size >= 6:
            if page.insert_textbox(box, label, fontsize=size, fontname='helv',
                    color=(0, 0, 0) if kind == 'text' else color,
                    rotate=page.rotation, overlay=True) >= 0:
                break
            size -= 1
        else:
            raise ProcessingError(f'{kind.title()} mark does not fit its selected area')
    else:
        raise ProcessingError('Unsupported PDF mark kind')


def resolved_pdf_marks(annotations: list[dict], starts: dict[str, int]) -> list[tuple[int, dict]]:
    items = []
    for item in annotations:
        start = starts.get(item['document_id'])
        if start is None or item['output_scope'] != 'pdf' or item['kind'] not in PDF_MARK_KINDS:
            continue
        if not item['resolved']:
            continue
        items.append((start + item['page_index'], item))
    return items


def apply_pdf_marks(pdf: pymupdf.Document, annotations: list[dict],
                    starts: dict[str, int]) -> dict:
    items = resolved_pdf_marks(annotations, starts)
    if any(not 0 <= index < pdf.page_count for index, _ in items):
        raise ProcessingError('Editable mark target differs from the deterministic page map')
    visible = 0
    for page_index, item in items:
        if item['kind'] == 'redaction':
            continue
        draw_pdf_mark(pdf[page_index], item)
        visible += 1
    # Redaction is last so an overlapping text, shape or image mark cannot
    # reintroduce content inside an area the user marked for removal.
    redactions = apply_redaction_marks(pdf, items)
    return {'editable_marks_inserted': visible, 'redactions_applied': redactions}
