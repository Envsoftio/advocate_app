"""Durable bundle inputs and structured recovery, using real assembled PDFs."""
import hashlib
import pymupdf
import pytest
from app import build_inputs
from app.pdf_ops import ProcessingError
from test_api import upload, wait_for_job


def submit(client, auth, matter):
    response = client.post(f'/matters/{matter["id"]}/build', headers=auth)
    assert response.status_code == 202, response.text
    return wait_for_job(client, auth, response.json()['id'])


def test_retry_uses_saved_revisions_settings_and_counts(client, auth, matter, make_pdf, monkeypatch):
    import app.engine as engine
    doc = upload(client, auth, matter, make_pdf, pages=3)
    original = client.get(f'/documents/{doc["id"]}/original', headers=auth).content
    actual = engine.render_volume
    def unavailable(*args, **kwargs):
        raise ProcessingError('Synthetic transient output failure')
    monkeypatch.setattr(engine, 'render_volume', unavailable)
    first = submit(client, auth, matter)
    assert first['status'] == 'failed'
    context = first['input_context']
    assert (context['included_documents'], context['source_pages'], context['generated_pages'], context['physical_pages'], context['volumes']) == (1, 3, 1, 4, 1)
    assert first['events'] and all(event['at'] for event in first['events'])
    assert client.patch(f'/documents/{doc["id"]}', headers=auth, json={'title': 'Changed current title'}).is_success
    assert client.post(f'/documents/{doc["id"]}/pages/rotate', headers=auth,
        json={'page_index': 0, 'degrees': 90, 'expected_revision': 1}).is_success
    assert client.patch(f'/matters/{matter["id"]}', headers=auth, json={'max_pages_per_volume': 2}).is_success
    monkeypatch.setattr(engine, 'render_volume', actual)
    response = client.post(f'/jobs/{first["id"]}/retry', headers=auth)
    assert response.status_code == 202, response.text
    retried = wait_for_job(client, auth, response.json()['id'])
    assert retried['status'] == 'completed', retried
    assert retried['input_context'] == context
    assert retried['result']['input_snapshot_id'] == context['snapshot_id']
    assert retried['result']['input_revisions'][0]['revision'] == 1
    assert client.post(f'/jobs/{first["id"]}/retry', headers=auth).status_code == 409
    exported = client.get(f'/matters/{matter["id"]}/exports', headers=auth).json()[0]
    assert exported['stale']
    data = client.app.state.store.get_export_file(matter['id'], retried['id'], 'combined.pdf').read_bytes()
    with pymupdf.open(stream=data, filetype='pdf') as pdf:
        assert pdf.page_count == 4
        assert pdf[1].rotation == 0
        assert 'Changed current title' not in pdf[0].get_text()
    assert client.get(f'/documents/{doc["id"]}/original', headers=auth).content == original
    assert submit(client, auth, matter)['status'] == 'failed'
    assert hashlib.sha256(data).hexdigest() == next(f['sha256'] for f in exported['files'] if f['name'] == 'combined.pdf')


def test_snapshot_restart_integrity_and_scoped_cleanup(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf)
    live = client.app.state.store
    with live.matter_lock(matter['id']):
        context = build_inputs.capture(live, matter['id'])
    root, _ = build_inputs.load(live, matter['id'], context)
    for _ in range(2):
        frozen = build_inputs.FrozenStore(live, root)
        assert len(frozen.get_documents(matter['id'])) == 1
        frozen.close()
    live.save_job(dict(id='retained', matter_id=matter['id'], kind='build', status='interrupted', progress=0,
        message='Restart', input_context=context, result=None))
    build_inputs.cleanup(live, matter['id'])
    assert root.exists()
    frozen_path = root / 'matters' / matter['id'] / doc['working_relpath']
    frozen_path.write_bytes(b'tampered snapshot')
    with pytest.raises(ProcessingError, match='integrity'):
        build_inputs.run(live, matter['id'], context, 'a'*32, lambda *args: None, lambda: False, False)
    assert not live.exports_root(matter['id']).joinpath('a'*32).exists()
    assert client.get(f'/documents/{doc["id"]}/pdf', headers=auth).status_code == 200


def test_failed_preflight_reports_exact_revision_and_source_page(client, auth, matter, make_pdf, monkeypatch):
    doc = upload(client, auth, matter, make_pdf)
    actual = pymupdf.Page.get_pixmap
    def failed(page, *args, **kwargs):
        if page.number == 1:
            raise RuntimeError('Synthetic source rendering failure')
        return actual(page, *args, **kwargs)
    monkeypatch.setattr(pymupdf.Page, 'get_pixmap', failed)
    job = submit(client, auth, matter)
    assert job['status'] == 'failed', job
    failure = next(f for f in job['result']['failures'] if f['code'] == 'PAGE_RENDER_FAILED')
    assert (failure['document_id'], failure['target_revision'], failure['page_index']) == (doc['id'], 1, 1)
    assert client.get(f'/matters/{matter["id"]}/exports', headers=auth).json() == []


@pytest.mark.parametrize('protection', ['plain', 'encrypted', 'migrate'])
def test_snapshot_retry_survives_engine_restart_and_vault_boundary(tmp_path, make_pdf, monkeypatch, protection):
    import os
    from fastapi.testclient import TestClient
    from app.main import create_app
    from app.vault_store import VaultStore
    from app.vault import MAGIC
    import app.engine as engine
    headers = {'Authorization': 'Bearer snapshot-test'}
    key = os.urandom(32) if protection == 'encrypted' else None
    actual = engine.render_volume
    def fail(*args, **kwargs):
        raise ProcessingError('Synthetic transient failure before restart')
    monkeypatch.setattr(engine, 'render_volume', fail)
    with TestClient(create_app(tmp_path, token='snapshot-test', data_key=key, require_vault=False)) as client:
        matter = client.post('/matters', headers=headers, json={'name':'Synthetic frozen private title', 'court_profile':'general-demo'}).json()
        upload(client, headers, matter, make_pdf, pages=2)
        first = submit(client, headers, matter)
        assert first['status'] == 'failed'
        context = first['input_context']
        snapshot = build_inputs.root_for(client.app.state.store, matter['id'], context['snapshot_id'])
    if protection == 'migrate':
        key, _ = VaultStore(tmp_path).setup('synthetic migration password')
    if key:
        assert not (snapshot / 'court_bundle.sqlite3').read_bytes().startswith(b'SQLite format 3')
        assert (snapshot / 'snapshot.json').read_bytes().startswith(MAGIC)
        assert b'Synthetic frozen private title' not in (snapshot / 'snapshot.json').read_bytes()
        assert all(p.read_bytes().startswith(MAGIC) for p in snapshot.rglob('*.pdf'))
    monkeypatch.setattr(engine, 'render_volume', actual)
    with TestClient(create_app(tmp_path, token='snapshot-test', data_key=key, require_vault=False)) as reopened:
        job = reopened.get(f'/jobs/{first["id"]}', headers=headers).json()
        assert job['input_context'] == context
        response = reopened.post(f'/jobs/{first["id"]}/retry', headers=headers)
        assert response.status_code == 202, response.text
        done = wait_for_job(reopened, headers, response.json()['id'])
        assert done['status'] == 'completed', done
        assert done['result']['pages'] == 3
        assert done['result']['input_snapshot_id'] == context['snapshot_id']
        assert reopened.post(f'/jobs/{first["id"]}/retry', headers=headers).status_code == 409
