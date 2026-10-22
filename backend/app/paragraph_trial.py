"""Experimental PDFium paragraph object editing inside the working-copy revision path.

The paragraph grouping, object removal/reflow, and outside-region checks follow
the approach reviewed in PDF Forge's PdfiumTextEditor.js (MIT). This is an
independent Python implementation using pypdfium2, not a bundled copy of its
Electron or JavaScript runtime.
"""
from __future__ import annotations

import ctypes
import io
import math
import os
from pathlib import Path
import re
import statistics

import pymupdf

from .file_io import open_pdf, read_bytes, write_bytes
from .pdf_ops import ProcessingError, editable_text_runs, validate_pdf

MAX_TRIAL_BYTES = 16 * 1024 * 1024


def _pdfium():
    try:
        import pypdfium2
        import pypdfium2.raw as raw
        return pypdfium2, raw
    except (ImportError, OSError) as exc:
        raise ProcessingError('PDFium runtime is missing or cannot load; no revision was published') from exc


def trial_status() -> dict:
    if os.getenv('COURTBUNDLE_ENABLE_PDFIUM_TRIAL', '1') == '0':
        return {'available': False, 'engine': 'pdfium-object-edit', 'reason': 'Trial disabled locally'}
    try:
        _pdfium()
    except ProcessingError as exc:
        return {'available': False, 'engine': 'pdfium-object-edit', 'reason': str(exc)}
    return {'available': True, 'engine': 'pdfium-object-edit', 'reason': None}


def _squash(text: str) -> str:
    return re.sub(r'\s+', '', text)


def _overlap(a, b) -> float:
    width = min(a[2], b[2]) - max(a[0], b[0])
    height = min(a[3], b[3]) - max(a[1], b[1])
    return max(0, width) * max(0, height)


def _rect_union(rects) -> tuple[float, float, float, float]:
    return (min(rect[0] for rect in rects), min(rect[1] for rect in rects),
            max(rect[2] for rect in rects), max(rect[3] for rect in rects))


def _collect(pdfium, raw, page):
    textpage = page.get_textpage()
    runs = []
    try:
        for index in range(raw.FPDFPage_CountObjects(page.raw)):
            handle = raw.FPDFPage_GetObject(page.raw, index)
            if not handle or raw.FPDFPageObj_GetType(handle) != raw.FPDF_PAGEOBJ_TEXT:
                continue
            obj = pdfium.PdfObject(handle, page=page, textpage=textpage)
            value = obj.extract()
            if not value.strip():
                continue
            matrix = obj.get_matrix()
            if abs(matrix.b) > .001 or abs(matrix.c) > .001 or matrix.a <= 0 or matrix.d <= 0:
                continue
            font = obj.get_font()
            bounds = obj.get_bounds()
            size = obj.get_font_size()
            if size <= 0 or not all(math.isfinite(v) for v in bounds):
                continue
            rgba = (ctypes.c_uint(), ctypes.c_uint(), ctypes.c_uint(), ctypes.c_uint())
            if not raw.FPDFPageObj_GetFillColor(handle, *rgba):
                raise ProcessingError('PDFium could not inspect paragraph colour')
            runs.append({'index': index, 'handle': handle, 'text': value,
                         'bounds': bounds, 'matrix': matrix, 'size': size,
                         'font': font.raw, 'font_name': font.get_base_name(),
                         'embedded': font.is_embedded,
                         'rgba': tuple(item.value for item in rgba),
                         'render_mode': raw.FPDFTextObj_GetTextRenderMode(handle)})
    finally:
        textpage.close()
    return runs


def _has_untracked_text(raw, page, runs: list[dict]) -> bool:
    tracked = {run['index'] for run in runs}
    return any(raw.FPDFPageObj_GetType(raw.FPDFPage_GetObject(page.raw, index)) ==
               raw.FPDF_PAGEOBJ_TEXT and index not in tracked
               for index in range(raw.FPDFPage_CountObjects(page.raw)))


def _paragraph(runs: list[dict], selected_bbox: list[float], expected_text: str,
               page_height: float) -> list[dict]:
    # PyMuPDF selection boxes are top-left; PDFium page objects are bottom-left.
    x0, y0, x1, y1 = selected_bbox
    target = (x0, page_height - y1, x1, page_height - y0)
    seeds = [run for run in runs if _squash(run['text']) == _squash(expected_text)
             and _overlap(run['bounds'], target) > 0]
    if len(seeds) != 1:
        raise ProcessingError('PDFium needs one exact text object for this selection')
    seed = seeds[0]
    if seed['render_mode'] != 0 or seed['rgba'][3] < 255:
        raise ProcessingError('PDFium trial supports visible opaque fill text only')
    paragraph = [seed]
    for direction in (-1, 1):
        current = seed
        spacing = None
        while len(paragraph) < 100:
            candidates = []
            for run in runs:
                if run in paragraph or run['font_name'] != seed['font_name']:
                    continue
                if (run['rgba'] != seed['rgba'] or run['render_mode'] != seed['render_mode'] or
                        run['embedded'] != seed['embedded'] or
                        any(abs(a - b) > .001 for a, b in zip(
                            run['matrix'].get()[:4], seed['matrix'].get()[:4]))):
                    continue
                if abs(run['size'] - seed['size']) > .15 * seed['size']:
                    continue
                delta = (run['matrix'].f - current['matrix'].f) * direction
                if not .9 * seed['size'] <= delta <= 2.2 * seed['size']:
                    continue
                if spacing is not None and abs(delta - spacing) > .2 * seed['size']:
                    continue
                a, b = current['bounds'], run['bounds']
                horizontal = max(0, min(a[2], b[2]) - max(a[0], b[0]))
                if horizontal / max(1, min(a[2] - a[0], b[2] - b[0])) < .3:
                    continue
                if abs(run['matrix'].e - seed['matrix'].e) > 2.5 * seed['size']:
                    continue
                candidates.append((delta, run))
            if not candidates:
                break
            delta, candidate = min(candidates, key=lambda item: item[0])
            if direction < 0:
                paragraph.append(candidate)
            else:
                paragraph.insert(0, candidate)
            current, spacing = candidate, spacing or delta
    paragraph.sort(key=lambda run: -run['matrix'].f)
    return paragraph


def _selection(path: Path, page_index: int, span_index: int,
               expected_text: str) -> tuple[list[float], bytes]:
    runs = editable_text_runs(path, page_index)
    if not 0 <= span_index < len(runs) or runs[span_index]['text'] != expected_text:
        raise ProcessingError('Selected text changed; reload and select it again')
    source = read_bytes(path)
    if len(source) > MAX_TRIAL_BYTES:
        raise ProcessingError('PDF exceeds the 16 MiB PDFium trial limit')
    return runs[span_index]['bbox'], source


def _inspect(source: bytes, page_index: int, bbox: list[float], expected_text: str):
    pdfium, raw = _pdfium()
    pdf = pdfium.PdfDocument(source)
    try:
        page = pdf[page_index]
        if page.get_rotation():
            raise ProcessingError('Rotated pages are unsupported by the PDFium paragraph trial')
        runs = _collect(pdfium, raw, page)
        paragraph = _paragraph(runs, bbox, expected_text, page.get_height())
        text = ' '.join(run['text'].strip() for run in paragraph)
        if not text:
            raise ProcessingError('PDFium found no editable paragraph')
        if len(text) > 2000:
            raise ProcessingError('Paragraph exceeds the 2,000-character trial limit')
        return pdf, page, runs, paragraph, text, raw
    except BaseException:
        pdf.close()
        raise


def preview_paragraph_trial(path: Path, page_index: int, span_index: int,
                            expected_text: str) -> dict:
    if not trial_status()['available']:
        raise ProcessingError('PDFium paragraph trial is unavailable')
    bbox, source = _selection(path, page_index, span_index, expected_text)
    pdf, page, _, paragraph, text, _ = _inspect(source, page_index, bbox, expected_text)
    try:
        bounds = _rect_union([run['bounds'] for run in paragraph])
        return {'text': text, 'line_count': len(paragraph),
                'font_name': paragraph[0]['font_name'],
                'bbox': [bounds[0], page.get_height() - bounds[3],
                         bounds[2], page.get_height() - bounds[1]],
                'engine': 'pdfium-object-edit'}
    finally:
        pdf.close()


def _text_buffer(value: str):
    encoded = value.encode('utf-16-le') + b'\0\0'
    return (ctypes.c_ushort * (len(encoded) // 2)).from_buffer_copy(encoded)


def _make_text(raw, pdf, font, size, matrix, text, rgba, render_mode):
    handle = raw.FPDFPageObj_CreateTextObj(pdf.raw, font, size)
    if not handle:
        raise ProcessingError('PDFium could not create replacement text')
    try:
        if not raw.FPDFText_SetText(handle, _text_buffer(text)):
            raise ProcessingError('PDFium could not encode replacement text')
        if not raw.FPDFPageObj_SetMatrix(handle, raw.FS_MATRIX(*matrix)):
            raise ProcessingError('PDFium could not position replacement text')
        if not raw.FPDFPageObj_SetFillColor(handle, *rgba):
            raise ProcessingError('PDFium could not set replacement colour')
        if render_mode >= 0:
            raw.FPDFTextObj_SetTextRenderMode(handle, render_mode)
        bounds = (ctypes.c_float(), ctypes.c_float(), ctypes.c_float(), ctypes.c_float())
        if not raw.FPDFPageObj_GetBounds(handle, *bounds):
            raise ProcessingError('PDFium could not measure replacement text')
        return handle, tuple(item.value for item in bounds)
    except BaseException:
        raw.FPDFPageObj_Destroy(handle)
        raise


def _measure(raw, pdf, font, size, matrix, value) -> float:
    handle, bounds = _make_text(raw, pdf, font, size, matrix, value, (0, 0, 0, 255), 0)
    raw.FPDFPageObj_Destroy(handle)
    return bounds[2] - bounds[0]


def _wrap(value: str, width: float, measure) -> list[str]:
    lines = []
    for part in value.split('\n'):
        words = part.split()
        if not words:
            lines.append('')
            continue
        current = words[0]
        if measure(current) > width:
            raise ProcessingError('A replacement word does not fit the paragraph width')
        for word in words[1:]:
            candidate = current + ' ' + word
            if measure(candidate) <= width:
                current = candidate
            elif measure(word) <= width:
                lines.append(current)
                current = word
            else:
                raise ProcessingError('A replacement word does not fit the paragraph width')
        lines.append(current)
    if len(lines) > 100:
        raise ProcessingError('Replacement has too many paragraph lines')
    return lines


def _check_visual_obstacles(source: bytes, page_index: int, pdfium_bounds: tuple,
                            page_height: float) -> None:
    target = pymupdf.Rect(pdfium_bounds[0], page_height - pdfium_bounds[3],
                          pdfium_bounds[2], page_height - pdfium_bounds[1])
    with pymupdf.open(stream=source, filetype='pdf') as pdf:
        page = pdf[page_index]
        obstacles = [pymupdf.Rect(item[:4]) for item in page.get_text('blocks') if item[6] == 1]
        obstacles.extend(pymupdf.Rect(item['from']) for item in page.get_links())
        obstacles.extend(pymupdf.Rect(item['rect']) for item in page.get_drawings())
        obstacles.extend(item.rect for item in (page.widgets() or ()))
        obstacles.extend(item.rect for item in (page.annots() or ()))
        if any(target.intersects(item) for item in obstacles):
            raise ProcessingError('Paragraph overlaps an image, graphic, link or annotation')


def _plan_following_text_shift(runs: list[dict], paragraph: list[dict],
                               old_bounds: tuple, new_bounds: tuple,
                               page_box: tuple) -> tuple[list[dict], float]:
    """Make room for extra lines by moving the subsequent text flow together."""
    others = [run for run in runs if run not in paragraph]
    if not any(_overlap(run['bounds'], new_bounds) > 0 for run in others):
        return [], 0.0
    # A same-row column or a heading above the paragraph is not part of its flow.
    following = [run for run in others if run['bounds'][3] <= old_bounds[1] + 0.5]
    fixed = [run for run in others if run not in following]
    if any(_overlap(run['bounds'], new_bounds) > 0 for run in fixed):
        raise ProcessingError('Replacement would cover text outside this paragraph flow')
    if not following:
        raise ProcessingError('No following text can be moved to make room')
    shift = max(0.0, old_bounds[1] - new_bounds[1])
    if shift <= 0:
        raise ProcessingError('Replacement would cover text outside this paragraph flow')
    if any(run['bounds'][1] - shift < page_box[1] + 2 for run in following):
        raise ProcessingError('Following text cannot fit on this page after reflow')
    for run in following:
        moved = (run['bounds'][0], run['bounds'][1] - shift,
                 run['bounds'][2], run['bounds'][3] - shift)
        if _overlap(moved, new_bounds) > 0 or any(
                _overlap(moved, fixed_run['bounds']) > 0 for fixed_run in fixed):
            raise ProcessingError('Following text cannot be moved without covering other text')
    return following, shift


def _links(page: pymupdf.Page) -> list[dict]:
    return [{key: str(value) for key, value in link.items() if key not in ('xref', 'id')}
            for link in page.get_links()]


def _outside_words(page: pymupdf.Page, region: pymupdf.Rect) -> list[tuple]:
    return [(word[4], tuple(round(value, 2) for value in word[:4]))
            for word in page.get_text('words')
            if not pymupdf.Rect(word[:4]).intersects(region)]


def _inside_text(page: pymupdf.Page, region: pymupdf.Rect) -> str:
    return _squash(' '.join(word[4] for word in page.get_text('words')
                            if pymupdf.Rect(word[:4]).intersects(region)))


def _outside_unchanged(before: pymupdf.Page, after: pymupdf.Page,
                       region: pymupdf.Rect) -> bool:
    rect = before.rect
    if rect.width * rect.height > 8_000_000:
        raise ProcessingError('Page is too large for the paragraph trial pixel check')
    old = before.get_pixmap(alpha=False)
    new = after.get_pixmap(alpha=False)
    if old.width != new.width or old.height != new.height or old.n != new.n:
        return False
    x0 = max(0, math.floor(region.x0 - rect.x0) - 2)
    x1 = min(old.width, math.ceil(region.x1 - rect.x0) + 2)
    y0 = max(0, math.floor(region.y0 - rect.y0) - 2)
    y1 = min(old.height, math.ceil(region.y1 - rect.y0) + 2)
    a, b = memoryview(old.samples), memoryview(new.samples)
    stride = old.width * old.n
    for y in range(old.height):
        start = y * stride
        if y0 <= y < y1:
            if a[start:start + x0 * old.n] != b[start:start + x0 * old.n]:
                return False
            if a[start + x1 * old.n:start + stride] != b[start + x1 * old.n:start + stride]:
                return False
        elif a[start:start + stride] != b[start:start + stride]:
            return False
    return True


def _validate(source: bytes, output: bytes, page_index: int, original_text: str,
              replacement: str, region: pymupdf.Rect) -> None:
    if not output.startswith(b'%PDF-') or len(output) > min(MAX_TRIAL_BYTES * 2, len(source) * 2 + 8_000_000):
        raise ProcessingError('PDFium returned an invalid or oversized PDF')
    with pymupdf.open(stream=source, filetype='pdf') as original, pymupdf.open(stream=output, filetype='pdf') as edited:
        if edited.needs_pass or edited.page_count != original.page_count:
            raise ProcessingError('PDFium changed the page count or protection')
        if edited.get_toc() != original.get_toc() or edited.metadata != original.metadata:
            raise ProcessingError('PDFium changed bookmarks or metadata')
        if edited.embfile_count() != original.embfile_count():
            raise ProcessingError('PDFium changed embedded files')
        for index in range(original.page_count):
            before, after = original[index], edited[index]
            if before.rect != after.rect or before.rotation != after.rotation:
                raise ProcessingError('PDFium changed page geometry')
            if _links(before) != _links(after):
                raise ProcessingError('PDFium changed page links')
            if len(list(before.annots() or ())) != len(list(after.annots() or ())):
                raise ProcessingError('PDFium changed annotations')
            before_widgets = [(item.field_name, item.field_value) for item in (before.widgets() or ())]
            after_widgets = [(item.field_name, item.field_value) for item in (after.widgets() or ())]
            if before_widgets != after_widgets:
                raise ProcessingError('PDFium changed form fields')
            if index != page_index and before.get_text() != after.get_text():
                raise ProcessingError('PDFium changed another page text')
        before, after = original[page_index], edited[page_index]
        output_text = _inside_text(after, region)
        requested_text = _squash(replacement)
        old_text = _squash(original_text)
        if requested_text not in output_text or (old_text not in requested_text and old_text in output_text):
            raise ProcessingError('PDFium did not produce the requested searchable paragraph')
        if _outside_words(before, region) != _outside_words(after, region):
            raise ProcessingError('PDFium changed extracted words outside the paragraph region')
        if not _outside_unchanged(before, after, region):
            raise ProcessingError('PDFium changed pixels outside the paragraph region')


def replace_paragraph_trial(path: Path, page_index: int, span_index: int,
                            expected_text: str, expected_paragraph: str,
                            replacement: str) -> dict:
    if not trial_status()['available']:
        raise ProcessingError('PDFium paragraph trial is unavailable; no revision was published')
    if not replacement.strip():
        raise ProcessingError('Replacement paragraph is empty')
    bbox, source = _selection(path, page_index, span_index, expected_text)
    pdf, page, runs, paragraph, old_text, raw = _inspect(source, page_index, bbox, expected_text)
    try:
        page_count = len(pdf)
        if old_text != expected_paragraph:
            raise ProcessingError('Paragraph changed; reload and select it again')
        if _squash(replacement) == _squash(old_text):
            raise ProcessingError('Replacement matches the current paragraph; no revision was published')
        first = paragraph[0]
        bounds = _rect_union([run['bounds'] for run in paragraph])
        if any(_overlap(run['bounds'], bounds) > 0 for run in runs if run not in paragraph):
            raise ProcessingError('Paragraph overlaps another text object')
        font = first['font']
        font_name = first['font_name']
        font_substituted = False
        if first['embedded'] and any(ch not in old_text for ch in replacement if not ch.isspace()):
            name = font_name.lower()
            family = 'Courier' if 'courier' in name else 'Times' if any(
                value in name for value in ('times', 'serif', 'roman')) else 'Helvetica'
            bold = 'bold' in name or 'black' in name
            italic = 'italic' in name or 'oblique' in name
            font_name = ({(False, False): 'Times-Roman', (True, False): 'Times-Bold',
                          (False, True): 'Times-Italic', (True, True): 'Times-BoldItalic'}
                         [(bold, italic)] if family == 'Times' else
                         family + ('-BoldOblique' if bold and italic else '-Bold' if bold else
                                   '-Oblique' if italic else ''))
            font = raw.FPDFText_LoadStandardFont(pdf.raw, font_name.encode('ascii'))
            if not font:
                raise ProcessingError('PDFium could not load a substitute font')
            font_substituted = True
        matrix = first['matrix']
        matrix_base = (matrix.a, matrix.b, matrix.c, matrix.d)
        measure = lambda text: _measure(raw, pdf, font, first['size'],
                                        (*matrix_base, 0, 0), text)
        page_width, page_height = page.get_size()
        width = (bounds[2] - first['matrix'].e + first['size'] * .5 if len(paragraph) > 1
                 else page_width - first['matrix'].e - 18)
        width = min(width, page_width - first['matrix'].e - 18)
        if width <= 0:
            raise ProcessingError('No room to reflow this paragraph')
        lines = _wrap(replacement, width, measure)
        spacing = (statistics.median([paragraph[i]['matrix'].f - paragraph[i + 1]['matrix'].f
                    for i in range(len(paragraph) - 1)]) if len(paragraph) > 1 else first['size'] * 1.2)
        created = []
        try:
            for line_index, line in enumerate(lines):
                if not line:
                    continue
                x = first['matrix'].e
                y = first['matrix'].f - line_index * spacing
                handle, rect = _make_text(raw, pdf, font, first['size'],
                                          (*matrix_base, x, y), line,
                                          first['rgba'], first['render_mode'])
                created.append((handle, rect, line))
            if not created:
                raise ProcessingError('Replacement paragraph has no visible text')
            new_bounds = _rect_union([item[1] for item in created])
            page_box = page.get_bbox()
            if (new_bounds[0] < page_box[0] + 2 or new_bounds[1] < page_box[1] + 2 or
                    new_bounds[2] > page_box[2] - 2 or new_bounds[3] > page_box[3] - 2):
                raise ProcessingError('Replacement paragraph would leave the page')
            following, shift = _plan_following_text_shift(
                runs, paragraph, bounds, new_bounds, page_box)
            if following and _has_untracked_text(raw, page, runs):
                raise ProcessingError('Page has unsupported text objects; following text cannot be reflowed safely')
            combined = _rect_union([bounds, new_bounds] + [
                (run['bounds'][0], run['bounds'][1] - shift,
                 run['bounds'][2], run['bounds'][3]) for run in following] +
                [run['bounds'] for run in following])
            _check_visual_obstacles(source, page_index, combined, page_height)
            if (combined[2] - combined[0]) * (combined[3] - combined[1]) > page_width * page_height * .5:
                raise ProcessingError('Paragraph trial region is too large')
            old_indexes = sorted((run['index'] for run in paragraph), reverse=True)
            for index in old_indexes:
                obj = raw.FPDFPage_GetObject(page.raw, index)
                if not obj or not raw.FPDFPage_RemoveObject(page.raw, obj):
                    raise ProcessingError('PDFium could not remove the old paragraph object')
                raw.FPDFPageObj_Destroy(obj)
            for run in following:
                matrix = run['matrix']
                if not raw.FPDFPageObj_SetMatrix(run['handle'], raw.FS_MATRIX(
                        matrix.a, matrix.b, matrix.c, matrix.d,
                        matrix.e, matrix.f - shift)):
                    raise ProcessingError('PDFium could not move following text')
            insert_at = min(old_indexes)
            for item in tuple(created):
                handle = item[0]
                if not raw.FPDFPage_InsertObjectAtIndex(page.raw, handle, insert_at):
                    raise ProcessingError('PDFium could not insert replacement text')
                created.remove(item)  # the page owns it from here onward
                insert_at += 1
            if not raw.FPDFPage_GenerateContent(page.raw):
                raise ProcessingError('PDFium could not regenerate page content')
            output_stream = io.BytesIO()
            page.close()
            pdf.save(output_stream)
            output = output_stream.getvalue()
            region = pymupdf.Rect(combined[0] - 3, page_height - combined[3] - 3,
                                  combined[2] + 3, page_height - combined[1] + 3)
            _validate(source, output, page_index, old_text, replacement, region)
        finally:
            for handle, _, _ in created:
                raw.FPDFPageObj_Destroy(handle)
    finally:
        pdf.close()
    write_bytes(path, output)
    validate_pdf(path, page_count)
    return {'edit_method': 'native-content-edit', 'engine': 'pdfium-object-edit',
            'font_substituted': font_substituted, 'font_name': font_name,
            'line_count': len(lines), 'edit_region': list(region),
            'following_text_shift': round(shift, 2),
            'following_text_objects_moved': len(following)}
