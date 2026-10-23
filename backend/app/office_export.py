"""Editable review exports derived from an identified, completed bundle.

The PDF and its deterministic index CSV remain authoritative. These Office
files are deliberately labelled drafts: PDF text extraction cannot recreate
page layout, images, signatures, or a court-approved filing document.
"""
from __future__ import annotations

import csv
from io import BytesIO, StringIO
from pathlib import Path
import re
from textwrap import wrap
from xml.etree import ElementTree as ET
from zipfile import ZIP_DEFLATED, ZipFile

from .file_io import open_pdf

P = 'http://schemas.openxmlformats.org/presentationml/2006/main'
A = 'http://schemas.openxmlformats.org/drawingml/2006/main'
R = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'
PKG_R = 'http://schemas.openxmlformats.org/package/2006/relationships'
CT = 'http://schemas.openxmlformats.org/package/2006/content-types'
W = 'http://schemas.openxmlformats.org/wordprocessingml/2006/main'
S = 'http://schemas.openxmlformats.org/spreadsheetml/2006/main'

for prefix, uri in [('p', P), ('a', A), ('r', R), ('w', W)]:
    ET.register_namespace(prefix, uri)


def _xml(element: ET.Element) -> bytes:
    return ET.tostring(element, encoding='utf-8', xml_declaration=True)


def _clean(value: str) -> str:
    # XML 1.0 does not permit most control characters. Do not interpret Excel
    # formula-looking PDF text as a formula; all cells are written as strings.
    return re.sub(r'[\x00-\x08\x0b\x0c\x0e-\x1f]', '', value)


def _package(parts: dict[str, bytes]) -> bytes:
    stream = BytesIO()
    with ZipFile(stream, 'w', compression=ZIP_DEFLATED, compresslevel=6) as archive:
        for name, content in parts.items():
            archive.writestr(name, content)
    return stream.getvalue()


def _pdf_pages(path: Path) -> list[list[str]]:
    pages: list[list[str]] = []
    total_characters = 0
    with open_pdf(path) as pdf:
        if len(pdf) > 2000:
            raise ValueError('Office review draft is limited to 2,000 PDF pages')
        for page in pdf:
            lines = [_clean(line).strip() for line in page.get_text(sort=True).splitlines()]
            pages.append([line for line in lines if line])
            total_characters += sum(map(len, pages[-1]))
            if total_characters > 10_000_000:
                raise ValueError('Office review draft exceeds the 10 million character limit')
    if not any(pages):
        raise ValueError('This PDF has no extractable text. Review OCR before making an editable draft')
    return pages


def word_draft(path: Path) -> bytes:
    pages = _pdf_pages(path)
    body = ET.Element(f'{{{W}}}body')
    for index, lines in enumerate(pages):
        if index:
            paragraph = ET.SubElement(body, f'{{{W}}}p')
            run = ET.SubElement(paragraph, f'{{{W}}}r')
            ET.SubElement(run, f'{{{W}}}br', {f'{{{W}}}type': 'page'})
        for line in ([f'PDF page {index + 1} — editable review draft'] +
                     (lines or ['[No extractable text on this page; inspect the source PDF]'])):
            paragraph = ET.SubElement(body, f'{{{W}}}p')
            run = ET.SubElement(paragraph, f'{{{W}}}r')
            ET.SubElement(run, f'{{{W}}}t', {'{http://www.w3.org/XML/1998/namespace}space': 'preserve'}).text = line
    ET.SubElement(body, f'{{{W}}}sectPr')
    document = ET.Element(f'{{{W}}}document')
    document.append(body)
    types = ET.Element(f'{{{CT}}}Types')
    ET.SubElement(types, f'{{{CT}}}Default', {'Extension': 'rels', 'ContentType': 'application/vnd.openxmlformats-package.relationships+xml'})
    ET.SubElement(types, f'{{{CT}}}Default', {'Extension': 'xml', 'ContentType': 'application/xml'})
    ET.SubElement(types, f'{{{CT}}}Override', {'PartName': '/word/document.xml',
        'ContentType': 'application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml'})
    relationships = ET.Element(f'{{{PKG_R}}}Relationships')
    ET.SubElement(relationships, f'{{{PKG_R}}}Relationship', {'Id': 'rId1',
        'Type': 'http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument',
        'Target': 'word/document.xml'})
    return _package({'[Content_Types].xml': _xml(types), '_rels/.rels': _xml(relationships),
                     'word/document.xml': _xml(document)})


def excel_index(csv_data: bytes) -> bytes:
    source = StringIO(csv_data.decode('utf-8-sig', errors='strict'), newline='')
    rows = list(csv.reader(source))
    if not rows or not rows[0]:
        raise ValueError('The completed build has no index rows to export')
    if len(rows) > 1_048_576 or max(map(len, rows)) > 16_384:
        raise ValueError('Index exceeds Excel worksheet limits')
    worksheet = ET.Element(f'{{{S}}}worksheet')
    views = ET.SubElement(worksheet, f'{{{S}}}sheetViews')
    view = ET.SubElement(views, f'{{{S}}}sheetView', {'workbookViewId': '0'})
    ET.SubElement(view, f'{{{S}}}pane', {'ySplit': '1', 'topLeftCell': 'A2', 'activePane': 'bottomLeft', 'state': 'frozen'})
    columns = ET.SubElement(worksheet, f'{{{S}}}cols')
    for column in range(max(map(len, rows))):
        longest = max((len(row[column]) for row in rows if column < len(row)), default=10)
        width = min(64, max(14, longest + 2))
        ET.SubElement(columns, f'{{{S}}}col', {'min': str(column + 1), 'max': str(column + 1),
                                              'width': str(width), 'customWidth': '1'})
    data = ET.SubElement(worksheet, f'{{{S}}}sheetData')
    for row_number, values in enumerate(rows, 1):
        row = ET.SubElement(data, f'{{{S}}}row', {'r': str(row_number)})
        for column, value in enumerate(values):
            if len(value) > 32_767:
                raise ValueError('Index cell exceeds Excel text limit')
            letters = ''
            number = column + 1
            while number:
                number, remainder = divmod(number - 1, 26)
                letters = chr(65 + remainder) + letters
            cell = ET.SubElement(row, f'{{{S}}}c', {'r': f'{letters}{row_number}', 't': 'inlineStr'})
            ET.SubElement(ET.SubElement(cell, f'{{{S}}}is'), f'{{{S}}}t').text = _clean(value)
    last_column = max(map(len, rows))
    last_letters = ''
    while last_column:
        last_column, remainder = divmod(last_column - 1, 26)
        last_letters = chr(65 + remainder) + last_letters
    ET.SubElement(worksheet, f'{{{S}}}autoFilter', {'ref': f'A1:{last_letters}{len(rows)}'})
    workbook = ET.Element(f'{{{S}}}workbook', {f'{{{R}}}dummy': ''})
    del workbook.attrib[f'{{{R}}}dummy']
    sheets = ET.SubElement(workbook, f'{{{S}}}sheets')
    ET.SubElement(sheets, f'{{{S}}}sheet', {'name': 'Bundle index', 'sheetId': '1', f'{{{R}}}id': 'rId1'})
    root_rels = ET.Element(f'{{{PKG_R}}}Relationships')
    ET.SubElement(root_rels, f'{{{PKG_R}}}Relationship', {'Id': 'rId1',
        'Type': 'http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument',
        'Target': 'xl/workbook.xml'})
    book_rels = ET.Element(f'{{{PKG_R}}}Relationships')
    ET.SubElement(book_rels, f'{{{PKG_R}}}Relationship', {'Id': 'rId1',
        'Type': 'http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet',
        'Target': 'worksheets/sheet1.xml'})
    types = ET.Element(f'{{{CT}}}Types')
    ET.SubElement(types, f'{{{CT}}}Default', {'Extension': 'rels', 'ContentType': 'application/vnd.openxmlformats-package.relationships+xml'})
    ET.SubElement(types, f'{{{CT}}}Default', {'Extension': 'xml', 'ContentType': 'application/xml'})
    ET.SubElement(types, f'{{{CT}}}Override', {'PartName': '/xl/workbook.xml',
        'ContentType': 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml'})
    ET.SubElement(types, f'{{{CT}}}Override', {'PartName': '/xl/worksheets/sheet1.xml',
        'ContentType': 'application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml'})
    return _package({'[Content_Types].xml': _xml(types), '_rels/.rels': _xml(root_rels),
                     'xl/workbook.xml': _xml(workbook),
                     'xl/_rels/workbook.xml.rels': _xml(book_rels),
                     'xl/worksheets/sheet1.xml': _xml(worksheet)})


def powerpoint_draft(path: Path) -> bytes:
    pages = _pdf_pages(path)
    template = Path(__file__).parent / 'assets' / 'editable-draft-template.pptx'
    with ZipFile(template) as archive:
        parts = {name: archive.read(name) for name in archive.namelist()}
    presentation = ET.fromstring(parts['ppt/presentation.xml'])
    slide_ids = presentation.find(f'{{{P}}}sldIdLst')
    assert slide_ids is not None
    slide_ids.clear()
    relationships = ET.fromstring(parts['ppt/_rels/presentation.xml.rels'])
    for item in list(relationships):
        if item.attrib.get('Type', '').endswith('/slide'):
            relationships.remove(item)
    types = ET.fromstring(parts['[Content_Types].xml'])
    for item in list(types):
        if item.attrib.get('PartName', '').startswith('/ppt/slides/slide'):
            types.remove(item)
    slide_template = parts.pop('ppt/slides/slide1.xml')
    slide_rels = parts.pop('ppt/slides/_rels/slide1.xml.rels')
    slide_number = 0
    for page_number, lines in enumerate(pages, 1):
        # More slides keep dense extracted pages editable without silently
        # clipping text in a single overflowing placeholder.
        content_lines = [piece for line in (lines or ['[No extractable text; inspect the source PDF]'])
                         for piece in (wrap(line, width=110, break_long_words=True) or [''])]
        chunks: list[list[str]] = []
        for line in content_lines:
            if not chunks or len(chunks[-1]) >= 10 or sum(map(len, chunks[-1])) + len(line) > 900:
                chunks.append([])
            chunks[-1].append(line)
        for part_number, chunk in enumerate(chunks, 1):
            slide_number += 1
            slide = ET.fromstring(slide_template)
            texts = slide.findall(f'.//{{{A}}}t')
            if len(texts) < 2:
                raise ValueError('PowerPoint template is missing editable text placeholders')
            texts[0].text = f'PDF page {page_number}' + (f' · part {part_number}' if len(chunks) > 1 else '')
            text_bodies = slide.findall(f'.//{{{P}}}txBody')
            if len(text_bodies) < 2:
                raise ValueError('PowerPoint template is missing its content placeholder')
            content_body = text_bodies[1]
            for paragraph in list(content_body):
                if paragraph.tag == f'{{{A}}}p':
                    content_body.remove(paragraph)
            for line in chunk:
                paragraph = ET.SubElement(content_body, f'{{{A}}}p')
                properties = ET.SubElement(paragraph, f'{{{A}}}pPr', {'lvl': '0', 'marL': '0', 'indent': '0'})
                ET.SubElement(properties, f'{{{A}}}buNone')
                run = ET.SubElement(paragraph, f'{{{A}}}r')
                ET.SubElement(run, f'{{{A}}}rPr')
                ET.SubElement(run, f'{{{A}}}t').text = line
            name = f'slide{slide_number}.xml'
            parts[f'ppt/slides/{name}'] = _xml(slide)
            parts[f'ppt/slides/_rels/{name}.rels'] = slide_rels
            relationship_id = f'rId{100 + slide_number}'
            ET.SubElement(slide_ids, f'{{{P}}}sldId', {'id': str(255 + slide_number), f'{{{R}}}id': relationship_id})
            ET.SubElement(relationships, f'{{{PKG_R}}}Relationship', {'Id': relationship_id,
                'Type': 'http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide',
                'Target': f'slides/{name}'})
            ET.SubElement(types, f'{{{CT}}}Override', {'PartName': f'/ppt/slides/{name}',
                'ContentType': 'application/vnd.openxmlformats-officedocument.presentationml.slide+xml'})
    parts['ppt/presentation.xml'] = _xml(presentation)
    parts['ppt/_rels/presentation.xml.rels'] = _xml(relationships)
    parts['[Content_Types].xml'] = _xml(types)
    return _package(parts)
