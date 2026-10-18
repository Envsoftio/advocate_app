"""FIL-09 technical preset snapshots and the existing deterministic page map."""
import json

from test_api import upload, wait_for_job


def test_versioned_presets_apply_atomically_and_preserve_export_and_backup(
        client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}'
    store = client.app.state.store
    first = upload(client, auth, matter, make_pdf, name='first.pdf', pages=2)
    upload(client, auth, matter, make_pdf, name='second.pdf', pages=2)
    original_hash = store.sha256(store.original_path(first))
    initial_job = wait_for_job(client, auth, client.post(base + '/build', headers=auth).json()['id'])
    assert initial_job['status'] == 'completed', initial_job['error']
    old_folder = store.exports_root(matter['id']) / initial_job['result']['id']
    old_pdf = (old_folder / 'combined.pdf').read_bytes()

    marks = client.get(base + '/output-marks', headers=auth).json()
    marks['watermark_enabled'] = True
    marks['watermark_text'] = 'REVIEW COPY'
    request = {'name': 'Chamber draft', 'max_pages_per_volume': 3,
               'keep_groups_intact': True, 'output_marks': marks}
    first_version = client.post('/output-presets', headers=auth, json=request)
    assert first_version.status_code == 201, first_version.text
    first_version = first_version.json()
    assert first_version['version'] == 1
    assert client.get(base + '/exports', headers=auth).json()[0]['stale'] is False

    applied = client.post(base + '/output-preset/' + first_version['id'], headers=auth)
    assert applied.status_code == 200, applied.text
    assert applied.json()['version'] == 1
    assert client.get(base, headers=auth).json()['max_pages_per_volume'] == 3
    assert client.get(base + '/output-marks', headers=auth).json() == marks
    assert client.get(base + '/exports', headers=auth).json()[0]['stale'] is True

    preview = client.post(base + '/bundle-preview', headers=auth,
                          json={'max_pages_per_volume': 3, 'keep_groups_intact': True})
    assert preview.status_code == 200, preview.text
    rebuilt = wait_for_job(client, auth, client.post(base + '/build', headers=auth).json()['id'])
    assert rebuilt['status'] == 'completed', rebuilt['error']
    folder = store.exports_root(matter['id']) / rebuilt['result']['id']
    report = json.loads((folder / 'validation-report.json').read_text())
    assert report['output_preset']['id'] == first_version['id']
    assert report['volume_count'] == 2
    assert report['combined_pages'] == 6
    assert (old_folder / 'combined.pdf').read_bytes() == old_pdf
    assert store.sha256(store.original_path(first)) == original_hash

    updated_request = dict(request, max_pages_per_volume=6)
    second_version = client.post('/output-presets', headers=auth,
                                 json=updated_request).json()
    assert second_version['version'] == 2
    assert client.get(base + '/output-preset', headers=auth).json()['version'] == 1
    assert client.get(base + '/exports', headers=auth).json()[0]['stale'] is False
    assert client.post(base + '/output-preset/' + second_version['id'],
                       headers=auth).status_code == 200
    assert client.get(base + '/exports', headers=auth).json()[0]['stale'] is True
    assert client.post(base + '/history/undo', headers=auth).status_code == 200
    assert client.get(base + '/output-preset', headers=auth).json()['version'] == 1
    assert client.get(base, headers=auth).json()['max_pages_per_volume'] == 3
    assert client.post(base + '/history/redo', headers=auth).status_code == 200
    assert client.get(base + '/output-preset', headers=auth).json()['version'] == 2

    package = client.get(base + '/portable', headers=auth)
    restored = client.post('/matters/restore', headers=auth,
        files={'file': ('matter.cbmatter', package.content, 'application/zip')})
    assert restored.status_code == 201, restored.text
    restored_base = f'/matters/{restored.json()["id"]}'
    assert client.get(restored_base + '/output-preset', headers=auth).json() == \
           client.get(base + '/output-preset', headers=auth).json()
    assert client.get(restored_base + '/output-marks', headers=auth).json() == marks


def test_preset_validation_and_other_matter_does_not_change(client, auth, matter):
    other = client.post('/matters', headers=auth, json={
        'name': 'Another synthetic matter', 'court_profile': 'delhi-hc-illustrative',
        'case_type': 'Demo', 'max_pages_per_volume': 50}).json()
    marks = client.get(f'/matters/{matter["id"]}/output-marks', headers=auth).json()
    invalid = client.post('/output-presets', headers=auth, json={
        'name': '  ', 'max_pages_per_volume': 1, 'output_marks': marks})
    assert invalid.status_code == 422
    preset = client.post('/output-presets', headers=auth, json={
        'name': 'Technical', 'max_pages_per_volume': 4,
        'output_marks': marks}).json()
    assert client.post(f'/matters/{matter["id"]}/output-preset/{preset["id"]}',
                       headers=auth).status_code == 200
    assert client.get(f'/matters/{other["id"]}', headers=auth).json()['max_pages_per_volume'] == 50
    assert client.get(f'/matters/{other["id"]}/output-preset', headers=auth).json() is None
