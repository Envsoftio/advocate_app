"""Reading metadata survives restart in the protected store without editing matter data."""
from fastapi.testclient import TestClient
from app.main import create_app
import pytest


def test_protected_reading_position_survives_restart_and_revision_guard(tmp_path, make_pdf):
    root = tmp_path / 'protected-reader'
    secret = b'r' * 32
    auth = {'Authorization': 'Bearer reader-test'}
    def open_client():
        return TestClient(create_app(root, token='reader-test', data_key=secret, require_vault=False))
    with open_client() as client:
        matter = client.post('/matters', headers=auth, json={'name': 'Synthetic reader', 'court_profile': 'general-demo'}).json()
        document = client.post(f'/matters/{matter["id"]}/documents?included=true', headers=auth,
            files=[('files', ('reader.pdf', make_pdf(4), 'application/pdf'))]).json()[0]
        key = f'{document["id"]}:1'
        state = {'page': 3, 'x': .1, 'y': .42, 'zoom': '125', 'mode': 'continuous'}
        store = client.app.state.store
        original = store.sha256(store.original_path(document))
        history = store.history_state(matter['id'])
        updated = store.get_matter(matter['id'])['updated_at']
        assert client.get('/reader-position', params={'key': key}, headers=auth).json() is None
        saved = client.put('/reader-position', json={'key': key, **state}, headers=auth)
        assert saved.status_code == 200, saved.text
        assert store.history_state(matter['id']) == history
        assert store.get_matter(matter['id'])['updated_at'] == updated
        assert store.sha256(store.original_path(document)) == original
        assert client.get('/reader-position', params={'key': key}).status_code == 401
    assert not (root / 'court_bundle.sqlite3').read_bytes().startswith(b'SQLite format 3')
    assert key.encode() not in (root / 'court_bundle.sqlite3').read_bytes()
    with open_client() as client:
        assert client.get('/reader-position', params={'key': key}, headers=auth).json() == state
        changed = client.post(f'/documents/{document["id"]}/pages/rotate', headers=auth,
                             json={'page_index': 0, 'degrees': 90, 'expected_revision': 1})
        assert changed.status_code == 200, changed.text
        assert client.put('/reader-position', json={'key': key, **state}, headers=auth).status_code == 409
        assert client.get('/reader-position', params={'key': f'{document["id"]}:2'}, headers=auth).json() is None


@pytest.mark.parametrize('patch', [{'page': 0}, {'page': True}, {'page': 5}, {'x': 1.1}, {'y': -1},
                                  {'zoom': '999'}, {'mode': 'dual'}, {'text': 'not navigation metadata'}])
def test_invalid_reader_metadata(client, auth, matter, make_pdf, patch):
    document = client.post(f'/matters/{matter["id"]}/documents?included=true', headers=auth,
        files=[('files', ('reader.pdf', make_pdf(4), 'application/pdf'))]).json()[0]
    state = {'key': f'{document["id"]}:1', 'page': 2, 'x': 0, 'y': 0, 'zoom': '100', 'mode': 'single', **patch}
    assert client.put('/reader-position', json=state, headers=auth).status_code == 422
