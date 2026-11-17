import hashlib
from test_api import upload
from test_build_workflow import start
from app.storage import Store


def test_retained_restore_is_new_revision_undoable_and_preserves_outputs(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, pages=3)
    root = f'/documents/{doc["id"]}'
    build = start(client, auth, matter)
    assert build['status'] == 'completed', build
    output = f'/matters/{matter["id"]}/exports/{build["id"]}/combined.pdf'
    prior = client.get(output, headers=auth).content
    original = client.get(root + '/original', headers=auth).content
    r = client.post(root + '/pages/reorder', headers=auth, json={'order':[2,0,1]})
    assert r.status_code == 200, r.text
    second = client.get(root + '/pdf', headers=auth).content
    rows = client.get(root + '/revisions', headers=auth).json()
    assert [r['revision'] for r in rows] == [2,1]
    old = rows[1]
    historic = root + f'/revisions/1/pdf?sha256={old["sha256"]}'
    before = client.get(historic, headers=auth).content
    assert hashlib.sha256(before).hexdigest() == old['sha256']
    restored = client.post(root + '/revisions/1/restore', headers=auth, json={
        'expected_revision':2, 'expected_sha256':old['sha256']})
    assert restored.status_code == 200, restored.text
    assert restored.json()['revision'] == 3
    assert client.get(root + '/pdf', headers=auth).content == before
    assert client.get(output, headers=auth).content == prior
    assert client.get(root + '/original', headers=auth).content == original
    assert client.get(f'/matters/{matter["id"]}/exports', headers=auth).json()[0]['stale']
    undo = client.post(f'/matters/{matter["id"]}/history/undo', headers=auth)
    assert undo.status_code == 200, undo.text
    assert client.get(root + '/pdf', headers=auth).content == second
    assert client.get(historic, headers=auth).content == before


def test_revision_conflicts_missing_and_tampered_preview_fail_closed(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf)
    root = f'/documents/{doc["id"]}'
    row = client.get(root + '/revisions', headers=auth).json()[0]
    payload = {'expected_revision':1,'expected_sha256':row['sha256']}
    assert client.post(root+'/revisions/1/restore', headers=auth, json=payload).status_code == 409
    client.post(root+'/pages/reorder',headers=auth,json={'order':[1,0]})
    assert client.post(root+'/revisions/1/restore',headers=auth,json=payload).status_code == 409
    payload['expected_revision'] = 2
    payload['expected_sha256'] = '0'*64
    assert client.post(root+'/revisions/1/restore',headers=auth,json=payload).status_code == 409
    assert client.get(root+'/revisions/1/pdf?sha256='+'0'*64,headers=auth).status_code == 409
    assert client.get(root+'/revisions/999/pdf?sha256='+row['sha256'],headers=auth).status_code == 409
    assert client.get(root+'/revisions').status_code == 401
    assert client.get(root+'/pdf?revision=2',headers=auth).status_code == 200


def test_archive_persists_without_staling_or_rewriting_artifacts(client, auth, matter, make_pdf):
    upload(client, auth, matter, make_pdf)
    build = start(client, auth, matter)
    assert build['status'] == 'completed', build
    root = f'/matters/{matter["id"]}/exports'
    url = f'{root}/{build["id"]}'
    before = client.get(url+'/combined.pdf',headers=auth).content
    result = client.put(url+'/archive',headers=auth,json={'archived':True})
    assert result.status_code == 200, result.text
    assert result.json()['archived_at'] and not result.json()['stale']
    assert client.get(url+'/combined.pdf',headers=auth).content == before
    status = client.get(url+'/validation/state',headers=auth).json()
    assert status['can_download'] and not status['can_release']
    assert any('archived' in reason for reason in status['release_reasons'])
    other = client.post('/matters',headers=auth,json={'name':'Other synthetic matter'}).json()
    assert client.put(f'/matters/{other["id"]}/exports/{build["id"]}/archive',headers=auth,json={'archived':True}).status_code == 404
    assert client.put(url+'/archive',headers=auth,json={'archived':False}).json()['archived_at'] is None
    assert not client.get(root,headers=auth).json()[0]['stale']
    client.put(url+'/archive',headers=auth,json={'archived':True})
    data = client.app.state.store.root
    client.app.state.store.close()
    reopened = Store(data)
    try:
        assert reopened.list_exports(matter['id'])[0]['archived_at']
        assert reopened.get_export_file(matter['id'],build['id'],'combined.pdf').read_bytes() == before
    finally: reopened.close()


def test_comparison_uses_frozen_map_and_independent_pages(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, pages=3)
    build = start(client, auth, matter)
    assert build['status'] == 'completed', build
    url = f'/matters/{matter["id"]}/exports/{build["id"]}/compare-pages'
    client.post(f'/documents/{doc["id"]}/pages/reorder',headers=auth,json={'order':[2,0,1]})
    payload = {'document_id':doc['id'],'expected_revision':2,'prior_source_page':1,'current_source_page':2}
    result = client.post(url,headers=auth,json=payload)
    assert result.status_code == 200, result.text
    data = result.json()
    assert data['prior_physical_page'] == 2 and data['current_source_page'] == 2
    assert data['prior_source_ranges'] == [{'first':1,'last':3}]
    assert data['differences']  # generated page numbering is honestly included
    filtered = client.post(url,headers=auth,json={**payload,'ignore_generated_numbering':True}).json()
    assert filtered['numbering_removed']
    assert not filtered['differences']
    assert not filtered['rendered_equal']  # rendering remains the actual saved PDF
    assert client.post(url,headers=auth,json={**payload,'expected_revision':1}).status_code == 409
    assert client.post(url,headers=auth,json={**payload,'prior_source_page':9}).status_code == 409
    assert client.post(url,headers=auth,json={**payload,'current_source_page':9}).status_code == 409


def test_index_destinations_follow_frozen_cover_and_volume_offsets():
    from app.engine import generated_index_destinations
    rows = [{'volume':1,'start_page':3,'combined_start_page':3},
            {'volume':2,'start_page':3,'combined_start_page':10}]
    assert generated_index_destinations(rows, {'front_matter':'cover_index'}) == {
        'combined.pdf':2, 'volume-01.pdf':2, 'volume-02.pdf':2}
    assert generated_index_destinations(rows, {'front_matter':'index_only'})['combined.pdf'] == 1
    assert generated_index_destinations([], {}) == {}


def test_restore_failure_does_not_publish_candidate(client, auth, matter, make_pdf, monkeypatch):
    doc = upload(client, auth, matter, make_pdf)
    root = f'/documents/{doc["id"]}'
    old = client.get(root+'/revisions',headers=auth).json()[0]
    client.post(root+'/pages/reorder',headers=auth,json={'order':[1,0]})
    store = client.app.state.store
    current = client.get(root+'/pdf',headers=auth).content
    def fail(*args, **kwargs): raise OSError('Synthetic commit failure')
    monkeypatch.setattr(store, 'update_document', fail)
    result = client.post(root+'/revisions/1/restore',headers=auth,json={'expected_revision':2,'expected_sha256':old['sha256']})
    assert result.status_code == 409
    assert store.get_document(doc['id'])['revision'] == 2
    assert client.get(root+'/pdf',headers=auth).content == current
    assert not (store.matter_dir(matter['id']) / 'working' / f'{doc["id"]}-r3.pdf').exists()


def test_revision_and_archive_in_encrypted_store(tmp_path, make_pdf):
    from fastapi.testclient import TestClient
    from app.main import create_app
    from app.vault import MAGIC
    auth = {'Authorization':'Bearer protected-review'}
    with TestClient(create_app(tmp_path/'encrypted', 'protected-review', require_vault=True)) as client:
        assert client.post('/vault/setup',headers=auth,json={'password':'synthetic password 123'}).status_code == 200
        matter = client.post('/matters',headers=auth,json={'name':'Protected revision review'}).json()
        uploaded = client.post(f'/matters/{matter["id"]}/documents/raw?included=true',headers={**auth,'Content-Type':'application/octet-stream','X-CourtBundle-Filename':'sample.pdf'},content=make_pdf(2))
        doc = uploaded.json()[0]
        root = f'/documents/{doc["id"]}'
        build = start(client,auth,matter)
        assert build['status'] == 'completed', build
        old = client.get(root+'/revisions',headers=auth).json()[0]
        client.post(root+'/pages/reorder',headers=auth,json={'order':[1,0]})
        restored = client.post(root+'/revisions/1/restore',headers=auth,json={'expected_revision':2,'expected_sha256':old['sha256']})
        assert restored.status_code == 200, restored.text
        store = client.app.state.store
        assert store.working_path(store.get_document(doc['id'])).read_bytes().startswith(MAGIC)
        assert client.put(f'/matters/{matter["id"]}/exports/{build["id"]}/archive',headers=auth,json={'archived':True}).json()['archived_at']
        assert not (store.root/'court_bundle.sqlite3').read_bytes().startswith(b'SQLite format 3')
