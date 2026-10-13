import pymupdf
import pytest
import io
import zipfile
import time
from pathlib import Path


def upload(client, auth, matter_id, name, data):
    response = client.post(f'/matters/{matter_id}/documents', headers=auth,
                           files=[('files', (name, data, 'application/pdf'))])
    assert response.status_code == 201, response.text
    return response.json()[0]


def pages(client, auth, doc_id):
    response = client.get(f'/documents/{doc_id}/pdf', headers=auth)
    assert response.status_code == 200
    with pymupdf.open(stream=response.content, filetype='pdf') as pdf:
        return [(page.get_text().strip(), tuple(page.cropbox)) for page in pdf]


def test_insert_replace_crop_preserve_original_and_revisions(client, auth, matter, make_pdf):
    store = client.app.state.store
    target = upload(client, auth, matter['id'], 'target.pdf', make_pdf(3, 'TARGET'))
    source = upload(client, auth, matter['id'], 'source.pdf', make_pdf(2, 'SOURCE'))
    original_hash = store.sha256(store.original_path(target))
    path = f'/documents/{target["id"]}/pages'
    build = client.post(f'/matters/{matter["id"]}/build', headers=auth)
    assert build.status_code == 202, build.text
    job_id = build.json()['id']
    for _ in range(200):
        job = client.get(f'/jobs/{job_id}', headers=auth).json()
        if job['status'] != 'running' and job['status'] != 'queued':
            break
        time.sleep(.03)
    assert job['status'] == 'completed', job
    exports = f'/matters/{matter["id"]}/exports'
    assert client.get(exports, headers=auth).json()[0]['stale'] is False

    inserted = client.post(path + '/insert', headers=auth, json={
        'source_document_id': source['id'], 'source_pages': [1, 0], 'before_index': 1})
    assert inserted.status_code == 200, inserted.text
    assert inserted.json()['revision'] == 2
    assert client.get(exports, headers=auth).json()[0]['stale'] is True
    assert [text for text, _ in pages(client, auth, target['id'])] == [
        'TARGET page 1', 'SOURCE page 2', 'SOURCE page 1', 'TARGET page 2', 'TARGET page 3']

    replaced = client.post(path + '/replace', headers=auth, json={
        'source_document_id': source['id'], 'target_pages': [0, 4], 'source_pages': [0, 1]})
    assert replaced.status_code == 200, replaced.text
    assert replaced.json()['revision'] == 3
    assert [text for text, _ in pages(client, auth, target['id'])] == [
        'SOURCE page 1', 'SOURCE page 2', 'SOURCE page 1', 'TARGET page 2', 'SOURCE page 2']

    cropped = client.post(path + '/crop', headers=auth, json={
        'pages': [1, 3], 'left': .1, 'top': .1, 'right': .9, 'bottom': .9})
    assert cropped.status_code == 200, cropped.text
    assert cropped.json()['revision'] == 4
    result = pages(client, auth, target['id'])
    assert result[0][1] == (0, 0, 595, 842)
    assert result[1][1] == pytest.approx((59.5, 84.2, 535.5, 757.8), abs=.001)
    assert result[3][1] == pytest.approx((59.5, 84.2, 535.5, 757.8), abs=.001)
    assert store.sha256(store.original_path(target)) == original_hash

    history = f'/matters/{matter["id"]}/history'
    assert client.post(history + '/undo', headers=auth).status_code == 200
    assert pages(client, auth, target['id'])[1][1] == (0, 0, 595, 842)
    assert client.post(history + '/redo', headers=auth).status_code == 200
    assert pages(client, auth, target['id'])[1][1] == pytest.approx((59.5, 84.2, 535.5, 757.8), abs=.001)


def test_invalid_page_tools_leave_no_revision(client, auth, matter, make_pdf):
    target = upload(client, auth, matter['id'], 'target.pdf', make_pdf(2))
    source = upload(client, auth, matter['id'], 'source.pdf', make_pdf(1))
    path = f'/documents/{target["id"]}/pages'
    cases = [
        ('insert', {'source_document_id': source['id'], 'source_pages': [9], 'before_index': 1}),
        ('replace', {'source_document_id': source['id'], 'target_pages': [0, 1], 'source_pages': [0]}),
        ('crop', {'pages': [0], 'left': .8, 'top': 0, 'right': .2, 'bottom': 1}),
    ]
    for operation, payload in cases:
        response = client.post(path + '/' + operation, headers=auth, json=payload)
        assert response.status_code == 422, response.text
    assert client.app.state.store.get_document(target['id'])['revision'] == 1
    assert len(pages(client, auth, target['id'])) == 2


def test_extract_and_split_are_read_only_and_keep_requested_order(client, auth, matter, make_pdf):
    source = upload(client, auth, matter['id'], 'source.pdf', make_pdf(4, 'SOURCE'))
    store = client.app.state.store
    baseline = store.sha256(store.working_path(source))
    base = f'/documents/{source["id"]}/pages'
    extracted = client.get(base + '/extract?selection=3,0,3', headers=auth)
    assert extracted.status_code == 200, extracted.text
    with pymupdf.open(stream=extracted.content, filetype='pdf') as pdf:
        assert [page.get_text().strip() for page in pdf] == [
            'SOURCE page 4', 'SOURCE page 1', 'SOURCE page 4']
    split = client.get(base + '/split?after=0,2', headers=auth)
    assert split.status_code == 200, split.text
    with zipfile.ZipFile(io.BytesIO(split.content)) as archive:
        assert archive.namelist() == ['part-001.pdf', 'part-002.pdf', 'part-003.pdf']
        counts = []
        for name in archive.namelist():
            with pymupdf.open(stream=archive.read(name), filetype='pdf') as pdf:
                counts.append(len(pdf))
        assert counts == [1, 2, 1]
    assert client.get(base + '/split?after=2,0', headers=auth).status_code == 422
    assert store.sha256(store.working_path(source)) == baseline
    assert store.get_document(source['id'])['revision'] == 1


def test_append_normalized_image_page_matches_source_dimensions(client, auth, matter, make_pdf):
    target = upload(client, auth, matter['id'], 'target.pdf', make_pdf(2, 'TARGET'))
    image = Path(__file__).resolve().parents[2] / 'samples' / '04_scanned_annexure.jpg'
    response = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
                           files=[('files', (image.name, image.read_bytes(), 'image/jpeg'))])
    assert response.status_code == 201, response.text
    source = response.json()[0]
    with pymupdf.open(stream=client.get(f'/documents/{source["id"]}/pdf', headers=auth).content,
                      filetype='pdf') as pdf:
        source_rect = tuple(pdf[0].rect)
    merged = client.post(f'/documents/{target["id"]}/pages/insert', headers=auth,
                         json={'source_document_id': source['id'], 'source_pages': [0],
                               'before_index': 2})
    assert merged.status_code == 200, merged.text
    assert merged.json()['page_count'] == 3
    with pymupdf.open(stream=client.get(f'/documents/{target["id"]}/pdf', headers=auth).content,
                      filetype='pdf') as pdf:
        assert [page.get_text().strip() for page in pdf[:2]] == ['TARGET page 1', 'TARGET page 2']
        assert tuple(pdf[2].rect) == source_rect
