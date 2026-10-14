import pymupdf

from test_api import upload, wait_for_job


def test_replace_entire_working_document_undo_redo(client, auth, matter, make_pdf):
    store = client.app.state.store
    target = upload(client, auth, matter, lambda pages: make_pdf(2, 'Target'),
                    name='target.pdf')
    source = upload(client, auth, matter, lambda pages: make_pdf(3, 'Source'),
                    name='source.pdf')
    original = store.original_path(target).read_bytes()
    route = f'/documents/{target["id"]}/replace'
    replaced = client.post(route, headers=auth,
                           json={'source_document_id': source['id']})
    assert replaced.status_code == 200, replaced.text
    assert replaced.json()['page_count'] == 3
    assert replaced.json()['revision'] == 2
    assert store.original_path(target).read_bytes() == original
    with pymupdf.open(store.working_path(replaced.json())) as pdf:
        assert 'Source' in pdf[0].get_text()
    history = f'/matters/{matter["id"]}/history'
    assert client.post(history + '/undo', headers=auth).status_code == 200
    undone = store.get_document(target['id'])
    assert undone['page_count'] == 2
    with pymupdf.open(store.working_path(undone)) as pdf:
        assert 'Target' in pdf[0].get_text()
    assert client.post(history + '/redo', headers=auth).status_code == 200
    assert store.get_document(target['id'])['page_count'] == 3
    assert client.post(route, headers=auth,
                       json={'source_document_id': target['id']}).status_code == 422


def test_export_fingerprint_marks_replaced_inputs_stale(client, auth, matter, make_pdf):
    first = upload(client, auth, matter, lambda pages: make_pdf(2, 'First'),
                   name='first.pdf')
    second = upload(client, auth, matter, lambda pages: make_pdf(1, 'Second'),
                    name='second.pdf')
    start = client.post(f'/matters/{matter["id"]}/build', headers=auth)
    assert start.status_code == 202, start.text
    job = wait_for_job(client, auth, start.json()['id'])
    assert job['status'] == 'completed', job
    exports = f'/matters/{matter["id"]}/exports'
    initial = client.get(exports, headers=auth).json()[0]
    assert initial['build_input_digest']
    assert initial['stale'] is False
    changed = client.post(f'/documents/{first["id"]}/replace', headers=auth,
                          json={'source_document_id': second['id']})
    assert changed.status_code == 200, changed.text
    assert client.get(exports, headers=auth).json()[0]['stale'] is True
    assert client.post(f'/matters/{matter["id"]}/history/undo', headers=auth).status_code == 200
    # Undo creates a new revision, so the earlier package stays traceably stale.
    assert client.get(exports, headers=auth).json()[0]['stale'] is True


def test_export_fingerprint_detects_external_original_change(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, pages=1)
    start = client.post(f'/matters/{matter["id"]}/build', headers=auth)
    assert wait_for_job(client, auth, start.json()['id'])['status'] == 'completed'
    exports = f'/matters/{matter["id"]}/exports'
    assert client.get(exports, headers=auth).json()[0]['stale'] is False
    original = client.app.state.store.original_path(doc)
    preserved = original.read_bytes()
    try:
        original.write_bytes(preserved + b'externally changed')
        assert client.get(exports, headers=auth).json()[0]['stale'] is True
    finally:
        original.write_bytes(preserved)
