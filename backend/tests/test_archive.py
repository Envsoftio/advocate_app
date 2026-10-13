def test_archive_restore_preserves_records_files_and_storage(client, auth, matter, make_pdf):
    imported = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
        files=[('files', ('synthetic.pdf', make_pdf(2), 'application/pdf'))])
    doc = imported.json()[0]
    store = client.app.state.store
    original = store.original_path(doc)
    before = store.sha256(original)
    usage = client.get(f'/matters/{matter["id"]}/storage', headers=auth)
    assert usage.status_code == 200
    assert usage.json()['by_category']['originals'] == original.stat().st_size

    archived = client.delete(f'/documents/{doc["id"]}', headers=auth)
    assert archived.status_code == 200
    assert client.get(f'/matters/{matter["id"]}/documents', headers=auth).json() == []
    assert client.get(f'/documents/{doc["id"]}/pdf', headers=auth).status_code == 404
    assert client.get(f'/matters/{matter["id"]}/archived-documents', headers=auth).json()[0]['id'] == doc['id']
    assert store.sha256(original) == before

    restored = client.post(f'/matters/{matter["id"]}/archived-documents/{doc["id"]}/restore', headers=auth)
    assert restored.status_code == 200
    assert restored.json()['id'] == doc['id']
    assert store.sha256(original) == before
    assert client.get(f'/matters/{matter["id"]}/archived-documents', headers=auth).json() == []

    assert client.delete(f'/matters/{matter["id"]}', headers=auth).status_code == 200
    assert client.get(f'/matters/{matter["id"]}', headers=auth).status_code == 404
    assert client.get('/matters/archived', headers=auth).json()[0]['id'] == matter['id']
    assert store.sha256(original) == before
    reopened = client.post(f'/matters/{matter["id"]}/restore', headers=auth)
    assert reopened.status_code == 200
    assert client.get(f'/matters/{matter["id"]}/documents', headers=auth).json()[0]['id'] == doc['id']
    assert client.get('/matters/archived', headers=auth).json() == []


def test_portable_backup_includes_archived_document(client, auth, matter, make_pdf):
    uploaded = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
        files=[('files', ('synthetic.pdf', make_pdf(1), 'application/pdf'))]).json()[0]
    client.delete(f'/documents/{uploaded["id"]}', headers=auth)
    backup = client.get(f'/matters/{matter["id"]}/portable', headers=auth)
    assert backup.status_code == 200
    restored = client.post('/matters/restore', headers=auth,
        files={'file': ('backup.cbmatter', backup.content, 'application/zip')})
    assert restored.status_code == 201, restored.text
    restored_id = restored.json()['id']
    assert client.get(f'/matters/{restored_id}/documents', headers=auth).json() == []
    archived = client.get(f'/matters/{restored_id}/archived-documents', headers=auth).json()
    assert len(archived) == 1
    assert archived[0]['source_sha256'] == uploaded['source_sha256']


def test_audit_records_actions_without_document_text_or_credentials(client, auth, matter, make_pdf):
    secret_title = 'Sensitive synthetic title 987654'
    uploaded = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
        files=[('files', ('synthetic.pdf', make_pdf(1), 'application/pdf'))]).json()[0]
    client.patch(f'/documents/{uploaded["id"]}', headers=auth, json={'title': secret_title})
    events = client.get(f'/matters/{matter["id"]}/audit', headers=auth)
    assert events.status_code == 200
    actions = [event['action'] for event in events.json()]
    assert 'document_imported' in actions and 'document_updated' in actions
    assert secret_title not in events.text
    assert 'unit-test-secret' not in events.text
    assert all(set(event) == {'action', 'document_id', 'created_at'} for event in events.json())
