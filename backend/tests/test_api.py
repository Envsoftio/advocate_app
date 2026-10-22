import io
import json
import time
import zipfile
from concurrent.futures import ThreadPoolExecutor
import threading
import pymupdf
import pytest


def upload(client, auth, matter, make_pdf, name='petition.pdf', pages=2):
    r = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
                    files=[('files', (name, make_pdf(pages), 'application/pdf'))])
    assert r.status_code == 201, r.text
    return r.json()[0]


def wait_for_job(client, auth, job_id, timeout=30):
    stop = time.monotonic() + timeout
    while time.monotonic() < stop:
        r = client.get(f'/jobs/{job_id}', headers=auth)
        assert r.status_code == 200, r.text
        job = r.json()
        if job['status'] in ('completed', 'failed', 'cancelled'):
            return job
        time.sleep(.08)
    raise AssertionError('Background job did not finish')


def test_authentication_and_matter(client, auth):
    assert client.get('/matters').status_code == 401
    assert client.get('/matters', headers=auth).status_code == 200
    response = client.get('/profiles', headers=auth)
    assert response.status_code == 200
    delhi = next(profile for profile in response.json() if profile['id'] == 'delhi-hc-illustrative')
    assert delhi['effective_from'] == '2026-01-27'
    assert delhi['max_file_mb'] == 300
    assert delhi['filing_guidance']['status'] == 'SOURCE_LINKED_DRAFT_REQUIRES_ADVOCATE_REVIEW'
    assert any(template['format'] == 'XLS' for template in delhi['filing_guidance']['templates'])
    created = client.post('/matters', headers=auth, json={'name': 'Default profile example'})
    assert created.status_code == 201, created.text
    assert created.json()['court_profile'] == 'general-demo'


def test_malformed_pdf_import_leaves_no_document_or_source_file(client, auth, matter):
    path = f'/matters/{matter["id"]}/documents'
    response = client.post(path, headers=auth,
                           files=[('files', ('damaged.pdf', b'not a PDF', 'application/pdf'))])
    assert response.status_code == 422
    assert client.get(path, headers=auth).json() == []
    matter_dir = client.app.state.store.matter_dir(matter['id'])
    assert list((matter_dir / 'originals').iterdir()) == []
    assert list((matter_dir / 'working').iterdir()) == []


def test_document_page_actions(client, auth, matter, make_pdf):
    source_bytes = make_pdf(3)
    doc = upload(client, auth, matter, lambda pages: source_bytes, pages=3)
    assert doc['page_count'] == 3
    doc_id = doc['id']
    r = client.post(f'/documents/{doc_id}/pages/reorder', headers=auth, json={'order': [2,1,0]})
    assert r.status_code == 200, r.text
    reordered_at = r.json()['updated_at']
    r = client.post(f'/documents/{doc_id}/pages/rotate', headers=auth, json={'page_index':0,'degrees':90})
    assert r.status_code == 200, r.text
    assert r.json()['updated_at'] != reordered_at
    assert client.app.state.store.original_path(doc).read_bytes() == source_bytes
    pdf_response = client.get(f'/documents/{doc_id}/pdf?revision={r.json()["revision"]}', headers=auth)
    assert pdf_response.headers['cache-control'] == 'no-store'
    with pymupdf.open(stream=pdf_response.content, filetype='pdf') as rotated:
        assert rotated[0].rotation == 90
    r = client.post(f'/documents/{doc_id}/pages/delete', headers=auth, json={'page_index':0})
    assert r.json()['page_count'] == 2
    with pymupdf.open(stream=client.get(f'/documents/{doc_id}/pdf',headers=auth).content,filetype='pdf') as pdf:
        assert len(pdf)==2
    assert client.post(f'/documents/{doc_id}/pages/delete', headers=auth, json={'page_index':10}).status_code == 422


def test_document_thumbnail_is_authenticated_and_revision_bound(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf)
    url = f'/documents/{doc["id"]}/thumbnail?revision={doc["revision"]}'
    assert client.get(url).status_code == 401
    preview = client.get(url, headers=auth)
    assert preview.status_code == 200
    assert preview.headers['content-type'] == 'image/png'
    assert preview.headers['cache-control'] == 'no-store'
    image = pymupdf.Pixmap(preview.content)
    assert image.width <= 180 and image.height <= 220
    assert image.width > 0 and image.height > 0
    changed = client.post(f'/documents/{doc["id"]}/pages/rotate', headers=auth,
                          json={'page_index':0,'degrees':90})
    assert changed.status_code == 200
    assert client.get(url, headers=auth).status_code == 409


def test_undo_redo_page_metadata_order_and_branch(client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}/history'
    empty = client.get(base, headers=auth).json()
    assert empty['undo_label'] is None and empty['redo_label'] is None
    first = upload(client, auth, matter, make_pdf, name='first.pdf', pages=2)
    second = upload(client, auth, matter, make_pdf, name='second.pdf', pages=1)
    assert client.get(base, headers=auth).json()['undo_count'] == 2
    assert client.get(base, headers=auth).json()['undo_label'] == 'Add or restore document'
    rotated = client.post(f'/documents/{first["id"]}/pages/rotate', headers=auth,
                          json={'page_index': 0, 'degrees': 90}).json()
    assert rotated['revision'] == 2
    assert client.get(base, headers=auth).json()['undo_label'] == 'Working PDF edit'
    client.patch(f'/documents/{first["id"]}', headers=auth, json={'title': 'Renamed synthetic file'})
    assert client.get(base, headers=auth).json()['undo_label'] == 'Document details'
    client.post(f'/matters/{matter["id"]}/documents/reorder', headers=auth,
                json={'ids': [second['id'], first['id']]})
    ordered = client.get(base, headers=auth).json()
    assert ordered['undo_count'] == 5 and ordered['undo_label'] == 'Document order'
    undone_order = client.post(base + '/undo', headers=auth).json()
    assert undone_order['redo_count'] == 1
    assert undone_order['undo_label'] == 'Document details'
    assert undone_order['redo_label'] == 'Document order'
    assert [d['id'] for d in client.get(f'/matters/{matter["id"]}/documents', headers=auth).json()] == [first['id'], second['id']]
    undone_details = client.post(base + '/undo', headers=auth).json()
    assert undone_details['undo_label'] == 'Working PDF edit'
    assert undone_details['redo_label'] == 'Document details'
    assert client.app.state.store.get_document(first['id'])['title'] == first['title']
    undone_pdf = client.post(base + '/undo', headers=auth).json()
    assert undone_pdf['redo_label'] == 'Working PDF edit'
    restored = client.app.state.store.get_document(first['id'])
    assert restored['revision'] == 3
    with pymupdf.open(stream=client.get(f'/documents/{first["id"]}/pdf', headers=auth).content,
                      filetype='pdf') as pdf:
        assert pdf[0].rotation == 0
    redone_pdf = client.post(base + '/redo', headers=auth).json()
    assert redone_pdf['undo_label'] == 'Working PDF edit'
    assert redone_pdf['redo_label'] == 'Document details'
    with pymupdf.open(stream=client.get(f'/documents/{first["id"]}/pdf', headers=auth).content,
                      filetype='pdf') as pdf:
        assert pdf[0].rotation == 90
    assert client.app.state.store.get_document(first['id'])['revision'] == 4
    client.patch(f'/documents/{first["id"]}', headers=auth, json={'label': 'NEW'})
    branched = client.get(base, headers=auth).json()
    assert branched['can_redo'] is False and branched['redo_label'] is None
    assert branched['undo_label'] == 'Document details'
    assert client.post(base + '/redo', headers=auth).status_code == 409
    assert client.app.state.store.sha256(client.app.state.store.original_path(first)) == first['source_sha256']


def test_import_and_archive_undo_redo_preserve_files_and_order(client, auth, matter, make_pdf):
    store = client.app.state.store
    base = f'/matters/{matter["id"]}'
    history = base + '/history'
    first = upload(client, auth, matter, make_pdf, name='first.pdf', pages=1)
    client.patch(f'/documents/{first["id"]}', headers=auth, json={'title': 'Reviewed first'})
    second = upload(client, auth, matter, make_pdf, name='second.pdf', pages=1)
    assert client.get(history, headers=auth).json()['undo_count'] == 3
    assert client.post(history + '/undo', headers=auth).status_code == 200
    assert [d['id'] for d in client.get(base + '/documents', headers=auth).json()] == [first['id']]
    assert store.original_path(second).is_file()
    assert store.working_path(second).is_file()
    assert client.post(history + '/redo', headers=auth).status_code == 200
    assert [d['id'] for d in client.get(base + '/documents', headers=auth).json()] == [first['id'], second['id']]

    assert client.delete(f'/documents/{first["id"]}', headers=auth).status_code == 200
    assert [d['id'] for d in client.get(base + '/documents', headers=auth).json()] == [second['id']]
    assert client.post(history + '/undo', headers=auth).status_code == 200
    assert [d['id'] for d in client.get(base + '/documents', headers=auth).json()] == [first['id'], second['id']]
    assert store.get_document(first['id'])['title'] == 'Reviewed first'
    assert client.post(history + '/redo', headers=auth).status_code == 200
    assert store.get_document(first['id']) is None
    assert client.post(history + '/undo', headers=auth).status_code == 200
    client.patch(f'/documents/{first["id"]}', headers=auth, json={'label': 'P-1'})
    assert client.get(history, headers=auth).json()['can_redo'] is False
    assert store.sha256(store.original_path(first)) == first['source_sha256']


def test_history_restore_requires_files_and_keeps_cursor_on_failure(client, auth, matter, make_pdf):
    store = client.app.state.store
    doc = upload(client, auth, matter, make_pdf, pages=1)
    history = f'/matters/{matter["id"]}/history'
    assert client.post(history + '/undo', headers=auth).status_code == 200
    working = store.working_path(doc)
    missing = working.with_suffix('.missing')
    working.rename(missing)
    try:
        failed = client.post(history + '/redo', headers=auth)
        assert failed.status_code == 409
        assert 'files are missing' in failed.json()['detail']
        assert client.get(history, headers=auth).json()['redo_count'] == 1
        assert store.get_document(doc['id']) is None
    finally:
        missing.rename(working)
    assert client.post(history + '/redo', headers=auth).status_code == 200
    assert store.get_document(doc['id']) is not None


def test_manual_archive_restore_can_be_undone(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, pages=1)
    base = f'/matters/{matter["id"]}'
    assert client.delete(f'/documents/{doc["id"]}', headers=auth).status_code == 200
    restored = client.post(base + f'/archived-documents/{doc["id"]}/restore', headers=auth)
    assert restored.status_code == 200
    assert client.app.state.store.get_document(doc['id']) is not None
    assert client.post(base + '/history/undo', headers=auth).status_code == 200
    assert client.app.state.store.get_document(doc['id']) is None
    assert client.post(base + '/history/redo', headers=auth).status_code == 200
    assert client.app.state.store.get_document(doc['id']) is not None


def test_edit_history_survives_engine_restart(tmp_path, auth, make_pdf):
    from fastapi.testclient import TestClient
    from app.main import create_app
    root = tmp_path / 'portable-history-test'
    with TestClient(create_app(data_dir=root, token='unit-test-secret')) as first_client:
        created = first_client.post('/matters', headers=auth, json={
            'name': 'Restart history', 'court_profile': 'delhi-hc-illustrative',
            'case_type': 'Synthetic', 'max_pages_per_volume': 10}).json()
        doc = upload(first_client, auth, created, make_pdf, pages=1)
        response = first_client.patch(f'/documents/{doc["id"]}', headers=auth,
                                      json={'title': 'After restart'})
        assert response.status_code == 200
    with TestClient(create_app(data_dir=root, token='unit-test-secret')) as reopened:
        base = f'/matters/{created["id"]}/history'
        assert reopened.get(base, headers=auth).json()['can_undo']
        assert reopened.post(base + '/undo', headers=auth).status_code == 200
        assert reopened.app.state.store.get_document(doc['id'])['title'] == doc['title']
        assert reopened.post(base + '/redo', headers=auth).status_code == 200
        assert reopened.app.state.store.get_document(doc['id'])['title'] == 'After restart'


def test_startup_quarantines_uncommitted_working_revision(tmp_path, auth, make_pdf):
    from fastapi.testclient import TestClient
    from app.main import create_app
    root = tmp_path / 'orphan-revision-test'
    with TestClient(create_app(data_dir=root, token='unit-test-secret')) as first_client:
        created = first_client.post('/matters', headers=auth, json={
            'name': 'Recovery fixture', 'court_profile': 'delhi-hc-illustrative',
            'case_type': 'Synthetic', 'max_pages_per_volume': 10}).json()
        doc = upload(first_client, auth, created, make_pdf, pages=1)
        store = first_client.app.state.store
        current = first_client.post(f'/documents/{doc["id"]}/pages/rotate', headers=auth,
                                    json={'page_index': 0, 'degrees': 90}).json()
        committed = store.working_path(current)
        candidate = store.matter_dir(created['id']) / f'working/{doc["id"]}-r3.pdf'
        candidate.write_bytes(committed.read_bytes())  # Simulate a crash before the DB update.
        interrupted_edit = candidate.parent / '.pdf-edit-synthetic.pdf'
        interrupted_edit.write_bytes(b'incomplete synthetic PDF')
    with TestClient(create_app(data_dir=root, token='unit-test-secret')) as reopened:
        store = reopened.app.state.store
        assert not candidate.exists()
        assert not interrupted_edit.exists()
        quarantined = store.matter_dir(created['id']) / 'quarantine' / 'working' / candidate.name
        assert quarantined.is_file()
        assert committed.is_file()
        assert store.get_document(doc['id'])['revision'] == 2
        assert any(event['action'] == 'orphan_working_revision_quarantined'
                   for event in store.list_audit(created['id']))
        assert reopened.post(f'/matters/{created["id"]}/history/undo', headers=auth).status_code == 200


def test_undo_marks_prior_export_stale(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, pages=1)
    base = f'/matters/{matter["id"]}'
    first = wait_for_job(client, auth, client.post(base + '/build', headers=auth).json()['id'])
    assert first['status'] == 'completed'
    assert client.get(base + '/exports', headers=auth).json()[0]['stale'] is False
    client.patch(base, headers=auth, json={'max_pages_per_volume': matter['max_pages_per_volume']})
    assert client.get(base + '/exports', headers=auth).json()[0]['stale'] is False
    client.patch(f'/documents/{doc["id"]}', headers=auth, json={'title': 'Temporary title'})
    assert client.get(base + '/exports', headers=auth).json()[0]['stale'] is True
    client.post(base + '/history/undo', headers=auth)
    assert client.get(base + '/exports', headers=auth).json()[0]['stale'] is True
    second = wait_for_job(client, auth, client.post(base + '/build', headers=auth).json()['id'])
    assert second['status'] == 'completed'
    latest, prior = client.get(base + '/exports', headers=auth).json()
    assert latest['stale'] is False
    assert prior['stale'] is True


def test_working_revisions_preserve_prior_copy_and_original(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, pages=2)
    store = client.app.state.store
    before_path = store.working_path(doc)
    before_hash = store.sha256(before_path)
    original_hash = store.sha256(store.original_path(doc))
    rotated = client.post(f'/documents/{doc["id"]}/pages/rotate', headers=auth,
                          json={'page_index': 0, 'degrees': 90})
    assert rotated.status_code == 200, rotated.text
    current = rotated.json()
    assert current['revision'] == 2
    assert store.working_path(current) != before_path
    assert store.sha256(before_path) == before_hash
    assert store.sha256(store.original_path(doc)) == original_hash
    with store.connect() as con:
        revisions = con.execute('SELECT revision,working_relpath FROM document_revisions '
                                'WHERE document_id=? ORDER BY revision', (doc['id'],)).fetchall()
    assert [row['revision'] for row in revisions] == [1, 2]
    assert [row['working_relpath'] for row in revisions] == [doc['working_relpath'], current['working_relpath']]
    failed = client.post(f'/documents/{doc["id"]}/pages/delete', headers=auth,
                         json={'page_index': 99})
    assert failed.status_code == 422
    assert store.get_document(doc['id'])['revision'] == 2
    assert not (store.matter_dir(matter['id']) / f'working/{doc["id"]}-r3.pdf').exists()


def test_edit_waits_for_build_and_export_uses_consistent_inputs(client, auth, matter, make_pdf, monkeypatch):
    from app import engine
    doc = upload(client, auth, matter, make_pdf, pages=1)
    entered, release = threading.Event(), threading.Event()
    original_render = engine.render_volume

    def paused_render(*args, **kwargs):
        entered.set()
        assert release.wait(10)
        return original_render(*args, **kwargs)

    monkeypatch.setattr(engine, 'render_volume', paused_render)
    job_id = client.post(f'/matters/{matter["id"]}/build', headers=auth).json()['id']
    assert entered.wait(10)
    with ThreadPoolExecutor(max_workers=1) as executor:
        edit = executor.submit(client.post, f'/documents/{doc["id"]}/pages/rotate',
                               headers=auth, json={'page_index': 0, 'degrees': 90})
        try:
            assert not edit.done()
        finally:
            release.set()
        result = wait_for_job(client, auth, job_id)
        rotated = edit.result(timeout=10)
    assert result['status'] == 'completed', result['error']
    assert rotated.status_code == 200
    assert rotated.json()['revision'] == 2
    assert result['result']['input_revisions'][0]['revision'] == 1
    with pymupdf.open(stream=client.get(
            f'/matters/{matter["id"]}/exports/{result["result"]["id"]}/combined.pdf',
            headers=auth).content, filetype='pdf') as pdf:
        assert pdf[1].rotation == 0
    with pymupdf.open(stream=client.get(f'/documents/{doc["id"]}/pdf', headers=auth).content,
                      filetype='pdf') as pdf:
        assert pdf[0].rotation == 90


def test_large_single_volume_cancellation_leaves_no_partial_export(client, auth, matter, make_pdf):
    from app.engine import build_bundle
    from app.pdf_ops import ProcessingError

    upload(client, auth, matter, make_pdf, pages=60)
    calls = 0

    def cancelled():
        nonlocal calls
        calls += 1
        return calls >= 6

    store = client.app.state.store
    with pytest.raises(ProcessingError, match='Operation cancelled'):
        build_bundle(store, matter['id'], 'synthetic-cancel-id', lambda *_: None, cancelled)
    assert calls >= 6
    assert store.list_exports(matter['id']) == []
    assert list(store.exports_root(matter['id']).glob('.build-*')) == []
    retry = wait_for_job(client, auth, client.post(
        f'/matters/{matter["id"]}/build', headers=auth).json()['id'])
    assert retry['status'] == 'completed', retry['error']
    assert retry['result']['pages'] == 61
    with pymupdf.open(stream=client.get(
            f'/matters/{matter["id"]}/exports/{retry["result"]["id"]}/combined.pdf',
            headers=auth).content, filetype='pdf') as pdf:
        assert pdf[0].get_links()[0]['page'] == 1


def test_bundle_links_index_and_archive(client, auth, matter, make_pdf):
    a = upload(client, auth, matter, make_pdf, name='01_petition.pdf', pages=2)
    b = upload(client, auth, matter, make_pdf, name='02_affidavit.pdf', pages=2)
    r = client.patch(f'/documents/{b["id"]}', headers=auth, json={'kind':'annexure', 'label':'P-1', 'part':2})
    assert r.status_code == 200, r.text
    r = client.post(f'/matters/{matter["id"]}/build', headers=auth)
    assert r.status_code == 202, r.text
    job = wait_for_job(client, auth, r.json()['id'])
    assert job['status'] == 'completed', job['error']
    result = job['result']
    assert result['pages'] == 5
    assert result['volumes'] == 1
    eid = result['id']
    response = client.get(f'/matters/{matter["id"]}/exports/{eid}/volume-01.pdf', headers=auth)
    with pymupdf.open(stream=response.content,filetype='pdf') as pdf:
        assert pdf.page_count == 5
        assert [entry[2] for entry in pdf.get_toc() if entry[0] == 2] == [2,4]
        assert sorted(link['page'] for link in pdf[0].get_links() if link.get('kind') == pymupdf.LINK_GOTO) == [1,3]
    response = client.get(f'/matters/{matter["id"]}/exports/{eid}/combined.pdf', headers=auth)
    with pymupdf.open(stream=response.content,filetype='pdf') as pdf:
        assert len(pdf[0].get_links()) == 2
    response = client.get(f'/matters/{matter["id"]}/exports/{eid}/filing-package.zip', headers=auth)
    with zipfile.ZipFile(io.BytesIO(response.content)) as package:
        assert {'volume-01.pdf','combined.pdf','index.csv','validation-report.json','READ_ME_FIRST.txt'} <= set(package.namelist())
    assert client.get(f'/matters/{matter["id"]}/exports/{eid}/../README', headers=auth).status_code != 200


def test_source_annotations_and_links_are_reported_and_omitted(client, auth, matter):
    with pymupdf.open() as source:
        first = source.new_page()
        first.insert_text((70, 90), 'Synthetic linked source page')
        first.add_text_annot((100, 130), 'Synthetic review note')
        source.new_page().insert_text((70, 90), 'Synthetic destination page')
        source[0].insert_link({'kind': pymupdf.LINK_GOTO, 'from': pymupdf.Rect(70, 150, 210, 170),
                               'page': 1, 'to': pymupdf.Point(0, 0)})
        source[1].insert_link({'kind': pymupdf.LINK_URI, 'from': pymupdf.Rect(70, 150, 210, 170),
                               'uri': 'https://example.invalid/synthetic'})
        source_bytes = source.tobytes()

    doc = upload(client, auth, matter, lambda pages: source_bytes, name='annotated.pdf')
    path = f'/matters/{matter["id"]}'
    checks = client.get(f'{path}/validate', headers=auth).json()
    policy_checks = {c['code']: c for c in checks if c['code'].startswith('SOURCE_')}
    assert policy_checks['SOURCE_ANNOTATIONS_OMITTED']['document_id'] == doc['id']
    assert policy_checks['SOURCE_LINKS_OMITTED']['document_id'] == doc['id']
    assert '1 working-copy annotation' in policy_checks['SOURCE_ANNOTATIONS_OMITTED']['message']
    assert '2 working-copy link' in policy_checks['SOURCE_LINKS_OMITTED']['message']

    job = wait_for_job(client, auth, client.post(f'{path}/build', headers=auth).json()['id'])
    assert job['status'] == 'completed', job['error']
    eid = job['result']['id']
    assert {c['code'] for c in job['result']['checks']} >= set(policy_checks)
    store = client.app.state.store
    assert store.original_path(doc).read_bytes() == source_bytes

    for filename in ('volume-01.pdf', 'combined.pdf'):
        response = client.get(f'{path}/exports/{eid}/{filename}', headers=auth)
        with pymupdf.open(stream=response.content, filetype='pdf') as output:
            assert output.page_count == 3
            assert [link['page'] for link in output[0].get_links()] == [1]
            assert all(not list(page.annots() or ()) for page in output)
            assert not output[1].get_links() and not output[2].get_links()

    response = client.get(f'{path}/exports/{eid}/filing-package.zip', headers=auth)
    with zipfile.ZipFile(io.BytesIO(response.content)) as package:
        report = json.loads(package.read('validation-report.json'))
        assert report['interactive_content_policy'] == {
            'source_annotations': 'omitted', 'source_links': 'omitted',
            'courtbundle_image_overlays': 'flattened into source-document pages when revision anchors resolve',
            'courtbundle_highlights_comments': 'private review annotations omitted from filing-preparation PDFs',
            'courtbundle_editable_pdf_marks': 'PDF-scope text, shapes and stamps flattened; staged redactions applied to generated PDFs when revision anchors resolve',
            'reviewed_user_links': 'retained when both revision-anchored endpoints resolve',
            'generated_index_links': 'retained'}
        assert {c['code'] for c in report['checks']} >= set(policy_checks)
        readme = package.read('READ_ME_FIRST.txt').decode()
        assert 'PDF annotation objects and link actions are omitted' in readme


def test_volume_splitting_and_oversized_failure(client, auth, matter, make_pdf):
    upload(client, auth, matter, make_pdf, name='one.pdf', pages=2)
    upload(client, auth, matter, make_pdf, name='two.pdf', pages=2)
    r = client.patch(f'/matters/{matter["id"]}', headers=auth, json={'max_pages_per_volume':3})
    assert r.status_code == 200, r.text
    j = wait_for_job(client, auth, client.post(f'/matters/{matter["id"]}/build', headers=auth).json()['id'])
    assert j['status'] == 'completed', j['error']
    assert j['result']['volumes'] == 2
    r = client.patch(f'/matters/{matter["id"]}', headers=auth, json={'max_pages_per_volume':2})
    j = wait_for_job(client, auth, client.post(f'/matters/{matter["id"]}/build', headers=auth).json()['id'])
    assert j['status'] == 'failed'
    assert 'No document was split' in j['error']


def test_export_size_limit_blocks_publication_and_keeps_prior_export(client, auth, matter, make_pdf, monkeypatch):
    from app import engine

    upload(client, auth, matter, make_pdf, pages=2)
    path = f'/matters/{matter["id"]}'
    first = wait_for_job(client, auth, client.post(f'{path}/build', headers=auth).json()['id'])
    assert first['status'] == 'completed', first['error']
    prior = first['result']
    sizes = {item['name']: item['bytes'] for item in prior['files']}
    assert sizes['filing-package.zip'] > sizes['combined.pdf']

    original_load_profiles = engine.load_profiles
    def set_limit(limit_bytes):
        def limited_profiles():
            profiles = original_load_profiles()
            profiles[matter['court_profile']]['max_file_mb'] = limit_bytes / (1024 * 1024)
            return profiles
        monkeypatch.setattr(engine, 'load_profiles', limited_profiles)

    # PDF metadata can shift a few bytes between runs; choose a clear margin.
    set_limit(sizes['combined.pdf'] // 2)
    blocked_pdf = wait_for_job(client, auth, client.post(f'{path}/build', headers=auth).json()['id'])
    assert blocked_pdf['status'] == 'failed'
    assert 'combined.pdf' in blocked_pdf['error']
    assert 'No new export was published' in blocked_pdf['error']

    set_limit((sizes['combined.pdf'] + sizes['filing-package.zip']) / 2)
    blocked_zip = wait_for_job(client, auth, client.post(f'{path}/build', headers=auth).json()['id'])
    assert blocked_zip['status'] == 'failed'
    assert 'filing-package.zip' in blocked_zip['error']
    assert 'combined.pdf' not in blocked_zip['error']

    exports = client.get(f'{path}/exports', headers=auth).json()
    assert [item['id'] for item in exports] == [prior['id']]
    assert client.get(f'{path}/exports/{prior["id"]}/combined.pdf', headers=auth).status_code == 200


def test_group_contiguity(client, auth, matter, make_pdf):
    x = [upload(client,auth,matter,make_pdf,name=f'doc-{n}.pdf',pages=1) for n in range(3)]
    for doc in (x[0],x[2]):
        client.patch(f'/documents/{doc["id"]}', headers=auth, json={'group_key':'related'})
    j = wait_for_job(client, auth, client.post(f'/matters/{matter["id"]}/build', headers=auth).json()['id'])
    assert j['status']=='failed' and 'not contiguous' in j['error']


def test_cover_and_missing_label(client, auth, matter):
    r = client.post(f'/matters/{matter["id"]}/cover', headers=auth)
    assert r.status_code == 201, r.text
    checks = client.get(f'/matters/{matter["id"]}/validate', headers=auth).json()
    assert any(c['code'] == 'PROFILE_NOT_APPROVED' for c in checks)


def test_original_bytes_unchanged_after_working_page_edit(client, auth, matter, make_pdf):
    source_bytes = make_pdf(pages=2)
    doc = upload(client, auth, matter, lambda pages: source_bytes, name='immutable_source.pdf', pages=2)
    store = client.app.state.store
    original = store.original_path(store.get_document(doc['id']))
    before = store.sha256(original)
    client.post(f'/documents/{doc["id"]}/pages/rotate', headers=auth, json={'page_index':0,'degrees':90})
    assert store.sha256(original) == before
    assert original.read_bytes() == source_bytes


def test_missing_and_modified_originals_block_build_without_rewriting_hashes(client, auth, matter, make_pdf):
    missing = upload(client, auth, matter, make_pdf, name='missing.pdf')
    modified = upload(client, auth, matter, make_pdf, name='modified.pdf')
    path = f'/matters/{matter["id"]}'
    prior = wait_for_job(client, auth, client.post(f'{path}/build', headers=auth).json()['id'])
    assert prior['status'] == 'completed', prior['error']

    store = client.app.state.store
    store.original_path(missing).unlink()
    changed_path = store.original_path(modified)
    changed_path.write_bytes(changed_path.read_bytes() + b'changed after import')

    checks = client.get(f'{path}/validate', headers=auth).json()
    assert {(c['code'], c['document_id']) for c in checks if c['severity'] == 'error'} == {
        ('MISSING_ORIGINAL', missing['id']), ('ORIGINAL_HASH_MISMATCH', modified['id'])}
    blocked = wait_for_job(client, auth, client.post(f'{path}/build', headers=auth).json()['id'])
    assert blocked['status'] == 'failed'
    assert missing['id'] in blocked['error'] and modified['id'] in blocked['error']
    assert not store.original_path(missing).exists()
    assert store.sha256(changed_path) != modified['source_sha256']
    assert store.get_document(modified['id'])['source_sha256'] == modified['source_sha256']
    assert [item['id'] for item in client.get(f'{path}/exports', headers=auth).json()] == [prior['result']['id']]


def test_original_changed_during_build_blocks_publication(client, auth, matter, make_pdf, monkeypatch):
    from app import engine

    doc = upload(client, auth, matter, make_pdf)
    original = client.app.state.store.original_path(doc)
    render_volume = engine.render_volume

    def change_source_after_render(*args, **kwargs):
        result = render_volume(*args, **kwargs)
        original.write_bytes(original.read_bytes() + b'changed during build')
        return result

    monkeypatch.setattr(engine, 'render_volume', change_source_after_render)
    path = f'/matters/{matter["id"]}'
    blocked = wait_for_job(client, auth, client.post(f'{path}/build', headers=auth).json()['id'])
    assert blocked['status'] == 'failed'
    assert 'Source integrity check failed before export' in blocked['error']
    assert doc['id'] in blocked['error']
    assert client.get(f'{path}/exports', headers=auth).json() == []


def test_index_overflows_to_second_page(client, auth, matter, make_pdf):
    # This checks index reservation, 1-based ranges, links and outline destinations.
    for i in range(25):
        upload(client, auth, matter, make_pdf, name=f'item_{i:02}.pdf', pages=1)
    assert client.patch(f'/matters/{matter["id"]}', headers=auth,
                        json={'max_pages_per_volume':30}).status_code == 200
    job = wait_for_job(client, auth,
            client.post(f'/matters/{matter["id"]}/build', headers=auth).json()['id'])
    assert job['status'] == 'completed', job['error']
    assert job['result']['volumes'] == 1
    assert job['result']['pages'] == 27
    eid = job['result']['id']
    r = client.get(f'/matters/{matter["id"]}/exports/{eid}/volume-01.pdf',headers=auth)
    with pymupdf.open(stream=r.content, filetype='pdf') as pdf:
        assert len(pdf[0].get_links()) == 23
        assert len(pdf[1].get_links()) == 2
        assert pdf[1].get_links()[-1]['page'] == 26
        assert len([row for row in pdf.get_toc() if row[0] == 2]) == 25
