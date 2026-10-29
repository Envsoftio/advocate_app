import hashlib
from app.storage import Store
from app.engine import compare_bundle_plan
from test_api import upload, wait_for_job


def setup_plan(client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}'
    doc = upload(client, auth, matter, make_pdf, name='baseline.pdf', pages=10)
    job = client.post(base+'/build', headers=auth).json()
    assert wait_for_job(client, auth, job['id'])['status'] == 'completed'
    prior = client.get(base+'/exports', headers=auth).json()[0]
    cat = client.get(base+'/supplements', headers=auth).json()
    ids = [p['id'] for p in cat['documents'][0]['pages']][1:9]
    payload = dict(baseline_build_id=prior['id'], title='Supplementary evidence', selected_page_ids=ids,
                   expected_snapshot=cat['input_snapshot'], **cat['defaults'])
    return base, doc, prior, payload


def test_eight_exact_pages_shared_map_settings_restart_and_hashes(client, auth, matter, make_pdf):
    base, doc, prior, payload = setup_plan(client, auth, matter, make_pdf)
    store = client.app.state.store
    paths = [store.original_path(doc), *store.exports_root(matter['id']).rglob('*')]
    hashes = {p: hashlib.sha256(p.read_bytes()).hexdigest() for p in paths if p.is_file()}
    primary = client.get(base+'/bundle-settings', headers=auth).json()
    saved = client.post(base+'/supplements', headers=auth, json=payload)
    assert saved.status_code == 200, saved.text
    plan = saved.json()
    preview = client.post(base+'/supplement-preview', headers=auth, json=payload)
    assert preview.status_code == 200, preview.text
    p = preview.json()
    assert (p['source_pages'], p['generated_pages'], p['combined_pages']) == (8, 2, 10)
    mapped = [r for v in p['volumes'] for r in v['pages'] if r['stable_page_id']]
    assert [r['stable_page_id'] for r in mapped] == payload['selected_page_ids']
    assert [r['page_index'] for r in mapped] == list(range(1,9))
    assert mapped[0]['physical_page'] == 3 and mapped[0]['printed_label'] == 'S1 · 3'
    # Same selected identities, genuinely different generated-page rule.
    payload['output_marks']['front_matter'] = 'index_only'
    p = client.post(base+'/supplement-preview', headers=auth, json=payload).json()
    assert (p['generated_pages'], p['combined_pages']) == (1,9)
    assert client.get(base+'/bundle-settings', headers=auth).json() == primary
    root = store.root
    store.close()
    reopened = Store(root)
    try:
        from app.supplements import plans, catalog
        assert plans(reopened,matter['id'])[0] == plan
        assert [p['id'] for p in catalog(reopened,matter['id'])[0]['pages']][1:9] == payload['selected_page_ids']
    finally:
        reopened.close()
    assert all(hashlib.sha256(p.read_bytes()).hexdigest()==h for p,h in hashes.items())


def test_supplement_stale_reorder_deleted_sources_and_concurrent_save_fail_closed(client, auth, matter, make_pdf):
    base, doc, prior, payload = setup_plan(client, auth, matter, make_pdf)
    saved = client.post(base+'/supplements', headers=auth, json=payload).json()
    route = base+'/supplements/'+saved['id']
    assert client.put(route,headers=auth,json={**payload,'expected_version':999}).status_code == 409
    assert client.post(f'/documents/{doc["id"]}/pages/reorder',headers=auth,json={'order':list(reversed(range(10)))}).status_code == 200
    assert client.post(base+'/supplement-preview',headers=auth,json=payload).status_code == 409
    catalog = client.get(base+'/supplements',headers=auth).json()
    assert set(payload['selected_page_ids']).isdisjoint(p['id'] for p in catalog['documents'][0]['pages'])
    payload['expected_snapshot']=catalog['input_snapshot']
    stale = client.post(base+'/supplement-preview',headers=auth,json=payload)
    assert stale.status_code==409 and 'stale working revision' in stale.text
    assert client.put(route,headers=auth,json={**payload,'expected_version':1}).status_code==409
    assert client.get(base+'/supplements',headers=auth).json()['plans'][0]['selected_page_ids']==saved['selected_page_ids']
    assert client.delete(f'/documents/{doc["id"]}',headers=auth).status_code==200
    payload['expected_snapshot']=client.get(base+'/supplements',headers=auth).json()['input_snapshot']
    assert client.post(base+'/supplement-preview',headers=auth,json=payload).status_code==409


def test_noncontiguous_selection_and_volume_overhead_empty_and_invalid(client, auth, matter, make_pdf):
    base, doc, prior, payload = setup_plan(client, auth, matter, make_pdf)
    all_ids=[p['id'] for p in client.get(base+'/supplements',headers=auth).json()['documents'][0]['pages']]
    payload['selected_page_ids']=[all_ids[8],all_ids[0],all_ids[1],all_ids[5]]
    payload['max_pages_per_volume']=4
    preview=client.post(base+'/supplement-preview',headers=auth,json=payload).json()
    assert preview['source_pages']==4 and preview['generated_pages']==6 and preview['combined_pages']==10
    assert [(r['source_start'],r['source_end']) for v in preview['volumes'] for r in v['rows']]==[(8,9),(0,2),(5,6)]
    assert [p['stable_page_id'] for v in preview['volumes'] for p in v['pages'] if p['stable_page_id']]==payload['selected_page_ids']
    payload['selected_page_ids']=[]
    assert client.post(base+'/supplements',headers=auth,json=payload).status_code==200
    empty=client.post(base+'/supplement-preview',headers=auth,json=payload).json()
    assert empty['combined_pages']==0 and empty['generated_pages']==0
    for ids in [[all_ids[0],all_ids[0]],['unknown']]:
        payload['selected_page_ids']=ids
        assert client.post(base+'/supplements',headers=auth,json=payload).status_code==409


def test_revision_comparison_membership_same_name_reordered_and_legacy(client, auth, matter, make_pdf):
    base, doc, prior, payload = setup_plan(client, auth, matter, make_pdf)
    second=upload(client,auth,matter,make_pdf,name='baseline.pdf',pages=10)
    comparison=client.post(base+f'/exports/{prior["id"]}/compare',headers=auth,json={'max_pages_per_volume':300,'keep_groups_intact':True}).json()
    by_id={d['document_id']:d for d in comparison['documents']}
    assert by_id[doc['id']]['status']=='unchanged' and by_id[second['id']]['status']=='added'
    client.post(f'/documents/{doc["id"]}/pages/reorder',headers=auth,json={'order':list(reversed(range(10)))})
    comparison=client.post(base+f'/exports/{prior["id"]}/compare',headers=auth,json={'max_pages_per_volume':300,'keep_groups_intact':True}).json()
    assert next(d for d in comparison['documents'] if d['document_id']==doc['id'])['status']=='changed'
    membership=client.get(base+'/membership',headers=auth).json()
    client.put(base+'/membership',headers=auth,json={'included_ids':[second['id']],'expected_snapshot':membership['input_snapshot']})
    comparison=client.post(base+f'/exports/{prior["id"]}/compare',headers=auth,json={'max_pages_per_volume':300,'keep_groups_intact':True}).json()
    assert next(d for d in comparison['documents'] if d['document_id']==doc['id'])['status']=='omitted'
    legacy={**prior,'input_revisions':[]}
    assert not compare_bundle_plan(client.app.state.store,matter['id'],legacy,300,True)['available']


def test_baseline_is_immutable_unknown_build_and_foreign_selection_rejected(client, auth, matter, make_pdf):
    base, doc, prior, payload = setup_plan(client, auth, matter, make_pdf)
    saved=client.post(base+'/supplements',headers=auth,json=payload).json()
    job=client.post(base+'/build',headers=auth).json()
    assert wait_for_job(client,auth,job['id'])['status']=='completed'
    other=client.get(base+'/exports',headers=auth).json()[0]
    assert other['id']!=prior['id']
    changed={**payload,'expected_version':1,'baseline_build_id':other['id']}
    assert client.put(base+'/supplements/'+saved['id'],headers=auth,json=changed).status_code==409
    assert client.post(base+'/supplements',headers=auth,json={**payload,'baseline_build_id':'0'*32}).status_code==409
    assert client.get(base+'/supplements',headers=auth).json()['plans'][0]['baseline_build_id']==prior['id']


def test_primary_build_only_registers_included_source_identities(client, auth, matter, make_pdf):
    base=f'/matters/{matter["id"]}'
    doc=upload(client,auth,matter,make_pdf,pages=2)
    reference=client.post(base+'/documents',headers=auth,files={'files':('reference.pdf',make_pdf(1),'application/pdf')}).json()[0]
    job=client.post(base+'/build',headers=auth).json()
    assert wait_for_job(client,auth,job['id'])['status']=='completed'
    with client.app.state.store.connect() as con:
        assert {r[0] for r in con.execute('SELECT DISTINCT document_id FROM source_page_identities')}=={doc['id']}
