import time

import pymupdf
import pytest


def test_replace_selectable_text_preserves_original_and_exports(client, auth, matter, make_pdf):
    original = make_pdf(1, 'OLD WORDS')
    uploaded = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
                           files=[('files', ('source.pdf', original, 'application/pdf'))])
    assert uploaded.status_code == 201, uploaded.text
    doc = uploaded.json()[0]
    store = client.app.state.store
    source_hash = store.sha256(store.original_path(doc))
    base = f'/documents/{doc["id"]}'
    runs_result = client.get(base + '/pages/0/text-runs', headers=auth)
    assert runs_result.status_code == 200, runs_result.text
    runs = runs_result.json()
    assert runs['revision'] == 1
    selected = next(run for run in runs['runs'] if 'OLD WORDS' in run['text'])
    changed = client.post(base + '/pages/replace-text-run', headers=auth, json={
        'page_index': 0, 'span_index': selected['index'], 'expected_revision': 1,
        'expected_text': selected['text'], 'replacement': 'NEW page 1'})
    assert changed.status_code == 200, changed.text
    assert changed.json()['revision'] == 2
    with pymupdf.open(stream=client.get(base + '/pdf', headers=auth).content, filetype='pdf') as pdf:
        text = pdf[0].get_text()
        assert 'NEW page 1' in text
        assert 'OLD WORDS' not in text
    assert store.sha256(store.original_path(doc)) == source_hash

    build = client.post(f'/matters/{matter["id"]}/build', headers=auth)
    assert build.status_code == 202, build.text
    for _ in range(200):
        job = client.get(f'/jobs/{build.json()["id"]}', headers=auth).json()
        if job['status'] not in ('queued', 'running'):
            break
        time.sleep(.03)
    assert job['status'] == 'completed', job
    exported = client.get(f'/matters/{matter["id"]}/exports/{job["result"]["id"]}/combined.pdf', headers=auth)
    with pymupdf.open(stream=exported.content, filetype='pdf') as pdf:
        assert 'NEW page 1' in pdf[1].get_text()
        assert 'OLD WORDS' not in pdf[1].get_text()

    undone = client.post(f'/matters/{matter["id"]}/history/undo', headers=auth)
    assert undone.status_code == 200, undone.text
    with pymupdf.open(stream=client.get(base + '/pdf', headers=auth).content, filetype='pdf') as pdf:
        assert 'OLD WORDS' in pdf[0].get_text()


def test_replace_fits_longer_text_and_wraps_in_clear_space(client, auth, matter, make_pdf):
    uploaded = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
                           files=[('files', ('source.pdf', make_pdf(1, 'SHORT'), 'application/pdf'))])
    doc = uploaded.json()[0]
    base = f'/documents/{doc["id"]}'
    selected = client.get(base + '/pages/0/text-runs', headers=auth).json()['runs'][0]
    replacement = 'A MUCH LONGER REPLACEMENT THAT FITS IN FREE SPACE AND WRAPS TO ANOTHER LINE'
    result = client.post(base + '/pages/replace-text-run', headers=auth, json={
        'page_index': 0, 'span_index': selected['index'], 'expected_revision': 1,
        'expected_text': selected['text'], 'replacement': replacement})
    assert result.status_code == 200, result.text
    assert result.json()['edit_layout']['line_count'] >= 2
    with pymupdf.open(stream=client.get(base + '/pdf', headers=auth).content, filetype='pdf') as pdf:
        text = pdf[0].get_text()
        assert 'SHORT' not in text
        assert 'A MUCH LONGER REPLACEMENT' in text
        assert 'ANOTHER LINE' in text


def test_replace_rejects_stale_and_unfittable_text_without_revision(client, auth, matter, make_pdf):
    uploaded = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
                           files=[('files', ('source.pdf', make_pdf(1, 'SHORT'), 'application/pdf'))])
    doc = uploaded.json()[0]
    base = f'/documents/{doc["id"]}'
    selected = client.get(base + '/pages/0/text-runs', headers=auth).json()['runs'][0]
    payload = {'page_index': 0, 'span_index': selected['index'], 'expected_revision': 1,
               'expected_text': selected['text'], 'replacement': 'W' * 200}
    too_wide = client.post(base + '/pages/replace-text-run', headers=auth, json=payload)
    assert too_wide.status_code == 422, too_wide.text
    assert client.app.state.store.get_document(doc['id'])['revision'] == 1
    rotated = client.post(base + '/pages/rotate', headers=auth,
                          json={'page_index': 0, 'degrees': 90})
    assert rotated.status_code == 200, rotated.text
    stale = client.post(base + '/pages/replace-text-run', headers=auth, json={
        **payload, 'replacement': 'NEW'})
    assert stale.status_code == 409, stale.text
    assert client.app.state.store.get_document(doc['id'])['revision'] == 2
    assert client.get(base + '/pages/0/text-runs', headers=auth).status_code == 200


@pytest.mark.parametrize('rotation', [0, 90, 180, 270])
def test_move_text_vertically_preserves_original_and_checks_clear_space(client, auth, matter, rotation):
    with pymupdf.open() as source:
        page = source.new_page()
        page.insert_text((80, 100), 'MOVE THIS LINE', fontsize=12)
        page.insert_text((430, 100) if rotation in (90, 270) else (80, 430), 'LOWER LINE', fontsize=12)
        page.set_rotation(rotation)
        original = source.tobytes()
    uploaded = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
                           files=[('files', ('move.pdf', original, 'application/pdf'))])
    assert uploaded.status_code == 201, uploaded.text
    document = uploaded.json()[0]
    store = client.app.state.store
    source_hash = store.sha256(store.original_path(document))
    base = f'/documents/{document["id"]}'
    first = client.get(base + '/pages/0/text-runs', headers=auth).json()['runs'][0]
    offset = .04
    moved = client.post(base + '/pages/move-text-run', headers=auth, json={
        'page_index': 0, 'span_index': first['index'], 'expected_revision': 1,
        'expected_text': first['text'], 'vertical_offset': offset})
    assert moved.status_code == 200, moved.text
    after = client.get(base + '/pages/0/text-runs', headers=auth).json()['runs']
    moved_run = next(run for run in after if run['text'] == first['text'])
    assert moved_run['relative_bbox'][1] == pytest.approx(first['relative_bbox'][1] + offset, abs=.002)
    assert store.sha256(store.original_path(document)) == source_hash
    stale = client.post(base + '/pages/move-text-run', headers=auth, json={
        'page_index': 0, 'span_index': first['index'], 'expected_revision': 1,
        'expected_text': first['text'], 'vertical_offset': offset})
    assert stale.status_code == 409
    lower = next(run for run in after if run['text'] == 'LOWER LINE')
    collision_offset = lower['relative_bbox'][1] - moved_run['relative_bbox'][1]
    collision = client.post(base + '/pages/move-text-run', headers=auth, json={
        'page_index': 0, 'span_index': moved_run['index'], 'expected_revision': 2,
        'expected_text': moved_run['text'], 'vertical_offset': collision_offset})
    assert collision.status_code == 422
    assert store.get_document(document['id'])['revision'] == 2
    undone = client.post(f'/matters/{matter["id"]}/history/undo', headers=auth)
    assert undone.status_code == 200
    restored = client.get(base + '/pages/0/text-runs', headers=auth).json()['runs'][0]
    assert restored['relative_bbox'] == pytest.approx(first['relative_bbox'], abs=.002)


@pytest.mark.parametrize('rotation', [90, 180, 270])
def test_imported_rotated_page_text_can_be_selected_and_replaced(
        client, auth, matter, make_pdf, rotation):
    with pymupdf.open(stream=make_pdf(1, 'ROTATED'), filetype='pdf') as source:
        source[0].set_rotation(rotation)
        original = source.tobytes()
    uploaded = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
                           files=[('files', ('rotated.pdf', original, 'application/pdf'))])
    assert uploaded.status_code == 201, uploaded.text
    doc = uploaded.json()[0]
    store = client.app.state.store
    original_hash = store.sha256(store.original_path(doc))
    base = f'/documents/{doc["id"]}'
    result = client.get(base + '/pages/0/text-runs', headers=auth)
    assert result.status_code == 200, result.text
    selected = result.json()['runs'][0]
    with pymupdf.open(stream=client.get(base + '/pdf', headers=auth).content, filetype='pdf') as pdf:
        page = pdf[0]
        assert page.rotation == rotation
        displayed = pymupdf.Rect(selected['bbox']) * page.rotation_matrix
        expected = (displayed.x0 / page.rect.width, displayed.y0 / page.rect.height,
                    displayed.x1 / page.rect.width, displayed.y1 / page.rect.height)
        assert selected['relative_bbox'] == pytest.approx(expected, abs=.0001)
    changed = client.post(base + '/pages/replace-text-run', headers=auth, json={
        'page_index': 0, 'span_index': selected['index'], 'expected_revision': 1,
        'expected_text': selected['text'], 'replacement': 'EDITED page 1'})
    assert changed.status_code == 200, changed.text
    with pymupdf.open(stream=client.get(base + '/pdf', headers=auth).content, filetype='pdf') as pdf:
        page = pdf[0]
        assert page.rotation == rotation
        assert 'EDITED page 1' in page.get_text()
        assert 'ROTATED' not in page.get_text()
    assert store.sha256(store.original_path(doc)) == original_hash
