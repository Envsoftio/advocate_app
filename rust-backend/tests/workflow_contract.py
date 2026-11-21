"""Compare document preparation and working-copy edits against live Python."""
from base64 import b64decode
from io import BytesIO
import json
from pathlib import Path
import re
import tempfile
from urllib.parse import quote, urlencode
from zipfile import ZipFile

import pymupdf


from vault_contract import (
    PASSWORD, TestClient, create_app, pdf_semantics, revision_fixture,
    running_vault,
)

AUTH = {'Authorization': 'Bearer synthetic'}
UPLOAD = {**AUTH, 'Content-Type': 'application/octet-stream'}
VOLATILE = {'created_at', 'updated_at', 'source_relpath', 'working_relpath', 'input_snapshot',
            'evidence_id', 'document_versions', 'effective_input_digest', 'input_digest'}

def compare_crop_and_binary_replacement(root, rust, python, matter_id, py_matter_id):
    original = root / 'source-cropped-page.pdf'
    with pymupdf.open() as pdf:
        first = pdf.new_page(width=200, height=180)
        first.insert_text((26, 45), 'First cropped page')
        crop_value = pdf.get_new_xref()
        pdf.update_object(crop_value, '[160 120 -50 -40]')
        pdf.xref_set_key(first.xref, 'CropBox', f'{crop_value} 0 R')
        rotation_value = pdf.get_new_xref()
        pdf.update_object(rotation_value, '90')
        pdf.xref_set_key(first.xref, 'Rotate', f'{rotation_value} 0 R')
        for label in ('Middle page stays', 'Last page is replaced'):
            pdf.new_page().insert_text((35, 75), label)
        pdf.save(original)
    source = root / 'binary-replacement-café.pdf'
    with pymupdf.open() as pdf:
        page = pdf.new_page()
        page.insert_text((50, 80), 'Replacement goes in the last slot')
        stream, = page.get_contents()
        pdf.update_stream(stream, b'%' + b'x' * (10 * 1024 * 1024) + b'\n' + pdf.xref_stream(stream),
                          compress=False)
        pdf.new_page().insert_text((50, 80), 'Replacement goes in the first slot')
        pdf.save(source)
    assert source.stat().st_size > 10 * 1024 * 1024
    data = original.read_bytes()
    reference = python.post(f'/matters/{py_matter_id}/documents/raw', content=data,
                            headers={**UPLOAD, 'X-CourtBundle-Filename': original.name})
    reference.raise_for_status()
    baseline = reference.json()[0]['id']
    native = rust('IMPORT', matter_id, source=str(original), status=201)[0]['id']
    a, b = f'/documents/{native}', f'/documents/{baseline}'
    crop = {'expected_revision': 1, 'pages': [0], 'left': .25, 'top': .25, 'right': .75, 'bottom': .75}
    assert rust('POST', a + '/pages/crop', crop)['revision'] == 2
    response = python.post(b + '/pages/crop', headers=AUTH, json=crop)
    response.raise_for_status()
    assert response.json()['revision'] == 2

    def compare_revision(revision):
        actual = bytes(rust('SNAPSHOT', a, {'kind': 'working', 'revision': revision})['bytes'])
        expected = python.get(b + '/pdf', headers=AUTH, params={'revision': revision})
        expected.raise_for_status()
        assert pdf_semantics(actual) == pdf_semantics(expected.content)
        return actual

    cropped = compare_revision(2)
    with pymupdf.open(stream=cropped) as pdf:
        assert pdf[0].rotation == 90
        assert tuple(pdf[0].cropbox) == (2.5, 100.0, 107.5, 180.0)

    turn = {'expected_revision': 2, 'page_index': 0, 'degrees': 90}
    assert rust('POST', a + '/pages/rotate', turn)['revision'] == 3
    response = python.post(b + '/pages/rotate', headers=AUTH, json=turn)
    response.raise_for_status()
    assert response.json()['revision'] == 3
    with pymupdf.open(stream=compare_revision(3)) as pdf:
        assert pdf[0].rotation == 180
        assert tuple(pdf[0].cropbox) == (2.5, 100.0, 107.5, 180.0)

    for order, revision, status, detail in (
        ('[0]', 2, 409, 'Working PDF revision changed; reload the document'),
        ('[99]', 3, 422, 'Replacement has 2 pages; select exactly 2 working pages'),
        ('[0,99]', 3, 422, 'Select existing pages using zero-based page indexes'),
        ('[2,0]', 3, 200, None),
    ):
        metadata = urlencode({'content_type': 'application/octet-stream', 'filename': source.name,
                              'target_pages': order, 'expected_revision': revision})
        native_reply = rust('REPLACE_RAW', native, source=str(source), metadata=metadata, status=status)
        expected = python.post(b + '/pages/replace-file', content=source.read_bytes(), headers={
            **UPLOAD, 'X-CourtBundle-Filename': quote(source.name), 'X-CourtBundle-Target-Pages': order,
            'X-CourtBundle-Expected-Revision': str(revision),
        })
        assert expected.status_code == status, expected.text
        if detail:
            assert native_reply == expected.json() == {'detail': detail}
        else:
            assert native_reply['revision'] == expected.json()['revision'] == 4
    replaced = compare_revision(4)
    with pymupdf.open(stream=replaced) as pdf:
        assert [page.get_text().strip() for page in pdf] == [
            'Replacement goes in the first slot', 'Middle page stays', 'Replacement goes in the last slot']
    assert bytes(rust('SNAPSHOT', a, {'kind': 'original'})['bytes']) == data
    assert python.get(b + '/original', headers=AUTH).content == data


def compare_page_form_graphs(root, rust, python, matter, py_matter):
    def create(path, caption):
        with pymupdf.open() as pdf:
            anchors = []
            widgets = []
            for index in range(3):
                page = pdf.new_page(width=320, height=240)
                anchors.append(page.xref)
                page.insert_text((30, 42), f'{caption} page {index + 1}')
                widget = pymupdf.Widget()
                widget.rect = pymupdf.Rect(25, 76, 155, 102)
                widget.field_type = pymupdf.PDF_WIDGET_TYPE_TEXT
                widget.field_name = 'Shared' if index < 2 else 'Solo'
                widget.field_value = f'{caption} field {index + 1}'
                if index == 2:
                    widget.script_calc = 'event.value = event.value;'
                widgets.append(page.add_widget(widget).xref)
                pdf.xref_set_key(widgets[-1], 'P', f'{page.xref} 0 R')
                note = page.add_text_annot((220, 134), f'{caption} note {index + 1}')
                note.set_popup(pymupdf.Rect(225, 155, 290, 185))
                reply = page.add_text_annot((200, 175), f'{caption} reply {index + 1}')
                reply.set_irt_xref(note.xref)
                pdf.xref_set_key(page.xref, 'AA', '<< /O << /S /Named /N /NextPage >> >>')
            parent = pdf.get_new_xref()
            pdf.update_object(parent, f'<< /FT /Tx /T (Shared) /V ({caption} shared value) '
                                      f'/Kids [{widgets[0]} 0 R {widgets[1]} 0 R] >>')
            for widget in widgets[:2]:
                for field in ('FT', 'T', 'V'):
                    pdf.xref_set_key(widget, field, 'null')
                pdf.xref_set_key(widget, 'Parent', f'{parent} 0 R')
            catalog = pdf.pdf_catalog()
            pdf.xref_set_key(catalog, 'AcroForm/Fields', f'[{parent} 0 R {widgets[2]} 0 R]')
            pdf.xref_set_key(catalog, 'AcroForm/CO', f'[{widgets[2]} 0 R]')
            pdf.xref_set_key(catalog, 'AcroForm/NeedAppearances', 'false')
            pdf.set_toc([[1, 'Opening', 1], [2, 'Middle', 2], [2, 'Final', 3]])
            pdf.set_page_labels([
                dict(startpage=0, style='D', prefix='A-', firstpagenum=3),
                dict(startpage=1, style='D', prefix='B-', firstpagenum=5),
                dict(startpage=2, style='r', prefix='C-', firstpagenum=2),
            ])
            pdf.embfile_add('synthetic.txt', caption.encode('ascii'))
            pdf.xref_set_key(catalog, 'OpenAction', f'[{anchors[1]} 0 R /Fit]')
            pdf.xref_set_key(catalog, 'Dests', f'<< /Review [{anchors[1]} 0 R /XYZ 30 192 0] >>')
            for index, page in enumerate(pdf):
                page.insert_link({'kind': pymupdf.LINK_GOTO, 'from': pymupdf.Rect(35, 149, 110, 164),
                                  'page': (index + 1) % 3, 'to': pymupdf.Point(30, 42)})

            root_pages = int(pdf.xref_get_key(catalog, 'Pages')[1].split()[0])
            branch = pdf.get_new_xref()
            pdf.update_object(branch, f'<< /Type /Pages /Parent {root_pages} 0 R /Count 2 '
                                      f'/Kids [{anchors[0]} 0 R {anchors[1]} 0 R] /Rotate 90 >>')
            pdf.xref_set_key(root_pages, 'Kids', f'[{branch} 0 R {anchors[2]} 0 R]')
            for page in anchors[:2]:
                pdf.xref_set_key(page, 'Parent', f'{branch} 0 R')
                pdf.xref_set_key(page, 'Rotate', 'null')
            pdf.set_metadata({'title': caption})
            pdf.save(path)

    def graph(data):
        def refs(text):
            return [int(value) for value in re.findall(r'(\d+)\s+\d+\s+R', text)]

        def base_name(title):
            return re.sub(r'(?: \[\d+\])+$', '', title or '')

        with pymupdf.open(stream=data, filetype='pdf') as pdf:
            owners = {w.xref: page.number for page in pdf for w in page.widgets()}
            addresses = {page.xref: page.number for page in pdf}
            seen = set()

            def field(xref, parent=None):
                assert xref not in seen, 'A field occurrence belongs to two parents'
                seen.add(xref)
                assert refs(pdf.xref_get_key(xref, 'Parent')[1]) == ([] if parent is None else [parent])
                name = pdf.xref_get_key(xref, 'T')[1]
                kids = refs(pdf.xref_get_key(xref, 'Kids')[1])
                return (base_name(name), pdf.xref_get_key(xref, 'FT')[1], pdf.xref_get_key(xref, 'V')[1],
                        owners.get(xref), tuple(field(kid, xref) for kid in kids))

            catalog = pdf.pdf_catalog()
            roots = refs(pdf.xref_get_key(catalog, 'AcroForm/Fields')[1])
            names = [pdf.xref_get_key(xref, 'T')[1] for xref in roots]
            assert len(names) == len(set(names))
            form = tuple(field(root) for root in roots)
            assert owners.keys() <= seen, 'A visible widget has no field in AcroForm'
            order = tuple(owners.get(xref) for xref in refs(pdf.xref_get_key(catalog, 'AcroForm/CO')[1]))
            notes = [[(note.info['content'], note.popup_xref > 0, note.irt_xref > 0)
                      for note in page.annots() or ()] for page in pdf]
            action = refs(pdf.xref_get_key(catalog, 'OpenAction')[1])
            return (form, order, notes, pdf.embfile_names(), pdf.get_page_labels(),
                    tuple(addresses.get(xref) for xref in action),
                    pdf.xref_get_key(catalog, 'Dests')[0] != 'null',
                    [pdf.xref_get_key(page.xref, 'AA')[0] != 'null' for page in pdf])

    def compare(actual, expected):
        assert pdf_semantics(actual) == pdf_semantics(expected)
        assert graph(actual) == graph(expected)

    target, source = root / 'form-target.pdf', root / 'form-source.pdf'
    create(target, 'Target')
    create(source, 'Source')
    ids = []
    for file in (target, source):
        item = rust('IMPORT', matter, source=str(file), status=201)[0]
        response = python.post(f'/matters/{py_matter}/documents/raw', content=file.read_bytes(),
                               headers={**UPLOAD, 'X-CourtBundle-Filename': file.name})
        assert response.status_code == 201, response.text
        ids.append((item['id'], response.json()[0]['id']))
    (native, reference), (other, py_other) = ids
    a, b = f'/documents/{native}', f'/documents/{reference}'

    for operation, field, selection in (('extract', 'selection', '1,0,1'), ('split', 'after', '0')):
        actual = bytes(rust('PAGE_OUTPUT', native, operation=operation, selection=selection)['bytes'])
        response = python.get(f'{b}/pages/{operation}', params={field: selection}, headers=AUTH)
        assert response.status_code == 200, response.text
        if operation == 'extract':
            compare(actual, response.content)
        else:
            with ZipFile(BytesIO(actual)) as left, ZipFile(BytesIO(response.content)) as right:
                assert left.namelist() == right.namelist()
                for part in left.namelist():
                    compare(left.read(part), right.read(part))

    def current(revision):
        actual = bytes(rust('SNAPSHOT', a, {'kind': 'working', 'revision': revision})['bytes'])
        expected = python.get(f'{b}/pdf', params={'revision': revision}, headers=AUTH)
        expected.raise_for_status()
        compare(actual, expected.content)
        return actual

    def edit(revision, operation, change):
        fields = {'expected_revision': revision, **change}
        target_fields = dict(fields, source_document_id=other) if 'expected_source_revision' in fields else fields
        python_fields = dict(fields, source_document_id=py_other) if 'expected_source_revision' in fields else fields
        native_edit = rust('POST', f'{a}/{operation}', target_fields)
        response = python.post(f'{b}/{operation}', json=python_fields, headers=AUTH)
        assert response.status_code == 200, response.text
        assert native_edit['revision'] == response.json()['revision'] == revision + 1
        return revision + 1

    revision = 1
    for operation, change in (
        ('pages/rotate', {'page_index': '1', 'degrees': '270.0'}),
        ('pages/crop', {'pages': [1], 'left': .01, 'top': .02, 'right': .99, 'bottom': .98}),
        ('pages/delete', {'page_index': 0}),
        ('pages/batch', {'operation': 'delete', 'pages': [0]}),
    ):
        revision = edit(revision, operation, change)
        current(revision)

    original_hash = next(item['sha256'] for item in rust('GET', f'{a}/revisions') if item['revision'] == 1)
    for operation, change in (
        ('pages/reorder', {'order': [1, 2, 0]}),
        ('pages/insert', {'expected_source_revision': 1, 'source_pages': [0, 1, 0], 'before_index': 1}),
        ('pages/replace', {'expected_source_revision': 1, 'source_pages': [2], 'target_pages': [2]}),
        ('replace', {'expected_source_revision': 1}),
    ):
        revision = edit(revision, 'revisions/1/restore', {'expected_sha256': original_hash})
        assert current(revision) == target.read_bytes()
        revision = edit(revision, operation, change)
        current(revision)

    # A repeated reference assembly can lose previously grafted form fields.
    # Reordering our saved form must continue to show every selected widget.
    revision = edit(revision, 'pages/reorder', {'order': [1, 2, 0]})
    assembled = bytes(rust('SNAPSHOT', a, {'kind': 'working', 'revision': revision})['bytes'])
    graph(assembled)
    with pymupdf.open(stream=assembled) as pdf:
        for page, index in zip(pdf, (2, 3, 1), strict=True):
            assert f'Source page {index}' in page.get_text()
            widget, = page.widgets()
            assert widget.field_value == ('Source field 3' if index == 3 else 'Source shared value')

    revision = edit(revision, 'revisions/1/restore', {
        'expected_sha256': original_hash, 'expected_revision': str(revision),
    })
    assert current(revision) == target.read_bytes()
    targets = '[2,0,1]'
    raw_fields = urlencode({'content_type': 'application/octet-stream', 'filename': source.name,
                            'target_pages': targets, 'expected_revision': revision})
    uploaded = rust('REPLACE_RAW', native, source=str(source), metadata=raw_fields)
    response = python.post(f'{b}/pages/replace-file', content=source.read_bytes(), headers={
        **UPLOAD, 'X-CourtBundle-Filename': quote(source.name), 'X-CourtBundle-Target-Pages': targets,
        'X-CourtBundle-Expected-Revision': str(revision),
    })
    assert response.status_code == 200, response.text
    assert uploaded['revision'] == response.json()['revision'] == revision + 1
    current(revision + 1)

    assert bytes(rust('SNAPSHOT', a, {'kind': 'original'})['bytes']) == target.read_bytes()
    assert bytes(rust('SNAPSHOT', a, {'kind': 'working', 'revision': 1})['bytes']) == target.read_bytes()
    assert bytes(rust('SNAPSHOT', f'/documents/{other}', {'kind': 'original'})['bytes']) == source.read_bytes()
    assert python.get(f'{b}/original', headers=AUTH).content == target.read_bytes()


def run(root):
    source = root / 'rotation.pdf'
    revision_fixture(source)
    second_source = root / 'related.pdf'
    with pymupdf.open(source) as related:
        related[1].insert_text((80, 120), 'Distinct second document. Annexure A-1 and Exhibit A-3.')
        related[1].insert_link({'kind': pymupdf.LINK_GOTO, 'from': pymupdf.Rect(60, 110, 160, 130), 'page': 1})
        related[1].insert_link({'kind': pymupdf.LINK_URI, 'from': pymupdf.Rect(60, 140, 160, 160),
                                'uri': 'https://example.com/record'})
        related.set_toc([[1, 'Related source', 1], [1, 'Related destination', 2]])
        related.save(second_source)
    substitutions = {}
    compared = 0

    def normalize(value, *, result=True):
        if isinstance(value, dict):
            return {key: normalize(item, result=result) for key, item in value.items()
                    if not result or key not in VOLATILE}
        if isinstance(value, list):
            return [normalize(item, result=result) for item in value]
        if isinstance(value, str):
            for original, replacement in substitutions.items():
                value = value.replace(original, replacement)
        return value

    with running_vault(root / 'vault') as rust, TestClient(
        create_app(root / 'python', token='synthetic', require_vault=False)
    ) as python:
        rust('POST', '/vault/setup', {'password': PASSWORD})

        def paired(method, path, body=None, rust_body=None, status=200, compare=True):
            nonlocal compared
            actual = rust(method, path, body, status=status, key_source=json.dumps(body, ensure_ascii=False))
            response = python.request(method, normalize(path), headers=AUTH,
                                      json=normalize(body if rust_body is None else rust_body, result=False))
            assert response.status_code == status, (path, response.status_code, response.text)
            expected = response.json()
            if compare:
                assert normalize(actual) == normalize(expected), (path, actual, expected)
                compared += 1
            return actual, expected

        matter, reference = paired('POST', '/matters', {'name': 'Synthetic workflow parity'},
                                   status=201, compare=False)
        substitutions[matter['id']] = reference['id']
        route = f"/matters/{matter['id']}"
        for endpoint in ('/membership', '/bundle-settings'):
            rust_state, python_state = paired('GET', route + endpoint)
            assert rust_state['input_snapshot'] == python_state['input_snapshot']

        empty, py_empty = paired('GET', route + '/preflight/review')
        assert empty['input_digest'] == py_empty['input_digest']
        baseline, py_baseline = paired('GET', route + '/bundle-settings')
        updated, py_updated = paired('PUT', route + '/bundle-settings',
            {'max_pages_per_volume': 12, 'keep_groups_intact': True,
             'expected_snapshot': baseline['input_snapshot'],
             'output_marks': {'numbering_style': 'roman_lower', 'timestamp_timezone': 'Asia/Kolkata'}},
            {'max_pages_per_volume': 12, 'keep_groups_intact': True,
             'expected_snapshot': py_baseline['input_snapshot'],
             'output_marks': {'numbering_style': 'roman_lower', 'timestamp_timezone': 'Asia/Kolkata'}})
        assert updated['input_snapshot'] == py_updated['input_snapshot']
        assert updated['input_snapshot'] != baseline['input_snapshot']
        paired('POST', route + '/history/undo')
        paired('POST', route + '/history/redo')
        for i, name in enumerate(('Review Layout', 'review layout'), 1):
            preset, py_preset = paired('POST', '/output-presets',
                {'name': name, 'max_pages_per_volume': 10 + i, 'output_marks': {'index_style': 'review'}},
                status=201, compare=False)
            substitutions[preset['id']] = py_preset['id']
            assert normalize(preset) == normalize(py_preset)
            assert preset['version'] == i
        paired('GET', '/output-presets')
        paired('POST', route + f'/output-preset/{preset["id"]}', {'preserve_overrides': True})
        paired('GET', route + '/output-preset')
        paired('PUT', route + '/output-marks',
            {'index_style': 'readable', 'cover_title': 'Synthetic record', 'numbering_style': 'roman_upper'})
        paired('POST', route + f'/output-preset/{preset["id"]}', {'preserve_overrides': True})
        paired('GET', route + '/output-marks')

        docs = []
        for file in (source, second_source):
            doc = rust('IMPORT', matter['id'], status=201, source=str(file), included=True)[0]
            response = python.post(normalize(route) + '/documents/raw?included=true', content=file.read_bytes(),
                                   headers={**UPLOAD, 'X-CourtBundle-Filename': file.name})
            assert response.status_code == 201, response.text
            substitutions[doc['id']] = response.json()[0]['id']
            docs.append(doc['id'])
        first, second = docs
        first_route = f'/documents/{first}'
        before_history, _ = paired('GET', route + '/history')
        before_snapshot, py_before_snapshot = paired('GET', route + '/metadata-review')
        before_document = before_snapshot['documents'][0]
        paired('PATCH', route, {'z_first': True, 'a_second': [], 'name': 7}, status=422)
        paired('PATCH', first_route, {
            'z_first': True, 'ids': [], 'a_second': {},
            'title': '', 'notes': 4, 'kind': False, 'part': '100',
            'label_status': 'other', 'document_date': '2025-02-29',
        }, status=422)
        wire = '{"z":1,"2":3,"1":4,"z":5}'
        raw_expected = python.patch(normalize(first_route), content=wire,
                                    headers={**AUTH, 'Content-Type': 'application/json'})
        assert raw_expected.status_code == 422
        assert [(entry['loc'][-1], entry['input']) for entry in raw_expected.json()['detail']] == [
            ('z', 5), ('2', 3), ('1', 4),
        ]
        assert rust('PATCH', first_route, json.loads(wire), key_source=wire, status=422) == raw_expected.json()
        paired('PATCH', first_route, {'document_date': '2025-2-29'}, status=422)
        paired('PATCH', first_route, {'document_date': '२०२५-०२-२८'}, status=422)
        paired('PATCH', first_route, {'document_date': '', 'date_status': 'unknown'}, status=422)
        paired('PATCH', first_route, {'notes': None}, status=422)
        paired('PATCH', '/documents/absent-synthetic', {'notes': None}, status=404)
        paired('PATCH', route + '/documents/batch', {
            'ids': [first, 1, {}, None], 'kind': 'wrong', 'part': 2.5, 'z': 0, 'a': 0,
        }, status=422)
        paired('PATCH', route + '/documents/batch', {'ids': [0] + [first] * 1000, 'part': 2}, status=422)
        paired('PATCH', route + '/documents/batch', {'ids': [first, first], 'kind': None}, status=422)
        paired('PATCH', route + '/documents/batch', {'ids': [first, first], 'group_key': ''}, status=422)
        paired('POST', route + '/documents/sort-preview', {
            'mode': 'wrong', 'unknown_dates': None, 'z': False, 'a': None,
        }, status=422)
        paired('PUT', route + '/metadata-review', {
            'z': True, 'a': False, 'expected_snapshot': 'stale',
            'rows': [
                {'changes': {'z': True, 'a': False, 'notes': 42, 'document_date': '2024-04-31'},
                 'last_extra': 1, 'first_extra': 2},
                {'document_id': 7, 'changes': 'not a patch'},
                {'document_id': second, 'changes': {'label_source': 'user'}},
            ],
        }, status=422)
        assert rust('GET', route + '/history') == before_history
        assert rust('GET', route + '/metadata-review') == before_snapshot

        paired('PATCH', first_route, {'part': '14.0'}, status=422)
        reviewed, _ = paired('PUT', route + '/metadata-review', {
            'expected_snapshot': before_snapshot['input_snapshot'],
            'rows': [{'document_id': first, 'changes': {'part': '14.0', 'notes': None}}],
        }, {
            'expected_snapshot': py_before_snapshot['input_snapshot'],
            'rows': [{'document_id': first, 'changes': {'part': '14.0', 'notes': None}}],
        })
        assert reviewed[0]['part'] == 14 and reviewed[0]['revision'] == 1
        paired('POST', route + '/history/undo')
        assert rust('GET', route + '/metadata-review')['documents'][0]['part'] == before_document['part']
        paired('POST', route + '/history/redo')
        assert rust('GET', route + '/metadata-review')['documents'][0]['part'] == 14
        paired('POST', route + '/history/undo')
        paired('PATCH', first_route, {
            'title': 'Synthetic group parent', 'group_key': 'Review', 'group_role': 'parent',
            'label': ' A-1 ', 'document_date': '2026-11-19', 'date_status': 'confirmed',
            'date_source': 'user', 'visible_signature_status': 'absent',
        })
        paired('PATCH', f'/documents/{second}', {'group_key': 'Review', 'group_role': 'parent'}, status=422)
        paired('PATCH', route + '/documents/batch',
            {'ids': docs, 'part': 3, 'group_key': 'Review', 'kind': 'annexure'})
        paired('POST', route + '/history/undo')
        paired('POST', route + '/history/redo')
        paired('PATCH', f'/documents/{second}', {'label': 'A-3'})
        paired('POST', route + '/documents/sort-preview', {'mode': 'filename'})
        paired('POST', route + '/documents/sort-preview', {'mode': 'document_date'})
        for prefix, tail, first_digit, last_digit, missing in (
            ('Exhibit', '', '२', '४', '3'),
            ('Large', '9' * 48, '1', '3', '9' * 48 + '2'),
        ):
            paired('PATCH', first_route, {'label': f'{prefix}-{tail}{first_digit}'})
            paired('PATCH', f'/documents/{second}', {'label': f'{prefix}-{tail}{last_digit}'})
            preview, _ = paired('POST', route + '/documents/sort-preview', {'mode': 'manual'})
            assert preview['label_review']['gaps'] == [f'{prefix.upper()}-{missing}']
            paired('POST', route + '/history/undo')
            paired('POST', route + '/history/undo')
        result, _ = paired('POST', route + '/labels/preview', {
            'ids': docs, 'mode': 'grouped_suffix', 'prefix': 'R-', 'start': 3,
            'group_suffix_style': 'upper_alpha_parenthesized', 'reference_document_id': second,
        })
        mappings = [{'document_id': row['document_id'], 'old_label': row['old_label'],
                     'new_label': row['proposed_label']} for row in result['rows']]
        apply = {'mappings': mappings, 'reference_document_id': second, 'expected_reference_revision': 1}
        paired('POST', route + '/labels/apply', apply, status=409)
        paired('POST', route + '/labels/apply', {
            **apply, 'acknowledge_existing_labels': True, 'acknowledge_references': True,
        })
        paired('POST', route + '/history/undo')
        paired('POST', route + '/history/redo')

        snapshot, py_snapshot = paired('GET', route + '/metadata-review')
        changes = {'rows': [{'document_id': second, 'changes': {'notes': 'Reviewed against synthetic fixture'}}]}
        paired('PUT', route + '/metadata-review',
               {**changes, 'expected_snapshot': snapshot['input_snapshot']},
               {**changes, 'expected_snapshot': py_snapshot['input_snapshot']})
        paired('PUT', route + '/metadata-review',
               {**changes, 'expected_snapshot': snapshot['input_snapshot']},
               {**changes, 'expected_snapshot': py_snapshot['input_snapshot']}, status=409)
        proposed, py_proposed = paired('POST', route + '/metadata-suggestions', {'ids': [first]})
        def accept(result, identifier):
            field = next(row for row in result['rows'] if row['field'] == 'title')
            return {'ids': [identifier], 'fields': [{'document_id': identifier, 'field': 'title',
                'evidence_id': field['evidence_id'], 'value': 'Reviewed title'}],
                'expected_snapshot': result['input_snapshot']}
        paired('POST', route + '/metadata-suggestions/accept',
            accept(proposed, first), accept(py_proposed, normalize(first)))
        paired('POST', route + '/history/undo')
        paired('POST', route + '/history/redo')

        for endpoint in ('thumbnail?revision=1', 'metadata-source?revision=1&page=2'):
            path = first_route + '/' + endpoint
            rendered = rust('GET', path)
            response = python.get(normalize(path), headers=AUTH)
            assert response.status_code == 200
            if endpoint.startswith('thumbnail'):
                actual_png = b64decode(rendered['png'])
                expected_png = response.content
            else:
                actual_png = b64decode(rendered['image'].split(',')[1])
                expected = response.json()
                expected_png = b64decode(expected['image'].split(',')[1])
                assert normalize({k:v for k,v in rendered.items() if k != 'image'}) == {
                    k:v for k,v in expected.items() if k != 'image'}
            actual, expected = pymupdf.Pixmap(actual_png), pymupdf.Pixmap(expected_png)
            assert (actual.width, actual.height) == (expected.width, expected.height)
            assert min(actual.samples) < 160
        paired('GET', first_route + '/thumbnail?revision=2', status=409)
        paired('GET', first_route + '/metadata-source?revision=2', status=409)

        for (path, body) in [
            ('/bookmarks', {'entries': [
                {'id': '1'*32, 'title': 'Parent', 'document_id': first, 'page_index': 0,
                 'target_revision': 1},
                {'id': '2'*32, 'title': 'Separate', 'document_id': second, 'page_index': 0,
                 'target_revision': 1},
                {'id': '3'*32, 'title': 'Child', 'document_id': first, 'page_index': 1,
                 'target_revision': 1, 'parent_id': '1'*32},
            ]}),
            ('/links', {'entries': [
                {'id': '1'*32, 'title': 'Jump', 'source_document_id': first,
                 'source_page_index': 0, 'source_revision': 1, 'rect_x': 0.2, 'rect_y': 0.3,
                 'rect_width': 0.4, 'rect_height': 0.1, 'kind': 'internal',
                 'target_document_id': second, 'target_page_index': 0, 'target_revision': 1},
            ]}),
        ]:
            entries, _ = paired('PUT', route + path, body)
            assert all(entry['resolved'] for entry in entries)
            paired('POST', route + '/history/undo')
            paired('POST', route + '/history/redo')
            paired('GET', route + path)
        paired('POST', route + '/bundle-preview', {'max_pages_per_volume': 20, 'keep_groups_intact': True})
        before, py_before = paired('GET', route + '/membership')
        after, _ = paired('PUT', route + '/membership',
            {'included_ids': [second], 'expected_snapshot': before['input_snapshot']},
            {'included_ids': [second], 'expected_snapshot': py_before['input_snapshot']})
        assert after['revision'] == 2
        assert after['included_documents'] == 1
        paired('PUT', route + '/membership',
            {'included_ids': docs, 'expected_snapshot': before['input_snapshot']},
            {'included_ids': docs, 'expected_snapshot': py_before['input_snapshot']},
            status=409)
        paired('POST', route + '/history/undo')
        paired('GET', route + '/membership')
        paired('POST', route + '/history/redo')
        paired('GET', route + '/membership')
        planning, py_planning = paired('GET', route + '/bundle-settings')
        planned = {'max_pages_per_volume': 20, 'keep_groups_intact': True, 'output_marks': {}}
        paired('POST', route + '/volume-planning',
               {**planned, 'expected_snapshot': planning['input_snapshot']},
               {**planned, 'expected_snapshot': py_planning['input_snapshot']})

        run, py_run = paired('GET', route + '/validate', compare=False)
        run_id, py_run_id = run[0]['validation_run_id'], py_run[0]['validation_run_id']
        substitutions[run_id] = py_run_id
        for finding, expected in zip(run, py_run, strict=True):
            substitutions[finding['finding_id']] = expected['finding_id']
        assert normalize(run) == normalize(py_run), (run, py_run)
        paired('GET', route + '/preflight/history')
        state, py_state = paired('GET', route + '/preflight/review')
        assert state['input_digest'] == run[0]['effective_input_digest']
        warning = next(finding for finding in run if finding['code'] == 'DOCUMENT_DATE_MISSING')
        finding_path = f"{route}/validation/findings/{run_id}/{warning['finding_id']}"
        paired('GET', finding_path)
        paired('PUT', finding_path, {'note': 'Synthetic missing-date review', 'acknowledged': True})
        paired('GET', finding_path)
        payload = {'run_id': run_id, 'version': 0,
                   'items': {item: True for item in state['items']}, 'warnings': [warning['finding_id']]}
        paired('PUT', route + '/preflight/review',
            {**payload, 'input_digest': state['input_digest']},
            {**payload, 'input_digest': py_state['input_digest']})
        paired('GET', route + '/preflight/review')
        paired('PUT', route + '/preflight/review',
            {**payload, 'input_digest': state['input_digest']},
            {**payload, 'input_digest': py_state['input_digest']}, status=409)

        def current_pdf(revision):
            rust_pdf = rust('SNAPSHOT', first_route, {'kind': 'working', 'revision': revision})
            py_pdf = python.get(normalize(first_route) + '/pdf', params={'revision': revision}, headers=AUTH)
            assert py_pdf.status_code == 200
            actual = pdf_semantics(bytes(rust_pdf['bytes']))
            expected = pdf_semantics(py_pdf.content)
            assert actual == expected, (revision, actual, expected)

        rejected_edits = [
            ('pages/rotate', {'page_index': -1, 'expected_revision': 0}),
            ('pages/delete', {'page_index': 0, 'expected_revision': True}),
            ('pages/reorder', {'order': [0, 'missing', None], 'expected_revision': 99}),
            ('pages/insert', {'source_document_id': 1, 'expected_revision': 1.0,
                              'expected_source_revision': False, 'source_pages': [], 'before_index': -1}),
            ('pages/replace', {'source_document_id': second, 'target_pages': {}, 'source_pages': []}),
            ('pages/crop', {'pages': ['bad', True, 1.5, None],
                            'left': '0.1', 'top': False, 'right': '1_0', 'bottom': 2}),
            ('pages/batch', {'z': True, 'operation': 'unknown', 'pages': [0, 0], 'degrees': 90.0, 'a': None}),
            ('pages/batch', {'expected_revision': 1, 'operation': 'rotate', 'pages': [0], 'degrees': 0}),
            ('pages/text-overlay', {'page_index': 0, 'x': 0.1, 'y': 0.1, 'text': 'Visible', 'expected_revision': 1}),
            ('pages/text-overlay', {'page_index': 0, 'x': '0.1', 'y': 0.1, 'text': 'हिंदी'}),
            ('revisions/unknown/restore', {'expected_revision': '1', 'expected_sha256': 'short'}),
        ]
        for action, payload in rejected_edits:
            paired('POST', first_route + '/' + action, payload, status=422)
        current_pdf(1)
        paired('POST', first_route + '/pages/crop', {
            'expected_revision': 1, 'pages': ['0', 0], 'left': '0.1', 'top': 0.18,
            'right': 0.85, 'bottom': '0.8',
        })
        current_pdf(2)
        paired('GET', route + '/bookmarks')
        paired('GET', route + '/links')
        paired('POST', first_route + '/pages/batch', {
            'expected_revision': 2, 'pages': [0, 1], 'operation': 'rotate', 'degrees': 90,
        })
        current_pdf(3)
        invalid = {'expected_revision': 3, 'pages': [0, 1], 'operation': 'delete'}
        paired('POST', first_route + '/pages/batch', invalid, status=422)
        current_pdf(3)
        paired('POST', first_route + '/pages/delete', {'expected_revision': 3, 'page_index': 0})
        current_pdf(4)
        for direction, revision in [('undo', 5), ('redo', 6)]:
            paired('POST', route + '/history/' + direction)
            current_pdf(revision)
        paired('POST', first_route + '/pages/delete', {'expected_revision': 6, 'page_index': 0}, status=422)

        related = {'source_document_id': second, 'expected_source_revision': 1}
        paired('POST', first_route + '/pages/insert', {
            **related, 'expected_revision': 6, 'before_index': 1, 'source_pages': [0, 1, 1],
        })
        current_pdf(7)
        paired('POST', first_route + '/pages/reorder', {
            'expected_revision': 7, 'order': [3, 2, 1, 0],
        })
        current_pdf(8)
        paired('POST', first_route + '/pages/replace', {
            **related, 'expected_revision': 8, 'target_pages': [0, 2], 'source_pages': [1, 0],
        })
        current_pdf(9)
        stale = {**related, 'expected_revision': 9, 'expected_source_revision': 2}
        paired('POST', first_route + '/replace', stale, status=409)
        paired('POST', first_route + '/replace', {**related, 'expected_revision': 9})
        current_pdf(10)
        paired('GET', first_route + '/pages/extract?acknowledge_signature_risk=maybe', status=422)
        paired('GET', first_route + '/pages/split?after=%C2%B2&acknowledge_signature_risk=YeS', status=422)
        for operation, field, selection in [('extract', 'selection', '१,٠,1'), ('split', 'after', '０')]:
            output = rust('PAGE_OUTPUT', first, operation=operation, selection=selection)
            expected = python.get(normalize(first_route) + '/pages/' + operation,
                                  params={field: selection}, headers=AUTH)
            assert expected.status_code == 200
            actual = bytes(output['bytes'])
            if operation == 'extract':
                assert pdf_semantics(actual) == pdf_semantics(expected.content)
            else:
                with ZipFile(BytesIO(actual)) as actual_zip, ZipFile(BytesIO(expected.content)) as expected_zip:
                    assert actual_zip.namelist() == expected_zip.namelist()
                    for part in actual_zip.namelist():
                        assert pdf_semantics(actual_zip.read(part)) == pdf_semantics(expected_zip.read(part))
        current_pdf(10)
        assert bytes(rust('SNAPSHOT', f'/documents/{second}', {'kind': 'original'})['bytes']) == second_source.read_bytes()
        assert bytes(rust('SNAPSHOT', f'/documents/{second}', {'kind': 'working', 'revision': 1})['bytes']) == second_source.read_bytes()
        original = rust('SNAPSHOT', first_route, {'kind': 'original'})
        assert bytes(original['bytes']) == source.read_bytes()
        compare_crop_and_binary_replacement(root, rust, python, matter['id'], reference['id'])
        compared += 6
        compare_page_form_graphs(root, rust, python, matter['id'], reference['id'])
        compared += 18
    return compared


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='matterdraft-rust-workflow-') as directory:
        print(f'{run(Path(directory))} preparation and page-edit comparisons passed')
