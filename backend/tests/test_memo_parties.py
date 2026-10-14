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


def test_memo_parties_and_reusable_values_replay_across_restart(tmp_path):
    root = tmp_path / 'userdata'
    auth = {'Authorization': 'Bearer memo-secret'}
    fields = dict(name='Synthetic Matter', court_profile='delhi-hc-illustrative',
                  case_type='Demo', max_pages_per_volume=10)
    first_party = dict(role='Petitioner', name='Synthetic One', fatherOrHusbandName='',
                       address='Test address', email='', mobile='')
    second_party = dict(first_party, name='Synthetic Two', role='Respondent')
    with TestClient(create_app(data_dir=root, token='memo-secret')) as client:
        matter_id = client.post('/matters', headers=auth, json=fields).json()['id']
        assert client.get(f'/matters/{matter_id}/values', headers=auth).json() == {'values': {}}
        assert client.put(f'/matters/{matter_id}/memo-parties', headers=auth,
                          json={'parties': [first_party, second_party]}).status_code == 200
        assert client.put(f'/matters/{matter_id}/values', headers=auth,
                          json={'values': {'reference_date': '14 October 2026',
                                           'client_ref': 'Synthetic, "quoted" value'}}).status_code == 200
        assert client.put(f'/matters/{matter_id}/values', headers=auth,
                          json={'values': {'Bad key': 'invalid'}}).status_code == 422
        assert client.get(f'/matters/{matter_id}/history', headers=auth).json()['undo_count'] == 2
        assert client.post(f'/matters/{matter_id}/history/undo', headers=auth).status_code == 200
        assert client.get(f'/matters/{matter_id}/values', headers=auth).json() == {'values': {}}
    with TestClient(create_app(data_dir=root, token='memo-secret')) as reopened:
        assert reopened.post(f'/matters/{matter_id}/history/undo', headers=auth).status_code == 200
        assert reopened.get(f'/matters/{matter_id}/memo-parties', headers=auth).json() == {'parties': []}
        assert reopened.post(f'/matters/{matter_id}/history/redo', headers=auth).status_code == 200
        assert reopened.post(f'/matters/{matter_id}/history/redo', headers=auth).status_code == 200
        assert reopened.get(f'/matters/{matter_id}/memo-parties', headers=auth).json()['parties'] == [first_party, second_party]
        assert reopened.get(f'/matters/{matter_id}/values', headers=auth).json()['values']['client_ref'] == 'Synthetic, "quoted" value'
