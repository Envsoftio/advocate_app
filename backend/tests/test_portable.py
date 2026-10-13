import io
import json
import os
import zipfile


def _matter(client, auth):
    response = client.post('/matters', headers=auth, json={
        'name': 'Portable synthetic matter', 'court_profile': 'delhi-hc-illustrative',
        'case_type': 'Synthetic', 'max_pages_per_volume': 10})
    assert response.status_code == 201
    return response.json()


def _package_with_manifest_change(raw: bytes, change):
    source = zipfile.ZipFile(io.BytesIO(raw))
    manifest = json.loads(source.read('manifest.json'))
    change(manifest)
    output = io.BytesIO()
    with zipfile.ZipFile(output, 'w') as target:
        for entry in source.infolist():
            data = json.dumps(manifest).encode() if entry.filename == 'manifest.json' else source.read(entry)
            target.writestr(entry.filename, data)
    return output.getvalue()


def test_portable_matter_round_trip_preserves_originals_and_revisions(client, auth, make_pdf):
    matter = _matter(client, auth)
    original = make_pdf(2)
    uploaded = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
        files=[('files', ('synthetic.pdf', original, 'application/pdf'))])
    assert uploaded.status_code == 201
    doc = uploaded.json()[0]
    rotated = client.post(f'/documents/{doc["id"]}/pages/rotate', headers=auth,
                          json={'page_index': 0, 'degrees': 90})
    assert rotated.status_code == 200
    backup = client.get(f'/matters/{matter["id"]}/portable', headers=auth)
    assert backup.status_code == 200, backup.text
    restored = client.post('/matters/restore', headers=auth,
        files={'file': ('backup.cbmatter', backup.content, 'application/zip')})
    assert restored.status_code == 201, restored.text
    new_matter = restored.json()
    assert new_matter['id'] != matter['id']
    new_doc = client.get(f'/matters/{new_matter["id"]}/documents', headers=auth).json()[0]
    assert new_doc['id'] != doc['id']
    assert new_doc['revision'] == 2
    store = client.app.state.store
    assert store.original_path(new_doc).read_bytes() == original
    assert store.sha256(store.working_path(new_doc)) == store.sha256(store.working_path(rotated.json()))
    with store.connect() as con:
        assert con.execute('SELECT COUNT(*) FROM document_revisions WHERE document_id=?',
                           (new_doc['id'],)).fetchone()[0] == 2
    assert list(store.root.glob('.portable-*.cbmatter*')) == []


def test_startup_cleans_interrupted_portable_files_and_restricts_store(tmp_path):
    from fastapi.testclient import TestClient
    from app.main import create_app

    root = tmp_path / 'matter-store'
    root.mkdir(mode=0o755)
    abandoned = root / '.portable-abandoned.cbmatter'
    partial = root / '.portable-abandoned.cbmatter.partial-123'
    unrelated = root / 'user-backup.cbmatter'
    for path in (abandoned, partial, unrelated):
        path.write_bytes(b'synthetic package bytes')
    with TestClient(create_app(data_dir=root, token='unit-test-secret')):
        assert not abandoned.exists()
        assert not partial.exists()
        assert unrelated.read_bytes() == b'synthetic package bytes'
        if os.name != 'nt':
            assert root.stat().st_mode & 0o777 == 0o700


def test_portable_restore_rejects_unsafe_corrupt_and_future_packages(client, auth, make_pdf):
    matter = _matter(client, auth)
    client.post(f'/matters/{matter["id"]}/documents', headers=auth,
        files=[('files', ('synthetic.pdf', make_pdf(1), 'application/pdf'))])
    raw = client.get(f'/matters/{matter["id"]}/portable', headers=auth).content
    variants = [
        _package_with_manifest_change(raw, lambda m: m.update(version=99)),
        _package_with_manifest_change(raw, lambda m: m['files'][0].update(path='../escape.pdf')),
        _package_with_manifest_change(raw, lambda m: m['files'][0].update(sha256='0' * 64)),
        _package_with_manifest_change(raw, lambda m: m['matter'].update(court_profile='unreviewed-missing-profile')),
    ]
    for package in variants:
        result = client.post('/matters/restore', headers=auth,
            files={'file': ('bad.cbmatter', package, 'application/zip')})
        assert result.status_code == 422, result.text
    assert len(client.get('/matters', headers=auth).json()) == 1
