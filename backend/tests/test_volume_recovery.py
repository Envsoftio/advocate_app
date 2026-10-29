"""B03B: reviewed output boundaries, measured policy failures and atomic recovery."""
import csv
import io
import json
import pymupdf
import pytest
from app.engine import build_bundle, compare_bundle_plan
from app.pdf_ops import ProcessingError
from app.storage import Store
from test_api import upload, wait_for_job
from test_bundle_structure import settings, save, build
from test_manual_bookmarks import bookmark


def planning(client, auth, matter, cfg):
    return client.post(f'/matters/{matter["id"]}/volume-planning', headers=auth, json={
        'max_pages_per_volume': cfg['max_pages_per_volume'], 'keep_groups_intact': cfg['keep_groups_intact'],
        'output_marks': cfg['output_marks'], 'expected_snapshot': cfg['input_snapshot']})


def test_reviewed_split_reconciles_all_destinations_and_preserves_files(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, pages=6)
    store = client.app.state.store
    prior = build(client, auth, matter)
    paths = [store.original_path(doc), store.working_path(doc), *list((store.exports_root(matter['id'])/prior['id']).iterdir())]
    hashes = {p:store.sha256(p) for p in paths}
    entries = [bookmark('a', 'First segment end', doc, 1), bookmark('b', 'Second segment', doc, 3), bookmark('c', 'Last source page', doc, 5)]
    assert client.put(f'/matters/{matter["id"]}/bookmarks', headers=auth,json={'entries':entries}).status_code == 200
    # Cross-segment reviewed links use the same original document ID on both ends.
    links = [dict(id='d'*32,title='Across split',source_document_id=doc['id'],source_page_index=1,
        source_revision=doc['revision'],rect_x=.1,rect_y=.2,rect_width=.2,rect_height=.05,
        kind='internal',target_document_id=doc['id'],target_page_index=4,target_revision=doc['revision'],uri='',position=0)]
    store.set_reviewed_links(matter['id'], links)
    mark = client.post(f'/documents/{doc["id"]}/annotations', headers=auth, json={
        'kind':'text','page_index':4,'expected_revision':doc['revision'],
        'rect_x':.1,'rect_y':.5,'rect_width':.5,'rect_height':.08,
        'body':'MARK ON SOURCE FIVE','color':'blue','output_scope':'pdf'})
    assert mark.status_code == 201, mark.text

    cfg = settings(client, auth, matter)
    cfg['max_pages_per_volume']=4
    cfg['output_marks'].update(front_matter='cover_index',part_extracts_enabled=True, numbering_front_matter='roman',
        numbering_restart='continuous', numbering_back_link=True, bates_enabled=True,
        volume_splits={doc['id']:{'revision':doc['revision'],'after_pages':[2,4]}})
    before = settings(client, auth, matter)
    draft = planning(client, auth, matter, cfg).json()
    assert settings(client, auth, matter)==before
    assert draft['plan']['combined_pages']==12
    assert draft['plan']['source_pages']==6 and draft['plan']['generated_pages']==6
    assert [v['rows'][0]['source_start'] for v in draft['plan']['volumes']]==[0,2,4]
    assert [m['combined_page'] for m in draft['plan']['manual_bookmarks']]==[4,8,12]
    assert [m['volume'] for m in draft['plan']['manual_bookmarks']]==[1,2,3]
    assert save(client,auth,matter,cfg).status_code==200
    result=build(client,auth,matter);folder=store.exports_root(matter['id'])/result['id']
    with pymupdf.open(folder/'combined.pdf') as pdf:
        assert len(pdf)==12
        assert 'MARK ON SOURCE FIVE' in pdf[10].get_text()
        assert all('MARK ON SOURCE FIVE' not in pdf[i].get_text() for i in range(12) if i!=10)
        for source_index,physical in enumerate([2,3,6,7,10,11]):
            assert f'page {source_index+1}' in pdf[physical].get_text()
            assert pdf[physical].get_label()==str(source_index+1)
            assert result['validation_contract']['combined.pdf']['sources'][str(physical)]['page_index']==source_index
        assert [b[2] for b in pdf.get_toc() if b[1] in [e['title'] for e in entries]]==[4,8,12]
        assert [link['page'] for link in pdf[3].get_links()]==[1,10]
        for offset in [0,4,8]:
            assert [l['page'] for l in pdf[offset+1].get_links()]==[offset+2]
    for v in range(1,4):
        with pymupdf.open(folder/f'volume-{v:02}.pdf') as pdf:
            assert len(pdf)==4
            assert [b[2] for b in pdf.get_toc() if b[1]==entries[v-1]['title']]==[4]
            assert [l['page'] for l in pdf[3].get_links()]==[1]  # crossing destination omitted
    with pymupdf.open(folder/'part-01.pdf') as pdf:
        assert len(pdf)==6
        for i,page in enumerate(pdf):assert f'page {i+1}' in page.get_text()
    rows=list(csv.DictReader(io.StringIO((folder/'index.csv').read_text(encoding='utf-8-sig'))))
    assert [r['combined_start_page'] for r in rows]==['3','7','11']
    assert [r['source_start'] for r in rows]==['0','2','4']
    assert [r['source_end'] for r in rows]==['2','4','6']
    assert all(store.sha256(p)==h for p,h in hashes.items())
    assert all(f['bytes']==(folder/f['name']).stat().st_size for f in result['files'])
    qa=client.get(f'/matters/{matter["id"]}/exports/{result["id"]}/validation/state',headers=auth).json()
    assert qa['can_download'],qa
    compare=compare_bundle_plan(store,matter['id'],result,4,True)
    assert compare['current_matches_prior'] and not compare['changes']
    assert len(compare_bundle_plan(store,matter['id'],prior,4,True)['changes'])==3
    store.close()
    restarted=Store(store.root)
    assert restarted.get_output_marks(matter['id'])['volume_splits']==cfg['output_marks']['volume_splits']
    restarted.close()


def test_measured_failure_raise_policy_retry_cancel_and_stale(client,auth,matter,make_pdf,monkeypatch):
    doc=upload(client,auth,matter,make_pdf,pages=2);store=client.app.state.store
    prior=build(client,auth,matter);prior_path=store.exports_root(matter['id'])/prior['id']/'combined.pdf'
    oldhash=store.sha256(prior_path);sourcehash=store.sha256(store.original_path(doc))
    cfg=settings(client,auth,matter);cfg['output_marks']['volume_max_bytes']=1
    assert save(client,auth,matter,cfg).status_code==200
    response=client.post(f'/matters/{matter["id"]}/build',headers=auth)
    job=wait_for_job(client,auth,response.json()['id']);assert job['status']=='failed'
    evidence=job['result']['size_exceptions'][0]
    assert evidence['bytes']>1 and evidence['limit_bytes']==1 and evidence['filename']=='volume-01.pdf'
    assert len(store.list_exports(matter['id']))==1
    cfg=settings(client,auth,matter);current=planning(client,auth,matter,cfg).json()
    assert current['measured_exception']['job_id']==job['id']
    # Raise only the optional volume policy; ZIP/combined sizes may be larger.
    cfg['output_marks']['volume_max_bytes']=evidence['bytes']+100
    assert save(client,auth,matter,cfg).status_code==200
    assert save(client,auth,matter,cfg).status_code==409
    assert planning(client,auth,matter,cfg).status_code==409
    result=build(client,auth,matter)
    assert next(f for f in result['files'] if f['name']=='volume-01.pdf')['bytes']<=cfg['output_marks']['volume_max_bytes']
    assert next(f for f in result['files'] if f['name']=='filing-package.zip')['bytes']>cfg['output_marks']['volume_max_bytes']
    assert planning(client,auth,matter,settings(client,auth,matter)).json()['measured_exception'] is None
    before=len(store.list_exports(matter['id']))
    with pytest.raises(ProcessingError,match='cancelled'):
        build_bundle(store,matter['id'],'a'*32,lambda *args:None,lambda:True)
    assert len(store.list_exports(matter['id']))==before
    assert store.sha256(prior_path)==oldhash and store.sha256(store.original_path(doc))==sourcehash
    state=client.get(f'/matters/{matter["id"]}/exports/{result["id"]}/validation/state',headers=auth).json()
    sizes=next(c for c in state['run']['checks'] if c['id']=='sizes')
    assert sizes['outcome']=='passed' and sizes['evidence']['volume_limit_bytes']==cfg['output_marks']['volume_max_bytes']


@pytest.mark.parametrize('cuts,revision', [([0],1),([6],1),([3,2],1),([2,2],1),([2],99)])
def test_invalid_and_stale_boundaries_do_not_commit(client,auth,matter,make_pdf,cuts,revision):
    doc=upload(client,auth,matter,make_pdf,pages=6);cfg=settings(client,auth,matter);before=json.dumps(cfg,sort_keys=True)
    cfg['output_marks']['volume_splits']={doc['id']:{'revision':revision,'after_pages':cuts}}
    assert save(client,auth,matter,cfg).status_code==422
    assert json.dumps(settings(client,auth,matter),sort_keys=True)==before


def test_page_overhead_keep_together_and_no_silent_split(client,auth,matter,make_pdf):
    doc=upload(client,auth,matter,make_pdf,pages=4);cfg=settings(client,auth,matter)
    cfg['max_pages_per_volume']=4;cfg['output_marks']['front_matter']='cover_index'
    draft=planning(client,auth,matter,cfg).json();assert 'No document was split' in draft['plan_error']
    assert draft['documents'][0]['id']==doc['id']
    cfg['output_marks']['volume_splits']={doc['id']:{'revision':1,'after_pages':[2]}}
    draft=planning(client,auth,matter,cfg).json();assert [v['page_count'] for v in draft['plan']['volumes']]==[4,4]
    cfg['output_marks']['volume_splits']={};cfg['max_pages_per_volume']=6
    assert len(planning(client,auth,matter,cfg).json()['plan']['volumes'])==1


def test_portable_split_policy_remaps_documents_and_rejects_bad_references(client,auth,matter,make_pdf):
    from test_portable import _package_with_manifest_change
    doc=upload(client,auth,matter,make_pdf,pages=4)
    cfg=settings(client,auth,matter)
    cfg['output_marks'].update(volume_max_bytes=1000000,
        volume_splits={doc['id']:{'revision':doc['revision'],'after_pages':[2]}})
    assert save(client,auth,matter,cfg).status_code==200
    raw=client.get(f'/matters/{matter["id"]}/portable',headers=auth).content
    restored=client.post('/matters/restore',headers=auth,
        files={'file':('split.cbmatter',raw,'application/zip')})
    assert restored.status_code==201,restored.text
    target=restored.json()
    new_doc=client.get(f'/matters/{target["id"]}/documents',headers=auth).json()[0]
    new_cfg=settings(client,auth,target)
    assert new_doc['id']!=doc['id']
    assert new_cfg['output_marks']['volume_splits']=={
        new_doc['id']:{'revision':doc['revision'],'after_pages':[2]}}
    assert new_cfg['output_marks']['volume_max_bytes']==1000000
    assert build(client,auth,target)['volumes']==2
    def corrupt(manifest):
        manifest['output_marks']['volume_splits']={'f'*32:{'revision':1,'after_pages':[2]}}
    bad=client.post('/matters/restore',headers=auth,files={
        'file':('bad.cbmatter',_package_with_manifest_change(raw,corrupt),'application/zip')})
    assert bad.status_code==422
