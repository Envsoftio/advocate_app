import hashlib
import csv
import io
from pathlib import Path
import pymupdf

from app.storage import Store
from test_api import upload, wait_for_job


def test_variant_exact_membership_independent_output_and_restart(client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}'
    first = upload(client, auth, matter, make_pdf, name='first.pdf', pages=3)
    second = upload(client, auth, matter, make_pdf, name='second.pdf', pages=2)
    initial = client.post(base + '/build', headers=auth).json()
    assert wait_for_job(client, auth, initial['id'])['status'] == 'completed'
    store = client.app.state.store
    preserved = [store.original_path(first), store.original_path(second),
                 *[p for p in (store.exports_root(matter['id']) / initial['id']).iterdir() if p.is_file()]]
    hashes = {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in preserved}
    catalog = client.get(base + '/variants', headers=auth).json()
    payload = dict(name='Focused extract', baseline_kind='primary',
                   baseline_id=catalog['primary_baseline']['id'], included_ids=[second['id']],
                   expected_snapshot=catalog['source_snapshot'], **catalog['primary_settings'])
    saved = client.post(base + '/variants', headers=auth, json=payload)
    assert saved.status_code == 200, saved.text
    plan = saved.json()
    assert plan['baseline']['build_id'] == initial['id']
    assert plan['included_ids'] == [second['id']]
    assert len(plan['sources'][0]['page_ids']) == 2
    review = client.get(base + f'/variants/{plan["id"]}/review', headers=auth).json()
    assert review['current']
    assert (review['preview']['source_pages'], review['preview']['generated_pages'],
            review['preview']['combined_pages']) == (2, 1, 3)
    assert next(d for d in review['differences'] if d['document_id'] == first['id'])['state'] == 'excluded'
    assert next(d for d in review['differences'] if d['document_id'] == second['id'])['state'] == 'unchanged'
    job = client.post(base + f'/variants/{plan["id"]}/build', headers=auth).json()
    result = wait_for_job(client, auth, job['id'])
    assert result['status'] == 'completed', result
    exports = client.get(base + '/exports', headers=auth).json()
    variant = next(item for item in exports if item['id'] == job['id'])
    assert variant['artifact_kind'] == 'variant' and variant['variant_plan_id'] == plan['id']
    assert variant['variant_plan_version'] == 1 and variant['pages'] == 3
    assert variant['bundle_membership']['documents'][0]['document_id'] == second['id']
    assert {row['id'] for row in variant['page_map_snapshot']} == {second['id']}
    assert variant['build_input_digest'] == review['contract_digest']
    contract = variant['validation_contract']['combined.pdf']
    assert len(contract['printed_labels']) == variant['pages']
    assert contract['labels'] == contract['printed_labels']
    assert all(0 <= target < variant['pages'] for values in contract['links'].values() for target in values)
    csv_file = store.get_export_file(matter['id'], variant['id'], 'index.csv')
    csv_rows = list(csv.DictReader(io.StringIO(csv_file.read_bytes().decode('utf-8-sig'))))
    assert len(csv_rows) == 1 and csv_rows[0]['title'] == second['title']
    assert int(csv_rows[0]['combined_start_page']) == variant['page_map_snapshot'][0]['combined_start_page']
    with pymupdf.open(store.get_export_file(matter['id'], variant['id'], 'combined.pdf')) as pdf:
        assert len(pdf) == variant['pages']
        assert all(1 <= entry[2] <= len(pdf) for entry in pdf.get_toc())
    qa = client.post(base + f'/exports/{variant["id"]}/validation/run', headers=auth, json={}).json()
    assert qa['can_download'] and not qa['can_release']
    issued_paths = {path: hashlib.sha256(path.read_bytes()).hexdigest()
                    for path in (store.exports_root(matter['id']) / variant['id']).iterdir() if path.is_file()}
    changed_marks = dict(payload['output_marks'], front_matter='cover_index')
    changed = client.put(base + f'/variants/{plan["id"]}', headers=auth,
                         json={**payload, 'expected_version':1, 'output_marks':changed_marks})
    assert changed.status_code == 200, changed.text
    new_job = client.post(base + f'/variants/{plan["id"]}/build', headers=auth).json()
    assert wait_for_job(client, auth, new_job['id'])['status'] == 'completed'
    revised = next(item for item in client.get(base + '/exports', headers=auth).json()
                   if item['id'] == new_job['id'])
    assert revised['id'] != variant['id'] and revised['pages'] == variant['pages'] + 1
    assert revised['variant_plan_version'] == 2 and revised['build_input_digest'] != variant['build_input_digest']
    assert all(hashlib.sha256(path.read_bytes()).hexdigest() == digest
               for path, digest in issued_paths.items())
    for file in variant['files']:
        path = store.exports_root(matter['id']) / variant['id'] / file['name']
        assert file['sha256'] == hashlib.sha256(path.read_bytes()).hexdigest()
    assert all(hashlib.sha256(Path(path).read_bytes()).hexdigest() == digest
               for path, digest in hashes.items())
    root = store.root
    store.close()
    reopened = Store(root)
    try:
        from app.variants import get_plan, review as reopened_review
        assert get_plan(reopened, matter['id'], plan['id'])['version'] == 2
        history = reopened_review(reopened, matter['id'], get_plan(reopened, matter['id'], plan['id']))['output_history']
        assert {item['id'] for item in history} == {variant['id'], revised['id']}
    finally:
        reopened.close()


def test_variant_version_conflict_stale_source_and_prior_output(client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}'
    first = upload(client, auth, matter, make_pdf, name='first.pdf', pages=2)
    cat = client.get(base + '/variants', headers=auth).json()
    payload = dict(name='Audience A', baseline_kind='primary',
                   baseline_id=cat['primary_baseline']['id'], included_ids=[first['id']],
                   expected_snapshot=cat['source_snapshot'], **cat['primary_settings'])
    plan = client.post(base + '/variants', headers=auth, json=payload).json()
    route = base + f'/variants/{plan["id"]}'
    assert client.put(route, headers=auth, json={**payload, 'expected_version':99}).status_code == 409
    assert client.put(route, headers=auth, json={**payload, 'expected_version':1,
                                                'included_ids':[]}).status_code == 200
    assert client.post(route + '/build', headers=auth).status_code == 409
    assert client.put(route, headers=auth, json={**payload, 'expected_version':1}).status_code == 409
    assert client.post(f'/documents/{first["id"]}/pages/reorder', headers=auth,
                       json={'order':[1,0]}).status_code == 200
    review = client.get(route + '/review', headers=auth).json()
    assert not review['current']
    assert client.post(route + '/build', headers=auth).status_code == 409
    updated = client.get(base + '/variants', headers=auth).json()
    assert client.put(route, headers=auth, json={**payload, 'expected_version':2,
                                                'expected_snapshot':updated['source_snapshot']}).status_code == 200
