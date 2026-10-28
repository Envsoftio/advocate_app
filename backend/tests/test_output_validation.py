"""B01C actual immutable files, named evidence and independent review/release."""
import hashlib
import json
import pymupdf
from app.validation import status, history
from app.storage import Store
from app.file_io import read_bytes, write_bytes
from test_api import upload
from test_build_workflow import start


def qa_url(matter, build):
    return f'/matters/{matter["id"]}/exports/{build["id"]}'


def test_named_evidence_reopens_and_review_release_survives_restart(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, pages=3)
    build = start(client, auth, matter)['result']; url = qa_url(matter, build)
    state = client.get(url+'/validation/state', headers=auth).json()
    assert state['can_download'] and not state['can_release']
    run = state['run']
    assert run['input_digest'] == build['effective_input_digest']
    assert run['input_revisions'] == build['input_revisions']
    assert run['build_id'] == build['id'] and set(run['selected_files']) == {f['name'] for f in build['files']}
    outcomes = {c['id']: c['outcome'] for c in run['checks']}
    assert all(outcomes[k] == 'passed' for k in ('files','counts','navigation','labels','sizes'))
    assert outcomes['cleanup'] == 'not-run' and outcomes['redaction'] == 'not-applicable'
    assert not any(state['review']['items'].values())
    assert client.get(url+'/combined.pdf?release=true', headers=auth).status_code == 409
    content = client.get(url+'/combined.pdf', headers=auth).content
    with pymupdf.open(stream=content, filetype='pdf') as pdf:
        assert pdf.page_count == run['measured_files']['combined.pdf']['pages']
        assert pdf.get_toc() == build['validation_contract']['combined.pdf']['toc']
        assert [p.get_label() for p in pdf] == build['validation_contract']['combined.pdf']['labels']
    assert hashlib.sha256(content).hexdigest() == run['measured_files']['combined.pdf']['sha256']
    update = {**state['review'], 'items': {k: True for k in state['review']['items']}}
    saved = client.put(url+'/validation/review', headers=auth, json=update)
    assert saved.status_code == 200 and saved.json()['can_release']
    assert client.put(url+'/validation/review', headers=auth, json=update).status_code == 409
    assert client.get(url+'/combined.pdf?release=true', headers=auth).content == content
    store=client.app.state.store; root=store.root
    original=read_bytes(store.original_path(store.get_document(doc['id'])))
    store.close(); reopened=Store(root)
    try:
        persisted=status(reopened,matter['id'],build['id'])
        assert persisted['can_release'] and persisted['run']==run
        assert read_bytes(reopened.original_path(reopened.get_document(doc['id'])))==original
    finally:
        reopened.close()


def test_effective_invalidation_preserves_history_and_reference_only_freshness(client,auth,matter,make_pdf):
    included=upload(client,auth,matter,make_pdf)
    reference=client.post(f'/matters/{matter["id"]}/documents',headers=auth,files={'files':('ref.pdf',make_pdf(1),'application/pdf')}).json()[0]
    build=start(client,auth,matter)['result'];url=qa_url(matter,build)
    before=client.get(url+'/validation/state',headers=auth).json()
    client.patch(f'/documents/{reference["id"]}',headers=auth,json={'title':'Reference title'})
    assert not client.get(url+'/validation/state',headers=auth).json()['stale']
    original=client.get(url+'/combined.pdf',headers=auth).content
    client.patch(f'/documents/{included["id"]}',headers=auth,json={'title':'Effective change'})
    state=client.get(url+'/validation/state',headers=auth).json()
    assert state['stale'] and state['run']==before['run'] and state['can_download'] and not state['can_release']
    assert client.get(url+'/combined.pdf',headers=auth).content==original
    assert client.get(url+'/inspection/combined.pdf',headers=auth).status_code==200
    assert client.get(url+'/combined.pdf?release=true',headers=auth).status_code==409


def test_failed_or_missing_checks_gate_every_export_path_and_reviewer_cannot_pass_them(client,auth,matter,make_pdf):
    upload(client,auth,matter,make_pdf)
    build=start(client,auth,matter)['result'];url=qa_url(matter,build);store=client.app.state.store
    # Real output modified with internally valid PDF bytes; validation must compare
    # against the recorded map, not merely trust the file's ability to reopen.
    folder=store.exports_root(matter['id'])/build['id'];path=folder/'combined.pdf'
    with pymupdf.open(stream=read_bytes(path),filetype='pdf') as pdf:
        pdf.set_toc([[1,'Incorrect target',1]])
        changed=pdf.tobytes()
    write_bytes(path,changed)
    manifest=json.loads(read_bytes(folder/'export_manifest.json'))
    for f in manifest['files']:
        if f['name']=='combined.pdf':f.update(bytes=len(changed),sha256=hashlib.sha256(changed).hexdigest())
    write_bytes(folder/'export_manifest.json',json.dumps(manifest).encode())
    response=client.post(url+'/validation/run',headers=auth,json={})
    assert response.status_code==200
    state=response.json();rows={c['id']:c for c in state['run']['checks']}
    assert rows['navigation']['outcome']=='failed' and rows['files']['outcome']=='failed' # ZIP no longer matches
    update={**state['review'],'items':{k:True for k in state['review']['items']}}
    state=client.put(url+'/validation/review',headers=auth,json=update).json()
    assert not state['can_download'] and not state['can_release']
    for name in ['combined.pdf','filing-package.zip','index.csv']:
        assert client.get(url+'/'+name,headers=auth).status_code==409
    assert client.get(url+'/combined.pdf/office/docx',headers=auth).status_code==409
    assert client.post(url+'/combined.pdf/print',headers=auth,json={}).status_code==409
    client.patch(f'/matters/{matter["id"]}',headers=auth,json={'court_profile':'general-demo'})
    assert client.post(url+'/combined.pdf/protected',headers=auth,json={'password':'synthetic'}).status_code==409
    assert client.get(url+'/inspection/combined.pdf',headers=auth).status_code==200
    assert len(state['history'])==2
    # A successful subset never hides the full-run failure.
    subset=client.post(url+'/validation/run',headers=auth,json={'files':['index.csv']}).json()
    assert not subset['can_download'] and subset['run']['id']==state['run']['id']
    assert next(c for c in subset['history'][0]['checks'] if c['id']=='counts')['outcome']=='not-applicable'


def test_multivolume_and_part_labels_use_same_map(client,auth,matter,make_pdf):
    upload(client,auth,matter,make_pdf,pages=3)
    upload(client,auth,matter,make_pdf,name='second.pdf',pages=3)
    client.patch(f'/matters/{matter["id"]}',headers=auth,json={'max_pages_per_volume':4})
    store=client.app.state.store
    settings=store.get_output_marks(matter['id']);settings['part_extracts_enabled']=True
    store.set_output_marks(matter['id'],settings)
    build=start(client,auth,matter)['result'];state=client.get(qa_url(matter,build)+'/validation/state',headers=auth).json()
    assert state['can_download'],state['download_reasons']
    assert build['validation_contract']['combined.pdf']['labels']==['1','2','3','4']*2
    for name,spec in build['validation_contract'].items():
        with pymupdf.open(store.get_export_file(matter['id'],build['id'],name)) as pdf:
            assert len(pdf)==spec['pages']
            assert [p.get_label() for p in pdf]==spec['labels']
            assert pdf.get_toc()==spec['toc']


def test_preflight_exact_finding_notes_and_obsolete_revision(client,auth,matter,make_pdf):
    doc=upload(client,auth,matter,lambda n:make_pdf(n,text=''),pages=3)
    base=f'/matters/{matter["id"]}'
    checks=client.get(base+'/validate',headers=auth).json()
    finding=next(c for c in checks if c['code']=='UNSEARCHABLE_PAGE' and c['page_index']==1)
    url=base+f'/validation/findings/{finding["validation_run_id"]}/{finding["finding_id"]}'
    record=client.get(url,headers=auth).json();assert record['applicable']
    assert client.put(url,headers=auth,json={'note':'Intentional blank page','acknowledged':True}).status_code==200
    assert client.get(url,headers=auth).json()['review']['note']=='Intentional blank page'
    assert next(c for c in client.get(base+'/validate',headers=auth).json() if c['finding_id']==finding['finding_id'])['severity']=='warning'
    client.post(f'/documents/{doc["id"]}/pages/reorder',headers=auth,json={'order':[1,0,2],'expected_revision':1})
    assert not client.get(url,headers=auth).json()['applicable']
    assert client.put(url,headers=auth,json={'note':'Cannot acknowledge a different page','acknowledged':True}).status_code==409
    assert len(history(client.app.state.store,matter['id'],'preflight'))>=2


def test_legacy_contract_and_redaction_unavailability_never_pass(client,auth,matter,make_pdf):
    from test_pdf10_redaction import redact
    doc=upload(client,auth,matter,make_pdf,pages=2)
    assert redact(client,auth,doc).status_code==200
    build=start(client,auth,matter)['result'];url=qa_url(matter,build)
    state=client.get(url+'/validation/state',headers=auth).json()
    redaction=next(c for c in state['run']['checks'] if c['id']=='redaction')
    assert redaction['outcome']=='not-run' and redaction['blocking']
    assert state['can_download'] and not state['can_release']
    review={**state['review'],'items':{k:True for k in state['review']['items']}}
    assert not client.put(url+'/validation/review',headers=auth,json=review).json()['can_release']
    store=client.app.state.store;path=store.exports_root(matter['id'])/build['id']/'export_manifest.json'
    manifest=json.loads(read_bytes(path));manifest.pop('validation_contract')
    write_bytes(path,json.dumps(manifest).encode())
    legacy=client.post(url+'/validation/run',headers=auth,json={}).json()
    assert not legacy['can_download']
    assert all(c['outcome']=='not-run' for c in legacy['run']['checks'] if c['id'] in ('counts','navigation','labels'))
