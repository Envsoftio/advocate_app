"""FIL-09 page-map rules for separators, Bates labels and annexure marks."""
import csv
import json

import pymupdf
import pytest

from app.engine import (bates_page_labels, bundle_preview, volume_plan,
                        volume_rows)
from app.pdf_ops import ProcessingError
from test_api import upload, wait_for_job


def confirmed_annexure(client, auth, document, label):
    response = client.patch(f'/documents/{document["id"]}', headers=auth, json={
        'kind': 'annexure', 'label': label, 'label_status': 'confirmed',
        'label_source': 'user'})
    assert response.status_code == 200, response.text
    return response.json()


def test_separator_bates_preview_csv_and_export_share_page_map(
        client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}'
    first = confirmed_annexure(client, auth,
        upload(client, auth, matter, make_pdf, name='first.pdf', pages=2), 'A-1')
    second = confirmed_annexure(client, auth,
        upload(client, auth, matter, make_pdf, name='second.pdf', pages=1), 'A-2')
    store = client.app.state.store
    source_hashes = {doc['id']: store.sha256(store.original_path(doc))
                     for doc in (first, second)}
    assert client.patch(base, headers=auth,
                        json={'max_pages_per_volume': 4}).status_code == 200
    marks = client.get(base + '/output-marks', headers=auth).json()
    marks.update(bates_enabled=True, bates_prefix='CB-', bates_start=100,
                 bates_padding=4, bates_include_index=True,
                 bates_include_separator=True, annexure_mode='separator',
                 annexure_overrides={second['id']: 'none'})
    saved = client.put(base + '/output-marks', headers=auth, json=marks)
    assert saved.status_code == 200, saved.text
    preview = client.post(base + '/bundle-preview', headers=auth, json={
        'max_pages_per_volume': 4, 'keep_groups_intact': True})
    assert preview.status_code == 200, preview.text
    volumes = preview.json()['volumes']
    assert [volume['page_count'] for volume in volumes] == [4, 2]
    assert [(row['separator_page'], row['start_page'], row['end_page'])
            for volume in volumes for row in volume['rows']] == [
                (2, 3, 4), (None, 2, 2)]
    assert [volume['bates_labels'] for volume in volumes] == [
        ['CB-0100', 'CB-0101', 'CB-0102', 'CB-0103'],
        ['CB-0104', 'CB-0105']]
    job = wait_for_job(client, auth, client.post(base + '/build', headers=auth).json()['id'])
    assert job['status'] == 'completed', job['error']
    folder = store.exports_root(matter['id']) / job['result']['id']
    with (folder / 'index.csv').open(encoding='utf-8-sig', newline='') as stream:
        csv_rows = list(csv.DictReader(stream))
    assert [(int(row['volume']), int(row['start_page']), int(row['end_page']))
            for row in csv_rows] == [(1, 3, 4), (2, 2, 2)]
    assert [row['separator_page'] for row in csv_rows] == ['2', '']
    with pymupdf.open(folder / 'combined.pdf') as pdf:
        assert pdf.page_count == preview.json()['combined_pages'] == 6
        assert [pdf[index].get_text().count(label)
                for index, label in enumerate(['CB-0100', 'CB-0101', 'CB-0102',
                                                'CB-0103', 'CB-0104', 'CB-0105'])] == [1] * 6
        assert 'A-1' in pdf[1].get_text()
        assert [link['page'] for link in pdf[0].get_links()
                if link['kind'] == pymupdf.LINK_GOTO] == [2]
        assert [link['page'] for link in pdf[4].get_links()
                if link['kind'] == pymupdf.LINK_GOTO] == [5]
        assert any(item[2] == 2 for item in pdf.get_toc() if 'separator' in item[1])
    report = json.loads((folder / 'validation-report.json').read_text())
    assert [(row['start_page'], row['separator_page']) for row in report['page_map']] == [
        (3, 2), (2, None)]
    assert all(store.sha256(store.original_path(doc)) == source_hashes[doc['id']]
               for doc in (first, second))


def test_bates_exclusions_restart_and_group_boundary_use_same_rows():
    docs = [dict(id=f'{i:032x}', title=f'Doc {i}', kind='annexure', part=1,
                 label=f'A-{i}', page_count=2, group_key='same' if i < 2 else '')
            for i in range(3)]
    settings = {'annexure_mode': 'separator', 'annexure_overrides': {},
                'bates_enabled': True, 'bates_start': 7, 'bates_padding': 3,
                'bates_prefix': 'X-', 'bates_include_index': False,
                'bates_include_separator': False, 'bates_restart_per_volume': True}
    with pytest.raises(ProcessingError, match='No document was split'):
        volume_plan(docs, 6, True, settings)
    plans = volume_plan(docs, 8, True, settings)
    assert [len(group) for group in plans] == [2, 1]
    rows, index_pages = volume_rows(plans[0], 1, settings)
    assert index_pages == 1
    assert [(row['separator_page'], row['start_page'], row['end_page'])
            for row in rows] == [(2, 3, 4), (5, 6, 7)]
    assert bates_page_labels(plans, settings) == [
        [None, None, 'X-007', 'X-008', None, 'X-009', 'X-010'],
        [None, None, 'X-007', 'X-008']]


def test_existing_cover_document_and_index_have_explicit_bates_policy():
    cover = dict(id='c' * 32, title='Technical cover', kind='other', part=1,
                 label='', page_count=1)
    annexure = dict(id='a' * 32, title='Annexure', kind='annexure', part=1,
                    label='A-1', page_count=1)
    settings = dict(annexure_mode='separator', bates_enabled=True,
                    bates_prefix='P-', bates_start=1, bates_padding=2,
                    bates_include_index=False, bates_include_separator=False)
    rows, index_pages = volume_rows([cover, annexure], 1, settings)
    assert index_pages == 1
    assert [(row['separator_page'], row['start_page']) for row in rows] == [
        (None, 2), (3, 4)]
    assert bates_page_labels([[cover, annexure]], settings) == [
        [None, 'P-01', None, 'P-02']]
    settings['bates_include_index'] = True
    settings['bates_include_separator'] = True
    assert bates_page_labels([[cover, annexure]], settings) == [
        ['P-01', 'P-02', 'P-03', 'P-04']]


def test_annexure_stamp_preview_rotated_page_and_unconfirmed_label_block(
        client, auth, matter):
    with pymupdf.open() as source:
        page = source.new_page(width=760, height=510)
        page.insert_text((80, 90), 'Rotated technical test', fontsize=14)
        page.set_rotation(90)
        second = source.new_page(width=420, height=620)
        second.insert_text((80, 90), 'Narrow technical test', fontsize=14)
        raw = source.tobytes()
    base = f'/matters/{matter["id"]}'
    uploaded = client.post(base + '/documents', headers=auth,
        files={'files': ('rotated.pdf', raw, 'application/pdf')})
    assert uploaded.status_code == 201, uploaded.text
    document = uploaded.json()[0]
    document = client.patch(f'/documents/{document["id"]}', headers=auth, json={
        'kind': 'annexure', 'label': 'R-1', 'label_status': 'unreviewed',
        'label_source': 'legacy'}).json()
    marks = client.get(base + '/output-marks', headers=auth).json()
    marks.update(annexure_mode='label_stamp', annexure_stamp_scope='all',
                 annexure_stamp_position='top_right', bates_enabled=True,
                 bates_prefix='ROT-', bates_start=9)
    assert client.put(base + '/output-marks', headers=auth, json=marks).status_code == 200
    blocked = client.post(base + '/build', headers=auth)
    assert blocked.status_code == 202
    failed = wait_for_job(client, auth, blocked.json()['id'])
    assert failed['status'] == 'failed' and 'confirmed register label' in failed['error']
    confirmed_annexure(client, auth, document, 'R-1')
    preview = client.post(
        f'/documents/{document["id"]}/pages/0/output-marks-preview',
        headers=auth, json=marks)
    assert preview.status_code == 200, preview.text
    items = preview.json()['marks']
    assert {item['kind']: item['text'] for item in items} == {
        'bates': 'ROT-000009', 'annexure_label': 'R-1'}
    assert all(0 <= number <= 1 for item in items
               for number in item['relative_rect'])
    preview_second = client.post(
        f'/documents/{document["id"]}/pages/1/output-marks-preview',
        headers=auth, json=marks)
    assert preview_second.status_code == 200, preview_second.text
    second_items = preview_second.json()['marks']
    assert {item['kind']: item['text'] for item in second_items} == {
        'bates': 'ROT-000010', 'annexure_label': 'R-1'}
    job = wait_for_job(client, auth, client.post(base + '/build', headers=auth).json()['id'])
    assert job['status'] == 'completed', job['error']
    folder = client.app.state.store.exports_root(matter['id']) / job['result']['id']
    with pymupdf.open(folder / 'combined.pdf') as output:
        assert output[1].rotation == 90
        assert output[2].rect.width == 420 and output[2].rect.height == 620
        for page_index, planned in ((1, items), (2, second_items)):
            page = output[page_index]
            for item in planned:
                rectangles = page.search_for(item['text'])
                assert rectangles
                box = item['relative_rect']
                expected = pymupdf.Rect(page.rect.x0 + box[0] * page.rect.width,
                                        page.rect.y0 + box[1] * page.rect.height,
                                        page.rect.x0 + (box[0] + box[2]) * page.rect.width,
                                        page.rect.y0 + (box[1] + box[3]) * page.rect.height)
                assert any((rect * page.rotation_matrix).intersects(expected)
                           for rect in rectangles)


def test_separator_index_growth_boundary(client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}'
    for index in range(24):
        doc = upload(client, auth, matter, make_pdf,
                     name=f'{index:02}.pdf', pages=1)
        if index == 23:
            confirmed_annexure(client, auth, doc, 'A-24')
    marks = client.get(base + '/output-marks', headers=auth).json()
    marks.update(annexure_mode='separator', bates_enabled=True,
                 bates_include_index=False, bates_include_separator=False)
    assert client.put(base + '/output-marks', headers=auth, json=marks).status_code == 200
    assert client.patch(base, headers=auth,
                        json={'max_pages_per_volume': 27}).status_code == 200
    preview = client.post(base + '/bundle-preview', headers=auth, json={
        'max_pages_per_volume': 27, 'keep_groups_intact': True}).json()
    assert len(preview['volumes']) == 1
    assert preview['volumes'][0]['index_pages'] == 2
    assert preview['volumes'][0]['page_count'] == 27
    assert preview['volumes'][0]['rows'][-1]['separator_page'] == 26
    assert preview['volumes'][0]['rows'][-1]['start_page'] == 27
    job = wait_for_job(client, auth, client.post(base + '/build', headers=auth).json()['id'])
    assert job['status'] == 'completed', job['error']
    folder = client.app.state.store.exports_root(matter['id']) / job['result']['id']
    with pymupdf.open(folder / 'combined.pdf') as output:
        assert output.page_count == 27
        assert [link['page'] for link in output[1].get_links()
                if link['kind'] == pymupdf.LINK_GOTO] == [26]


def test_matter_exceptions_stay_local_and_survive_portable_restore(
        client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}'
    first = confirmed_annexure(client, auth,
        upload(client, auth, matter, make_pdf, name='exception.pdf', pages=1), 'LOCAL-1')
    marks = client.get(base + '/output-marks', headers=auth).json()
    marks.update(annexure_mode='separator',
                 annexure_overrides={first['id']: 'label_stamp'})
    saved = client.put(base + '/output-marks', headers=auth, json=marks)
    assert saved.status_code == 200, saved.text
    rejected = client.post('/output-presets', headers=auth, json={
        'name': 'No matter IDs', 'max_pages_per_volume': 10,
        'output_marks': saved.json()})
    assert rejected.status_code == 422
    preset = client.post('/output-presets', headers=auth, json={
        'name': 'No matter IDs', 'max_pages_per_volume': 10,
        'output_marks': {**saved.json(), 'annexure_overrides': {}}})
    assert preset.status_code == 201, preset.text
    applied = client.post(base + '/output-preset/' + preset.json()['id'], headers=auth)
    assert applied.status_code == 200, applied.text
    assert client.get(base + '/output-marks', headers=auth).json()['annexure_overrides'] == {
        first['id']: 'label_stamp'}
    assert applied.json()['settings']['output_marks']['annexure_overrides'] == {}

    package = client.get(base + '/portable', headers=auth)
    restored = client.post('/matters/restore', headers=auth,
        files={'file': ('matter.cbmatter', package.content, 'application/zip')})
    assert restored.status_code == 201, restored.text
    new_id = restored.json()['id']
    restored_doc = client.get(f'/matters/{new_id}/documents', headers=auth).json()[0]
    restored_marks = client.get(f'/matters/{new_id}/output-marks', headers=auth).json()
    assert restored_marks['annexure_overrides'] == {restored_doc['id']: 'label_stamp'}

    assert client.delete(f'/documents/{first["id"]}', headers=auth).status_code == 200
    dormant = client.get(base + '/output-marks', headers=auth).json()
    dormant['bates_enabled'] = True
    updated = client.put(base + '/output-marks', headers=auth, json=dormant)
    assert updated.status_code == 200, updated.text
    assert updated.json()['annexure_overrides'] == {first['id']: 'label_stamp'}
    package = client.get(base + '/portable', headers=auth)
    archived_copy = client.post('/matters/restore', headers=auth,
        files={'file': ('archived.cbmatter', package.content, 'application/zip')})
    assert archived_copy.status_code == 201, archived_copy.text
    restored_archive = client.get(
        f'/matters/{archived_copy.json()["id"]}/archived-documents', headers=auth).json()[0]
    archived_marks = client.get(
        f'/matters/{archived_copy.json()["id"]}/output-marks', headers=auth).json()
    assert archived_marks['annexure_overrides'] == {
        restored_archive['id']: 'label_stamp'}
