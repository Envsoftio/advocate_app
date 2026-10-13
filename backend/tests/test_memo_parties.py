from fastapi.testclient import TestClient

from app.main import create_app


def test_memo_parties_are_matter_scoped_and_persist_after_restart(tmp_path):
    root = tmp_path / 'userdata'
    auth = {'Authorization': 'Bearer memo-secret'}
    sample = dict(role='Petitioner', name='Synthetic Party', fatherOrHusbandName='',
                  address='Synthetic address', email='', mobile='')
    with TestClient(create_app(data_dir=root, token='memo-secret')) as client:
        fields = dict(name='Synthetic Matter', court_profile='delhi-hc-illustrative',
                      case_type='Demo', max_pages_per_volume=10)
        first = client.post('/matters', headers=auth, json=fields).json()['id']
        second = client.post('/matters', headers=auth, json=fields).json()['id']
        assert client.get(f'/matters/{first}/memo-parties', headers=auth).json() == {'parties': []}
        saved = client.put(f'/matters/{first}/memo-parties', headers=auth, json={'parties': [sample]})
        assert saved.status_code == 200, saved.text
        assert saved.json()['parties'] == [sample]
        assert client.get(f'/matters/{second}/memo-parties', headers=auth).json() == {'parties': []}
        assert client.put(f'/matters/{first}/memo-parties', headers=auth,
                          json={'parties': [dict(sample, role='invalid')]}).status_code == 422
        assert client.get(f'/matters/{first}/memo-parties', headers=auth).json()['parties'] == [sample]

    with TestClient(create_app(data_dir=root, token='memo-secret')) as reopened:
        assert reopened.get(f'/matters/{first}/memo-parties', headers=auth).json()['parties'] == [sample]
