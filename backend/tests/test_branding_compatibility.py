"""The product rename must preserve existing deployments and credential isolation."""
import pytest
from fastapi.testclient import TestClient

from app.main import create_app


def test_legacy_environment_reopens_existing_matters(tmp_path, monkeypatch):
    for name in ('API_TOKEN', 'DATA_DIR', 'REQUIRE_VAULT'):
        monkeypatch.delenv('MATTERDRAFT_' + name, raising=False)
    monkeypatch.setenv('COURTBUNDLE_API_TOKEN', 'legacy-session')
    monkeypatch.setenv('COURTBUNDLE_DATA_DIR', str(tmp_path))
    monkeypatch.setenv('COURTBUNDLE_REQUIRE_VAULT', '0')
    with TestClient(create_app()) as client:
        created = client.post('/matters', headers={'Authorization': 'Bearer legacy-session'},
                              json={'name': 'Synthetic upgrade matter'})
        assert created.status_code == 201
        matter_id = created.json()['id']
    monkeypatch.setenv('MATTERDRAFT_API_TOKEN', 'new-session')
    with TestClient(create_app()) as client:
        assert client.get('/matters', headers={'Authorization': 'Bearer legacy-session'}).status_code == 401
        response = client.get('/matters', headers={'Authorization': 'Bearer new-session'})
        assert response.status_code == 200
        assert response.json()[0]['id'] == matter_id
        assert client.get('/health', headers={'Authorization': 'Bearer new-session'}).json()['app'] == 'MatterDraft Studio'
    monkeypatch.setenv('MATTERDRAFT_API_TOKEN', '')
    with pytest.raises(RuntimeError, match='MATTERDRAFT_API_TOKEN'):
        create_app()


def test_legacy_vault_requirement_remains_enforced(tmp_path, monkeypatch):
    monkeypatch.delenv('MATTERDRAFT_REQUIRE_VAULT', raising=False)
    monkeypatch.setenv('COURTBUNDLE_REQUIRE_VAULT', '1')
    with TestClient(create_app(data_dir=str(tmp_path), token='synthetic-session')) as client:
        assert client.get('/matters', headers={'Authorization': 'Bearer synthetic-session'}).status_code == 423
