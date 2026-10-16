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
    dated = client.patch(f'/documents/{doc["id"]}', headers=auth, json={
        'document_date': '2026-10-16', 'date_status': 'confirmed', 'date_source': 'user',
        'label': 'P-7', 'label_status': 'confirmed', 'label_source': 'user',
        'group_key': 'portable-group', 'group_role': 'parent'})
    assert dated.status_code == 200, dated.text
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
    assert (new_doc['document_date'], new_doc['date_status'], new_doc['date_source']) == (
        '2026-10-16', 'confirmed', 'user')
    assert (new_doc['label'], new_doc['label_status'], new_doc['label_source']) == (
        'P-7', 'confirmed', 'user')
    assert (new_doc['group_key'], new_doc['group_role']) == ('portable-group', 'parent')
    assert (new_doc['certificate_signature_status'], new_doc['visible_signature_status'],
            new_doc['signature_status']) == ('not_detected', 'unreviewed',
                                             'no_certificate_signature_detected')
    store = client.app.state.store
    assert store.original_path(new_doc).read_bytes() == original
    assert store.sha256(store.working_path(new_doc)) == store.sha256(store.working_path(rotated.json()))
    with store.connect() as con:
        assert con.execute('SELECT COUNT(*) FROM document_revisions WHERE document_id=?',
                           (new_doc['id'],)).fetchone()[0] == 2
    assert list(store.root.glob('.portable-*.cbmatter*')) == []


def test_legacy_v1_portable_package_still_restores(client, auth, make_pdf):
    matter = _matter(client, auth)
    imported = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
        files=[('files', ('synthetic.pdf', make_pdf(1), 'application/pdf'))])
    assert imported.status_code == 201
    raw = client.get(f'/matters/{matter["id"]}/portable', headers=auth).content
    def as_v1(manifest):
        manifest['version'] = 1
        manifest.pop('manual_bookmarks')
        for document in manifest['documents']:
            for field in ('document_date', 'date_status', 'date_source', 'date_note',
                          'source_file_mtime', 'label_status', 'label_source',
                          'certificate_signature_status', 'certificate_signature_count',
                          'signature_inspection_note', 'visible_signature_status',
                          'signature_status'):
                document.pop(field)
            document.pop('group_role')
    legacy = _package_with_manifest_change(raw, as_v1)
    restored = client.post('/matters/restore', headers=auth,
        files={'file': ('legacy.cbmatter', legacy, 'application/zip')})
    assert restored.status_code == 201, restored.text
    assert client.get(f'/matters/{restored.json()["id"]}/bookmarks', headers=auth).json() == []
    legacy_doc = client.get(f'/matters/{restored.json()["id"]}/documents', headers=auth).json()[0]
    assert legacy_doc['signature_status'] == 'inspection_inconclusive'


def test_portable_matter_values_round_trip_and_legacy_v2(client, auth):
    matter = _matter(client, auth)
    matter_id = matter['id']
    values = {'client_ref': 'Synthetic reference', 'hearing_date': '2026-10-14'}
    saved = client.put(f'/matters/{matter_id}/values', headers=auth, json={'values': values})
    assert saved.status_code == 200, saved.text
    raw = client.get(f'/matters/{matter_id}/portable', headers=auth).content
    restored = client.post('/matters/restore', headers=auth,
        files={'file': ('backup.cbmatter', raw, 'application/zip')})
    assert restored.status_code == 201, restored.text
    assert client.get(f'/matters/{restored.json()["id"]}/values', headers=auth).json() == {'values': values}
    def as_v2(manifest):
        manifest['version'] = 2
        manifest.pop('matter_values')
    legacy = _package_with_manifest_change(raw, as_v2)
    old = client.post('/matters/restore', headers=auth,
        files={'file': ('legacy.cbmatter', legacy, 'application/zip')})
    assert old.status_code == 201, old.text
    assert client.get(f'/matters/{old.json()["id"]}/values', headers=auth).json() == {'values': {}}


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
    client.post(f'/matters/{matter["id"]}/documents', headers=auth,
        files=[('files', ('synthetic-2.pdf', make_pdf(1), 'application/pdf'))])
    raw = client.get(f'/matters/{matter["id"]}/portable', headers=auth).content
    def duplicate_group_parents(manifest):
        for document in manifest['documents']:
            document['group_key'] = 'duplicate-parent-group'
            document['group_role'] = 'parent'
    variants = [
        _package_with_manifest_change(raw, lambda m: m.update(version=99)),
        _package_with_manifest_change(raw, lambda m: m.pop('manual_bookmarks')),
        _package_with_manifest_change(raw, lambda m: m.pop('matter_values')),
        _package_with_manifest_change(raw, lambda m: m.update(matter_values={'Invalid key': 'x'})),
        _package_with_manifest_change(raw, lambda m: m['files'][0].update(path='../escape.pdf')),
        _package_with_manifest_change(raw, lambda m: m['files'][0].update(sha256='0' * 64)),
        _package_with_manifest_change(raw, lambda m: m['matter'].update(court_profile='unreviewed-missing-profile')),
        _package_with_manifest_change(raw, lambda m: m['documents'][0].update(
            signature_status='certificate_signature_detected')),
        _package_with_manifest_change(raw, duplicate_group_parents),
    ]
    for package in variants:
        result = client.post('/matters/restore', headers=auth,
            files={'file': ('bad.cbmatter', package, 'application/zip')})
        assert result.status_code == 422, result.text
    assert len(client.get('/matters', headers=auth).json()) == 1
