import hashlib
import csv
import io
from app.storage import Store
from app.engine import compare_bundle_plan, insert_suffix_labels, ProcessingError
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


def test_preserved_insert_policy_builds_distinct_validated_artifact(client, auth, matter, make_pdf):
    base=f'/matters/{matter["id"]}'
    target=upload(client,auth,matter,make_pdf,name='issued-material.pdf',pages=10)
    primary_job=client.post(base+'/build',headers=auth).json()
    assert wait_for_job(client,auth,primary_job['id'])['status']=='completed'
    prior=client.get(base+'/exports',headers=auth).json()[0]
    store=client.app.state.store
    preserved_paths=[store.original_path(target), *[p for p in (store.exports_root(matter['id'])/prior['id']).iterdir() if p.is_file()]]
    preserved_hashes={p:hashlib.sha256(p.read_bytes()).hexdigest() for p in preserved_paths}

    source=upload(client,auth,matter,make_pdf,name='inserted-pages.pdf',pages=8)
    inserted=client.post(f'/documents/{target["id"]}/pages/insert',headers=auth,json={
        'source_document_id':source['id'],'source_pages':list(range(8)),
        'before_index':5,'expected_revision':1,'expected_source_revision':1})
    assert inserted.status_code==200,inserted.text
    catalog=client.get(base+'/supplements',headers=auth).json()
    current=next(d for d in catalog['documents'] if d['id']==target['id'])
    exact_ids=[p['id'] for p in current['pages']][5:13]
    payload=dict(baseline_build_id=prior['id'],title='Eight-page preserved insert',
        selected_page_ids=exact_ids,expected_snapshot=catalog['input_snapshot'],
        insertion_policy='preserve_printed_labels',policy_reviewed=True,**catalog['defaults'])
    preview=client.post(base+'/supplement-policy-preview',headers=auth,json=payload)
    assert preview.status_code==200,preview.text
    policy=preview.json()
    assert not policy['blockers']
    assert list(policy['labels_by_page_id'])==exact_ids
    assert all(label.count('.')>=1 for label in policy['labels_by_page_id'].values())
    assert policy['preview_rows'][0]['kind']=='earlier'
    assert policy['preview_rows'][-1]['combined_position']>policy['preview_rows'][0]['combined_position']
    assert policy['volume_boundaries'][0]['shift']==8

    saved=client.post(base+'/supplements',headers=auth,json=payload)
    assert saved.status_code==200,saved.text
    plan=saved.json()
    assert plan['reviewed_policy']['mode']=='preserve_printed_labels'
    job=client.post(base+f'/supplements/{plan["id"]}/build',headers=auth,json={}).json()
    done=wait_for_job(client,auth,job['id'])
    assert done['status']=='completed',done
    artifact=next(item for item in client.get(base+'/exports',headers=auth).json() if item['id']==job['id'])
    assert artifact['artifact_kind']=='supplement'
    assert artifact['supplement_plan_id']==plan['id']
    assert artifact['supplement_plan_version']==plan['version']
    assert artifact['baseline_build_id']==prior['id']
    assert artifact['insertion_policy']=='preserve_printed_labels'
    assert artifact['pages']==10 and artifact['volumes']==1
    assert artifact['bundle_membership']['selected_page_ids']==exact_ids
    contract=artifact['validation_contract']['combined.pdf']
    selected_labels=[contract['printed_labels'][int(position)] for position,source_map in contract['sources'].items()
                     if source_map['document_id']==target['id']]
    assert selected_labels==list(policy['labels_by_page_id'].values())
    assert contract['labels']==contract['printed_labels']
    assert all(0<=destination<artifact['pages'] for values in contract['links'].values() for destination in values)
    csv_path=store.get_export_file(matter['id'],artifact['id'],'index.csv')
    rows=list(csv.DictReader(io.StringIO(csv_path.read_bytes().decode('utf-8-sig'))))
    assert rows[0]['printed_start']==selected_labels[0] and rows[0]['printed_end']==selected_labels[-1]
    state=client.get(base+f'/exports/{artifact["id"]}/validation/state',headers=auth).json()
    assert state['can_download'] and not state['can_release']
    assert all(hashlib.sha256(path.read_bytes()).hexdigest()==digest for path,digest in preserved_hashes.items())


def test_policy_review_is_invalidated_and_unsupported_or_colliding_labels_block(client, auth, matter, make_pdf):
    base, doc, prior, payload = setup_plan(client, auth, matter, make_pdf)
    payload.update(insertion_policy='separate_supplement',policy_reviewed=True)
    saved=client.post(base+'/supplements',headers=auth,json=payload).json()
    assert saved['reviewed_policy']
    changed={**payload,'expected_version':saved['version'],'title':'Changed after review','policy_reviewed':False}
    changed_saved=client.put(base+'/supplements/'+saved['id'],headers=auth,json=changed).json()
    assert changed_saved['reviewed_policy'] is None
    refused=client.post(base+f'/supplements/{saved["id"]}/build',headers=auth,json={})
    assert refused.status_code==409 and 'Review and accept' in refused.text
    # Deliberate renumbering is a separate, explicitly reviewed policy and produces
    # its own artifact rather than mutating the baseline or silently reusing labels.
    reviewed={**changed,'expected_version':changed_saved['version'],'policy_reviewed':True}
    accepted=client.put(base+'/supplements/'+saved['id'],headers=auth,json=reviewed)
    assert accepted.status_code==200,accepted.text
    accepted=accepted.json()
    separate_preview=client.post(base+'/supplement-policy-preview',headers=auth,json=reviewed).json()
    assert separate_preview['mode']=='separate_supplement'
    assert separate_preview['effects']['viewer_labels']=='Generated from the supplemental numbering settings.'
    separate_job=client.post(base+f'/supplements/{accepted["id"]}/build',headers=auth,json={}).json()
    assert wait_for_job(client,auth,separate_job['id'])['status']=='completed'
    separate=next(item for item in client.get(base+'/exports',headers=auth).json() if item['id']==separate_job['id'])
    assert separate['artifact_kind']=='supplement'
    assert separate['insertion_policy']=='separate_supplement'
    assert separate['baseline_build_id']==prior['id']
    assert separate['validation_contract']['combined.pdf']['printed_labels'][2].startswith('S1 · ')
    try:
        insert_suffix_labels('V3 · xvi',2,set())
        assert False,'unsupported label should fail'
    except ProcessingError as exc:
        assert 'cannot use' in str(exc)
    try:
        insert_suffix_labels('V3 · 16',2,{'V3 · 16.1'})
        assert False,'collision should fail'
    except ProcessingError as exc:
        assert 'collides' in str(exc)
