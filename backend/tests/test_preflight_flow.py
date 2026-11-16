"""FLOW08 saved-input review never passes an error or releases an artifact."""
import pymupdf
from app.validation import preflight_review_state
from app.storage import Store
from test_api import upload


def test_review_is_snapshot_bound_persistent_and_conflict_checked(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf)
    url = f'/matters/{matter["id"]}'
    checks = client.get(url + '/validate', headers=auth).json()
    state = client.get(url + '/preflight/review', headers=auth).json()
    warning = next(c for c in checks if c['code'] == 'DOCUMENT_DATE_MISSING')
    payload = {**state, 'run_id': checks[0]['validation_run_id'],
               'items': {k: True for k in state['items']}, 'warnings': [warning['finding_id']]}
    saved = client.put(url + '/preflight/review', headers=auth, json=payload)
    assert saved.status_code == 200, saved.text
    assert client.put(url + '/preflight/review', headers=auth, json=payload).status_code == 409
    checks2 = client.get(url + '/validate', headers=auth).json()
    assert checks2[0]['validation_run_id'] != checks[0]['validation_run_id']
    assert client.get(url + '/preflight/review', headers=auth).json() == saved.json()
    client.post(url + '/documents', headers=auth,
                files={'files': ('reference.pdf', make_pdf(1), 'application/pdf')})
    assert client.get(url + '/preflight/review', headers=auth).json() == saved.json()
    client.patch(f'/documents/{doc["id"]}', headers=auth, json={'title': 'Changed included title'})
    changed = client.get(url + '/preflight/review', headers=auth).json()
    assert changed['input_digest'] != state['input_digest']
    assert not any(changed['items'].values()) and changed['warnings'] == []
    assert client.put(url + '/preflight/review', headers=auth, json={**saved.json(), 'run_id': checks[0]['validation_run_id']}).status_code == 409
    store = client.app.state.store
    original = store.sha256(store.original_path(store.get_document(doc['id'])))
    root = store.root
    store.close()
    reopened = Store(root)
    try:
        assert preflight_review_state(reopened, matter['id']) == changed
        assert reopened.sha256(reopened.original_path(reopened.get_document(doc['id']))) == original
    finally:
        reopened.close()


def test_errors_cannot_be_acknowledged_and_auth_required(client, auth, matter):
    url = f'/matters/{matter["id"]}'
    checks = client.get(url + '/validate', headers=auth).json()
    state = client.get(url + '/preflight/review', headers=auth).json()
    error = next(c for c in checks if c['severity'] == 'error')
    payload = {**state, 'run_id': checks[0]['validation_run_id'], 'warnings': [error['finding_id']]}
    assert client.put(url + '/preflight/review', headers=auth, json=payload).status_code == 409
    assert client.get(url + '/preflight/review').status_code == 401


def test_render_failure_and_volume_plan_are_reported(client, auth, matter, make_pdf, monkeypatch):
    doc = upload(client, auth, matter, make_pdf, pages=3)
    url = f'/matters/{matter["id"]}'
    original_pixmap = pymupdf.Page.get_pixmap
    def fail_page(self, *args, **kwargs):
        if self.number == 1:
            raise RuntimeError('Synthetic rendering failure')
        return original_pixmap(self, *args, **kwargs)
    monkeypatch.setattr(pymupdf.Page, 'get_pixmap', fail_page)
    checks = client.get(url + '/validate', headers=auth).json()
    issue = next(c for c in checks if c['code'] == 'PAGE_RENDER_FAILED')
    assert issue['severity'] == 'error' and issue['page_index'] == 1 and issue['document_id'] == doc['id']
    monkeypatch.setattr(pymupdf.Page, 'get_pixmap', original_pixmap)
    client.patch(url, headers=auth, json={'max_pages_per_volume': 2})
    checks = client.get(url + '/validate', headers=auth).json()
    assert any(c['code'] == 'PAGE_MAP_UNAVAILABLE' and c['severity'] == 'error' for c in checks)


def test_unresolved_bookmark_visible_without_changing_omission_policy(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, pages=3)
    url = f'/matters/{matter["id"]}'
    response = client.put(url + '/bookmarks', headers=auth, json={'entries': [{
        'id': 'a'*32, 'title': 'Synthetic target', 'document_id': doc['id'],
        'page_index': 2, 'target_revision': doc['revision'], 'parent_id': None}]})
    assert response.status_code == 200, response.text
    client.post(f'/documents/{doc["id"]}/pages/delete', headers=auth,
                json={'page_index': 2, 'expected_revision': doc['revision']})
    checks = client.get(url + '/validate', headers=auth).json()
    issue = next(c for c in checks if c['code'] == 'UNRESOLVED_BOOKMARK')
    assert issue['severity'] == 'warning' and issue['action'] == 'bookmarks'


def test_ocr_review_finding_tracks_actual_page_acceptance_and_revision(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, pages=2)
    store = client.app.state.store
    store.update_document(doc['id'], {'ocr_status': 'processed'})
    current = store.get_document(doc['id'])
    url = f'/matters/{matter["id"]}/validate'
    findings = client.get(url, headers=auth).json()
    assert [f['page_index'] for f in findings if f['code'] == 'OCR_REVIEW_PENDING'] == [0, 1]
    response = client.post(f'/documents/{doc["id"]}/ocr-review/accept', headers=auth,
                           json={'expected_revision': current['revision'], 'page_index': 0})
    assert response.status_code == 200, response.text
    findings = client.get(url, headers=auth).json()
    assert [f['page_index'] for f in findings if f['code'] == 'OCR_REVIEW_PENDING'] == [1]


def test_working_page_count_mismatch_blocks_preflight(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, pages=2)
    store = client.app.state.store
    # Simulate a damaged working file outside the revision API; never touch original.
    from app.file_io import write_bytes
    original = store.sha256(store.original_path(store.get_document(doc['id'])))
    write_bytes(store.working_path(store.get_document(doc['id'])), make_pdf(1))
    findings = client.get(f'/matters/{matter["id"]}/validate', headers=auth).json()
    assert any(f['code'] == 'PAGE_COUNT_MISMATCH' and f['severity'] == 'error' for f in findings)
    assert store.sha256(store.original_path(store.get_document(doc['id']))) == original
