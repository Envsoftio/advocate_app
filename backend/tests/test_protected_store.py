import time

from fastapi.testclient import TestClient

from app.file_io import read_bytes
from app.main import create_app
from app.vault import MAGIC
from app.portable import PROTECTED_PACKAGE_MAGIC


def test_protected_matter_import_build_backup_restore_has_no_plaintext_files(tmp_path, make_pdf):
    root = tmp_path / 'protected'
    key = b'P' * 32
    auth = {'Authorization': 'Bearer protected-test'}
    source = make_pdf(2, 'CONFIDENTIAL SYNTHETIC MARKER')
    with TestClient(create_app(root, 'protected-test', require_vault=True)) as client:
        assert client.get('/vault/status', headers=auth).json()['state'] == 'setup'
        assert client.get('/matters', headers=auth).status_code == 423
        setup = client.post('/vault/setup', headers=auth,
                            json={'password': 'synthetic password 123'})
        assert setup.status_code == 200, setup.text
        recovery = setup.json()['recovery_key']
        assert len(recovery) == 64
        assert client.get('/vault/status', headers=auth).json()['state'] == 'unlocked'
        matter = client.post('/matters', headers=auth, json={
            'name': 'Synthetic protected matter', 'court_profile': 'delhi-hc-illustrative',
            'case_type': 'Synthetic', 'max_pages_per_volume': 10}).json()
        assert client.post(f'/matters/{matter["id"]}/documents?included=true', headers=auth,
            files=[('files', ('sample.pdf', source, 'application/pdf'))]).status_code == 415
        uploaded = client.post(f'/matters/{matter["id"]}/documents/raw?included=true',
            headers={**auth, 'Content-Type': 'application/octet-stream',
                     'X-CourtBundle-Filename': 'sample.pdf'}, content=source)
        assert uploaded.status_code == 201, uploaded.text
        doc = uploaded.json()[0]
        store = client.app.state.store
        assert store.sha256(store.original_path(doc)) == doc['source_sha256']
        assert read_bytes(store.original_path(doc)) == source
        pdf = client.get(f'/documents/{doc["id"]}/pdf', headers=auth)
        assert pdf.status_code == 200 and pdf.content.startswith(b'%PDF')
        rotated = client.post(f'/documents/{doc["id"]}/pages/rotate', headers=auth,
                              json={'page_index': 0, 'degrees': 90})
        assert rotated.status_code == 200, rotated.text
        start = client.post(f'/matters/{matter["id"]}/build', headers=auth)
        assert start.status_code == 202, start.text
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            job = client.get(f'/jobs/{start.json()["id"]}', headers=auth).json()
            if job['status'] in ('completed', 'failed', 'cancelled'):
                break
            time.sleep(.03)
        assert job['status'] == 'completed', job.get('error')
        export = client.get(f'/matters/{matter["id"]}/exports/{job["result"]["id"]}/combined.pdf',
                            headers=auth)
        assert export.status_code == 200 and export.content.startswith(b'%PDF')
        backup = client.get(f'/matters/{matter["id"]}/portable', headers=auth)
        assert backup.status_code == 200 and backup.content.startswith(PROTECTED_PACKAGE_MAGIC)
        assert b'CONFIDENTIAL SYNTHETIC MARKER' not in backup.content
        restored = client.post('/matters/restore', headers={**auth, 'Content-Type': 'application/octet-stream'},
            content=backup.content)
        assert restored.status_code == 201, restored.text
        assert restored.json()['id'] != matter['id']
        for path in root.rglob('*'):
            if not path.is_file() or path.name in ('.sidecar.lock', 'court_bundle.sqlite3',
                                                   'court_bundle.sqlite3-wal', 'court_bundle.sqlite3-shm',
                                                   'vault.json'):
                continue
            if path.name.startswith('.portable-'):
                raise AssertionError('Transfer scratch survived completion')
            assert path.read_bytes().startswith(MAGIC), path
            assert b'CONFIDENTIAL SYNTHETIC MARKER' not in path.read_bytes()
    with TestClient(create_app(root, 'protected-test', require_vault=True)) as reopened:
        assert reopened.get('/vault/status', headers=auth).json()['state'] == 'locked'
        assert reopened.post('/vault/unlock', headers=auth,
                             json={'credential': 'wrong password'}).status_code == 403
        unlocked = reopened.post('/vault/unlock', headers=auth,
                                 json={'credential': recovery, 'recovery': True})
        assert unlocked.status_code == 200, unlocked.text
        assert len(reopened.get('/matters', headers=auth).json()) == 2


def test_protected_backup_cross_vault_requires_source_credential(tmp_path, make_pdf):
    auth = {'Authorization': 'Bearer protected-test'}
    source_root = tmp_path / 'source'
    target_root = tmp_path / 'target'
    with TestClient(create_app(source_root, 'protected-test', require_vault=True)) as source:
        setup = source.post('/vault/setup', headers=auth,
                            json={'password': 'source synthetic password'})
        assert setup.status_code == 200, setup.text
        matter = source.post('/matters', headers=auth, json={
            'name': 'Source synthetic matter', 'court_profile': 'delhi-hc-illustrative',
            'case_type': 'Synthetic', 'max_pages_per_volume': 10}).json()
        uploaded = source.post(f'/matters/{matter["id"]}/documents/raw?included=true',
            headers={**auth, 'Content-Type': 'application/octet-stream',
                     'X-CourtBundle-Filename': 'source.pdf'}, content=make_pdf(1))
        assert uploaded.status_code == 201, uploaded.text
        backup = source.get(f'/matters/{matter["id"]}/portable', headers=auth).content
    with TestClient(create_app(target_root, 'protected-test', require_vault=True)) as target:
        assert target.post('/vault/setup', headers=auth,
                           json={'password': 'target synthetic password'}).status_code == 200
        rejected_multipart = target.post('/matters/restore', headers=auth,
            files={'file': ('backup.cbmatter', backup, 'application/octet-stream')})
        assert rejected_multipart.status_code == 415
        missing = target.post('/matters/restore', headers={**auth, 'Content-Type': 'application/octet-stream'},
            content=backup)
        assert missing.status_code == 422 and 'backup needs' in missing.text
        wrong = target.post('/matters/restore', headers={**auth, 'Content-Type': 'application/octet-stream',
            'X-CourtBundle-Backup-Credential': 'wrong password'}, content=backup)
        assert wrong.status_code == 422
        assert target.get('/matters', headers=auth).json() == []
        restored = target.post('/matters/restore', headers={**auth, 'Content-Type': 'application/octet-stream',
            'X-CourtBundle-Backup-Credential': 'source synthetic password'}, content=backup)
        assert restored.status_code == 201, restored.text
        assert target.get('/matters', headers=auth).json()[0]['name'] == 'Source synthetic matter'
