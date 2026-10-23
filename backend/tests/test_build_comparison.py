from app.engine import compare_bundle_plan
from test_api import upload, wait_for_job


def test_build_comparison_reports_inserted_document_and_shifted_ranges(client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}'
    first = upload(client, auth, matter, make_pdf, name='first.pdf', pages=2)
    second = upload(client, auth, matter, make_pdf, name='second.pdf', pages=2)
    started = client.post(base + '/build', headers=auth)
    assert wait_for_job(client, auth, started.json()['id'])['status'] == 'completed'
    prior = client.get(base + '/exports', headers=auth).json()[0]
    assert len(prior['page_map_snapshot']) == 2
    original = client.app.state.store.original_path(first).read_bytes()
    inserted = upload(client, auth, matter, make_pdf, name='inserted.pdf', pages=1)
    order = [first['id'], inserted['id'], second['id']]
    assert client.post(base + '/documents/reorder', headers=auth, json={'ids': order}).status_code == 200
    route = base + f'/exports/{prior["id"]}/compare'
    response = client.post(route, headers=auth, json={'max_pages_per_volume': 300,
                                                     'keep_groups_intact': True})
    assert response.status_code == 200, response.text
    result = response.json()
    assert result['available'] is True
    assert result['prior_pages'] == 5 and result['current_pages'] == 6
    by_id = {item['document_id']: item for item in result['changes']}
    assert 'added' in by_id[inserted['id']]['reasons']
    assert by_id[inserted['id']]['before'] is None
    assert by_id[inserted['id']]['after']['combined_start_page'] == 4
    assert 'page range shifted' in by_id[second['id']]['reasons']
    assert by_id[second['id']]['before']['combined_start_page'] == 4
    assert by_id[second['id']]['after']['combined_start_page'] == 5
    assert client.app.state.store.original_path(first).read_bytes() == original
    assert client.get(base + '/exports', headers=auth).json()[0]['stale'] is True


def test_build_comparison_settings_label_and_legacy_manifest(client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}'
    doc = upload(client, auth, matter, make_pdf, name='one.pdf', pages=1)
    started = client.post(base + '/build', headers=auth)
    assert wait_for_job(client, auth, started.json()['id'])['status'] == 'completed'
    prior = client.get(base + '/exports', headers=auth).json()[0]
    route = base + f'/exports/{prior["id"]}/compare'
    payload = {'max_pages_per_volume': 300, 'keep_groups_intact': True}
    assert client.post(route, headers=auth, json=payload).json()['current_matches_prior'] is True
    assert client.patch(f'/documents/{doc["id"]}', headers=auth,
                        json={'label': 'ANN-1'}).status_code == 200
    changed = client.post(route, headers=auth,
                          json={**payload, 'max_pages_per_volume': 10}).json()
    assert 'Volume page cap' in changed['setting_changes']
    assert changed['changes'][0]['before']['label'] == ''
    assert changed['changes'][0]['after']['label'] == 'ANN-1'
    assert 'label changed' in changed['changes'][0]['reasons']
    legacy = dict(prior)
    legacy.pop('page_map_snapshot')
    assert compare_bundle_plan(client.app.state.store, matter['id'], legacy, 300, True)['available'] is False


def test_build_comparison_uses_volume_map_when_cap_changes(client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}'
    first = upload(client, auth, matter, make_pdf, name='first.pdf', pages=2)
    second = upload(client, auth, matter, make_pdf, name='second.pdf', pages=2)
    assert client.patch(base, headers=auth, json={'max_pages_per_volume': 4}).status_code == 200
    started = client.post(base + '/build', headers=auth)
    assert wait_for_job(client, auth, started.json()['id'])['status'] == 'completed'
    prior = client.get(base + '/exports', headers=auth).json()[0]
    assert prior['volumes'] == 2 and prior['pages'] == 6
    route = base + f'/exports/{prior["id"]}/compare'
    comparison = client.post(route, headers=auth,
                             json={'max_pages_per_volume': 5,
                                   'keep_groups_intact': True}).json()
    assert comparison['current_pages'] == 5
    assert comparison['setting_changes'] == ['Volume page cap']
    second_change = next(item for item in comparison['changes']
                         if item['document_id'] == second['id'])
    assert second_change['before']['volume'] == 2
    assert second_change['before']['combined_start_page'] == 5
    assert second_change['after']['volume'] == 1
    assert second_change['after']['combined_start_page'] == 4
    assert first['id'] not in {item['document_id'] for item in comparison['changes']}


def test_build_comparison_distinguishes_replacement_from_reorder(client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}'
    first = upload(client, auth, matter, make_pdf, name='first.pdf', pages=2)
    second = upload(client, auth, matter, make_pdf, name='second.pdf', pages=1)
    started = client.post(base + '/build', headers=auth)
    assert wait_for_job(client, auth, started.json()['id'])['status'] == 'completed'
    prior = client.get(base + '/exports', headers=auth).json()[0]
    assert client.post(f'/documents/{first["id"]}/replace', headers=auth,
                       json={'source_document_id': second['id']}).status_code == 200
    assert client.post(base + '/documents/reorder', headers=auth,
                       json={'ids': [second['id'], first['id']]}).status_code == 200
    result = client.post(base + f'/exports/{prior["id"]}/compare', headers=auth,
                         json={'max_pages_per_volume': 300,
                               'keep_groups_intact': True}).json()
    by_id = {item['document_id']: item for item in result['changes']}
    assert 'working PDF changed' in by_id[first['id']]['reasons']
    assert 'order changed' in by_id[first['id']]['reasons']
    assert 'order changed' in by_id[second['id']]['reasons']
    assert result['prior_pages'] == 4 and result['current_pages'] == 3
