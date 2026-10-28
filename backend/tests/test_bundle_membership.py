"""B01A: real membership, exact map/output, guards and preserved sources."""
import csv
import io
import sqlite3
import pymupdf
import pytest
from app.engine import build_bundle, bundle_preview
from app.pdf_ops import ProcessingError
from app.storage import Store
from app.portable import create_package, restore_package
from test_api import upload, wait_for_job


def state(client, auth, matter):
    response = client.get(f'/matters/{matter["id"]}/membership', headers=auth)
    assert response.status_code == 200, response.text
    return response.json()


def apply(client, auth, matter, ids, snapshot=None):
    return client.put(f'/matters/{matter["id"]}/membership', headers=auth, json={
        'included_ids': ids, 'expected_snapshot': snapshot or state(client, auth, matter)['input_snapshot']})


def test_import_stages_reference_only_and_explicit_inclusion_persists(client, auth, matter, make_pdf):
    original = make_pdf(3)
    doc = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
        files={'files': ('reference.pdf', original, 'application/pdf')}).json()[0]
    assert doc['bundle_included'] == 0
    before = state(client, auth, matter)
    assert before['included_documents'] == 0 and before['reference_source_pages'] == 3
    assert any(c['code'] == 'NO_DOCUMENTS' for c in client.get(f'/matters/{matter["id"]}/validate', headers=auth).json())
    included = apply(client, auth, matter, [doc['id']])
    assert included.status_code == 200 and included.json()['included_source_pages'] == 3
    assert state(client, auth, matter)['revision'] == 2
    excluded = apply(client, auth, matter, [])
    assert excluded.status_code == 200
    assert client.get(f'/documents/{doc["id"]}/original', headers=auth).content == original
    assert client.post(f'/documents/{doc["id"]}/pages/rotate', headers=auth,
        json={'page_index':0,'degrees':90,'expected_revision':1}).status_code == 200
    assert client.get(f'/documents/{doc["id"]}/pdf', headers=auth).content.startswith(b'%PDF')
    store = client.app.state.store
    with store.connect() as con:
        assert con.execute('SELECT bundle_included FROM documents WHERE id=?', (doc['id'],)).fetchone()[0] == 0
    # Reopening the persisted store must not re-include staged documents.
    root = store.root; store.close()
    reopened = Store(root)
    try:
        assert reopened.bundle_membership(matter['id'])['reference_documents'] == 1
        assert reopened.get_document(doc['id'])['revision'] == 2
    finally:
        reopened.close()


def test_stale_apply_and_foreign_ids_are_rejected_atomically(client, auth, matter, make_pdf):
    first = upload(client, auth, matter, make_pdf)
    snapshot = state(client, auth, matter)['input_snapshot']
    client.patch(f'/documents/{first["id"]}', headers=auth, json={'title':'New working title'})
    assert apply(client, auth, matter, [], snapshot).status_code == 409
    assert apply(client, auth, matter, ['foreign-id']).status_code == 409
    assert apply(client, auth, matter, [first['id'], first['id']]).status_code == 409
    assert state(client, auth, matter)['included_documents'] == 1


def test_exclusion_drives_multi_volume_pdf_csv_bookmarks_and_preserves_prior(client, auth, matter, make_pdf):
    a = upload(client, auth, matter, lambda n:make_pdf(n, text='Included alpha'), pages=4)
    b = upload(client, auth, matter, lambda n:make_pdf(n, text='PRIVATE reference beta'), name='beta.pdf', pages=3)
    c = upload(client, auth, matter, lambda n:make_pdf(n, text='Included gamma'), name='gamma.pdf', pages=4)
    store=client.app.state.store
    prior=build_bundle(store,matter['id'],'11111111111111111111111111111111',lambda *a:None,lambda:False)
    prior_path=store.exports_root(matter['id'])/'11111111111111111111111111111111'/'combined.pdf'
    old_bytes=prior_path.read_bytes()
    assert apply(client,auth,matter,[a['id'],c['id']]).status_code==200
    client.patch(f'/matters/{matter["id"]}',headers=auth,json={'max_pages_per_volume':5})
    preview=bundle_preview(store,matter['id'],5,True)
    assert preview['combined_pages']==10
    rows=[r for v in preview['volumes'] for r in v['rows']]
    assert [r['id'] for r in rows]==[a['id'],c['id']]
    assert [(r['combined_start_page'],r['combined_end_page']) for r in rows]==[(2,5),(7,10)]
    output=build_bundle(store,matter['id'],'22222222222222222222222222222222',lambda *a:None,lambda:False)
    folder=store.exports_root(matter['id'])/'22222222222222222222222222222222'
    with pymupdf.open(folder/'combined.pdf') as pdf:
        assert len(pdf)==10
        assert 'PRIVATE reference beta' not in ''.join(p.get_text() for p in pdf)
        for index,start in [(0,1),(5,6)]:
            assert any(link.get('page')==start for link in pdf[index].get_links())
        assert all(1<=entry[2]<=10 for entry in pdf.get_toc())
    csv_path=next(folder.glob('*.csv'))
    csv_rows=list(csv.DictReader(io.StringIO(csv_path.read_text(encoding='utf-8-sig'))))
    assert len(csv_rows)==2
    assert [(int(r['volume']),int(r['start_page']),int(r['end_page'])) for r in csv_rows]==[(1,2,5),(2,2,5)]
    assert output['bundle_membership']['selection_mode']=='whole_documents'
    assert [d['document_id'] for d in output['bundle_membership']['documents']]==[a['id'],c['id']]
    assert {d['document_id'] for d in output['input_revisions']}=={a['id'],c['id']}
    assert store.list_exports(matter['id'])[0]['id'] in ('22222222222222222222222222222222','11111111111111111111111111111111')
    assert next(e for e in store.list_exports(matter['id']) if e['id']=='11111111111111111111111111111111')['stale']
    assert prior_path.read_bytes()==old_bytes
    assert store.original_path(b).exists()


def test_excluded_link_blocks_and_bookmark_omission_is_reported(client, auth, matter, make_pdf):
    a=upload(client,auth,matter,make_pdf);b=upload(client,auth,matter,make_pdf,name='reference.pdf')
    store=client.app.state.store
    store.set_manual_bookmarks(matter['id'],[dict(id='bm',title='Reference bookmark',document_id=b['id'],page_index=0,target_revision=1,parent_id=None,position=0)])
    store.set_reviewed_links(matter['id'],[dict(id='link',title='Go to reference',source_document_id=a['id'],source_page_index=0,source_revision=1,rect_x=.1,rect_y=.1,rect_width=.2,rect_height=.1,kind='internal',target_document_id=b['id'],target_page_index=0,target_revision=1,uri='',position=0)])
    assert apply(client,auth,matter,[a['id']]).status_code==200
    checks=client.get(f'/matters/{matter["id"]}/validate',headers=auth).json()
    assert any(c['code']=='EXCLUDED_LINK_DESTINATION' and c['severity']=='error' for c in checks)
    assert any(c['code']=='EXCLUDED_BOOKMARK_OMITTED' for c in checks)
    assert bundle_preview(store,matter['id'],300,True)['manual_bookmarks'][0]['resolved'] is False
    with pytest.raises(ProcessingError,match='excluded material'):
        build_bundle(store,matter['id'],'blocked',lambda *a:None,lambda:False)
    store.set_reviewed_links(matter['id'],[])
    build_bundle(store,matter['id'],'33333333333333333333333333333333',lambda *a:None,lambda:False)
    with pymupdf.open(store.exports_root(matter['id'])/'33333333333333333333333333333333'/'combined.pdf') as pdf:
        assert all('Reference bookmark' not in row[1] for row in pdf.get_toc())


def test_membership_undo_backup_and_empty_build(client, auth, matter, make_pdf, tmp_path):
    doc=upload(client,auth,matter,make_pdf)
    store=client.app.state.store
    assert apply(client,auth,matter,[]).status_code==200
    store.traverse_history(matter['id'])
    assert store.get_bundle_documents(matter['id'])[0]['id']==doc['id']
    store.traverse_history(matter['id'],redo=True)
    assert store.get_bundle_documents(matter['id'])==[]
    package=tmp_path/'membership.cbmatter'
    create_package(store,matter['id'],package)
    restored=restore_package(store,package)
    assert store.get_documents(restored['id'])[0]['bundle_included']==0
    assert store.bundle_membership(restored['id'])['revision']==store.bundle_membership(matter['id'])['revision']
    with pytest.raises(ProcessingError,match='Include at least one'):
        build_bundle(store,matter['id'],'empty',lambda *a:None,lambda:False)


def test_queued_build_rejects_membership_change(client, auth, matter, make_pdf, monkeypatch):
    upload(client,auth,matter,make_pdf)
    pending=[]
    def capture(matter_id,kind,task,**kwargs):
        pending.append(task)
        return {'id':'membership-queued'}
    monkeypatch.setattr(client.app.state.jobs,'submit',capture)
    assert client.post(f'/matters/{matter["id"]}/build',headers=auth).status_code==202
    assert apply(client,auth,matter,[]).status_code==200
    with pytest.raises(ProcessingError,match='inputs changed while queued'):
        pending[0]('job',lambda *a:None,lambda:False)


def test_reference_only_signature_does_not_require_build_acknowledgment(client, auth, matter, make_pdf):
    upload(client,auth,matter,make_pdf)
    reference=client.post(f'/matters/{matter["id"]}/documents',headers=auth,
        files={'files':('reference.pdf',make_pdf(1),'application/pdf')}).json()[0]
    client.patch(f'/documents/{reference["id"]}',headers=auth,json={'visible_signature_status':'present'})
    started=client.post(f'/matters/{matter["id"]}/build',headers=auth)
    assert started.status_code==202,started.text
    assert wait_for_job(client,auth,started.json()['id'])['status']=='completed'
