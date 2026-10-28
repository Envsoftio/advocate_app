import time

from fastapi.testclient import TestClient

from app.main import create_app
from app.storage import Store


AUTH = {'Authorization': 'Bearer recovery-secret'}


def test_interrupted_build_is_visible_and_can_retry_once(tmp_path):
    root = tmp_path / 'userdata'
    store = Store(root)
    matter = store.create_matter(dict(
        name='Synthetic recovery', court_profile='delhi-hc-illustrative',
        case_type='Demonstration', petitioner='', respondent='', advocate='', case_number='',
        max_pages_per_volume=300, keep_groups_intact=True))
    store.save_job(dict(id='interrupted-job', matter_id=matter['id'], kind='build',
                        target_id=None, status='running', progress=42, message='Rendering',
                        error=None, result=None))
    abandoned = store.exports_root(matter['id']) / '.build-abandoned'
    abandoned.mkdir(parents=True)
    (abandoned / 'partial.pdf').write_bytes(b'partial')
    store.close()
    app = create_app(data_dir=root, token='recovery-secret')
    with TestClient(app) as client:
        old = client.get('/jobs/interrupted-job', headers=AUTH)
        assert old.status_code == 200
        assert old.json()['status'] == 'interrupted'
        assert 'restart' in old.json()['message']
        assert not abandoned.exists()
        assert len(client.get(f'/matters/{matter["id"]}/jobs', headers=AUTH).json()) == 1
        retry = client.post('/jobs/interrupted-job/retry', headers=AUTH)
        assert retry.status_code == 202, retry.text
        assert retry.json()['id'] != 'interrupted-job'
        assert client.post('/jobs/interrupted-job/retry', headers=AUTH).status_code == 409
        stop = time.monotonic() + 5
        while time.monotonic() < stop:
            new = client.get(f'/jobs/{retry.json()["id"]}', headers=AUTH).json()
            if new['status'] in ('completed', 'failed', 'cancelled'):
                break
            time.sleep(.02)
        assert new['status'] == 'failed'
        assert 'Include at least one document' in new['error']
        assert client.get(f'/matters/{matter["id"]}/exports', headers=AUTH).json() == []

    with TestClient(create_app(data_dir=root, token='recovery-secret')) as reopened:
        assert reopened.get(f'/jobs/{retry.json()["id"]}', headers=AUTH).json()['status'] == 'failed'
        assert reopened.get('/jobs/interrupted-job', headers=AUTH).json()['retry_job_id'] == retry.json()['id']
