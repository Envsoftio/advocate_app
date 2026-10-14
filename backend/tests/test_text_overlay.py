import time

import pymupdf


def test_text_overlay_is_visible_searchable_undoable_and_exported(client, auth, matter, make_pdf):
    source = make_pdf(1, 'ORIGINAL')
    response = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
                           files=[('files', ('source.pdf', source, 'application/pdf'))])
    assert response.status_code == 201, response.text
    doc = response.json()[0]
    store = client.app.state.store
    original_hash = store.sha256(store.original_path(doc))
    url = f'/documents/{doc["id"]}/pages/text-overlay'
    added = client.post(url, headers=auth, json={
        'page_index': 0, 'x': .15, 'y': .18, 'text': 'REVIEW COPY', 'font_size': 12})
    assert added.status_code == 200, added.text
    assert added.json()['revision'] == 2
    working = client.get(f'/documents/{doc["id"]}/pdf', headers=auth)
    with pymupdf.open(stream=working.content, filetype='pdf') as pdf:
        assert 'REVIEW COPY' in pdf[0].get_text()
    assert store.sha256(store.original_path(doc)) == original_hash

    build = client.post(f'/matters/{matter["id"]}/build', headers=auth)
    assert build.status_code == 202, build.text
    for _ in range(200):
        job = client.get(f'/jobs/{build.json()["id"]}', headers=auth).json()
        if job['status'] not in ('queued', 'running'):
            break
        time.sleep(.03)
    assert job['status'] == 'completed', job
    export_id = job['result']['id']
    exported = client.get(f'/matters/{matter["id"]}/exports/{export_id}/combined.pdf', headers=auth)
    assert exported.status_code == 200, exported.text
    with pymupdf.open(stream=exported.content, filetype='pdf') as pdf:
        assert 'REVIEW COPY' in pdf[1].get_text()

    assert client.post(f'/matters/{matter["id"]}/history/undo', headers=auth).status_code == 200
    undone = client.get(f'/documents/{doc["id"]}/pdf', headers=auth)
    with pymupdf.open(stream=undone.content, filetype='pdf') as pdf:
        assert 'REVIEW COPY' not in pdf[0].get_text()


def test_invalid_overlay_does_not_publish_revision(client, auth, matter, make_pdf):
    response = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
                           files=[('files', ('source.pdf', make_pdf(1), 'application/pdf'))])
    doc = response.json()[0]
    url = f'/documents/{doc["id"]}/pages/text-overlay'
    for payload in (
        {'page_index': 0, 'x': .99, 'y': .5, 'text': 'EDGE', 'font_size': 12},
        {'page_index': 0, 'x': .2, 'y': .2, 'text': 'हिंदी', 'font_size': 12},
        {'page_index': 8, 'x': .2, 'y': .2, 'text': 'TEST', 'font_size': 12},
    ):
        result = client.post(url, headers=auth, json=payload)
        assert result.status_code == 422, result.text
    assert client.app.state.store.get_document(doc['id'])['revision'] == 1
    rotated = client.post(f'/documents/{doc["id"]}/pages/rotate', headers=auth,
                          json={'page_index': 0, 'degrees': 90})
    assert rotated.status_code == 200, rotated.text
    accepted = client.post(url, headers=auth, json={
        'page_index': 0, 'x': .2, 'y': .2, 'text': 'TEST', 'font_size': 12})
    assert accepted.status_code == 200, accepted.text
    assert client.app.state.store.get_document(doc['id'])['revision'] == 3
