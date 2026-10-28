"""02B artifact permissions, frozen inputs and revision-qualified findings."""
import hashlib
import pymupdf
import pytest
from app.pdf_ops import ProcessingError
from test_api import upload, wait_for_job


def test_original_artifact_is_authenticated_immutable_and_separate(client, auth, matter, make_pdf):
    source = make_pdf(3)
    doc = upload(client, auth, matter, lambda _: source, pages=3)
    path = f'/documents/{doc["id"]}/original'
    assert client.get(path).status_code == 401
    info = client.get(path + '-info', headers=auth).json()
    assert info['artifact'] == 'original' and info['read_only']
    assert info['page_count'] == 3 and info['sha256'] == hashlib.sha256(source).hexdigest()
    assert not any('path' in key for key in info)
    changed = client.post(f'/documents/{doc["id"]}/pages/delete', headers=auth,
                          json={'page_index': 1, 'expected_revision': doc['revision']})
    assert changed.status_code == 200 and changed.json()['page_count'] == 2
    assert client.get(path, headers=auth).content == source
    assert client.get(path, headers={**auth, 'Range': 'bytes=0-9'}).content == source[:10]
    for method in ('post', 'put', 'patch', 'delete'):
        assert getattr(client, method)(path, headers=auth).status_code == 405
    key = f'original:{doc["id"]}'
    state = {'key':key,'page':3,'x':0,'y':0.2,'zoom':'100','mode':'single'}
    assert client.put('/reader-position', headers=auth,json=state).status_code == 200
    assert client.get('/reader-position', headers=auth,params={'key':key}).json()['page'] == 3
    client.app.state.store.original_path(doc).write_bytes(source+b'changed')
    assert client.get(path, headers=auth).status_code == 409


def test_image_original_returns_image_not_converted_pdf(client, auth, matter):
    with pymupdf.open() as pdf:
        data = pdf.new_page(width=64,height=64).get_pixmap().tobytes('png')
    response = client.post(f'/matters/{matter["id"]}/documents?included=true', headers=auth,
                           files={'files':('source.png',data,'image/png')})
    doc = response.json()[0]
    info = client.get(f'/documents/{doc["id"]}/original-info',headers=auth).json()
    assert info['media_type'] == 'image/png' and info['page_count'] is None
    image = client.get(f'/documents/{doc["id"]}/original',headers=auth)
    assert image.content == data and image.headers['content-type']=='image/png'
    assert client.get(f'/documents/{doc["id"]}/pdf',headers=auth).content.startswith(b'%PDF')


def test_preflight_records_exact_revision_position_and_snapshot(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, lambda n:make_pdf(n,text=''), pages=3)
    path = f'/matters/{matter["id"]}'
    checks = client.get(path+'/validate',headers=auth).json()
    findings = [c for c in checks if c['code']=='UNSEARCHABLE_PAGE']
    assert [c['page_index'] for c in findings] == [0,1,2]
    assert all(c['target_revision']==doc['revision'] and c['artifact']=='working' for c in findings)
    assert len({c['finding_id'] for c in findings}) == 3
    assert all('page_index' not in c for c in checks if c['document_id'] is None)
    snapshot = findings[0]['input_snapshot']
    preview = client.post(path+'/bundle-preview',headers=auth,json={'max_pages_per_volume':300,'keep_groups_intact':True}).json()
    assert preview['input_snapshot'] == snapshot
    assert preview['settings_snapshot']['max_pages_per_volume']==300
    client.post(f'/documents/{doc["id"]}/pages/reorder',headers=auth,json={'order':[2,0,1],'expected_revision':doc['revision']})
    fresh = client.get(path+'/validate',headers=auth).json()
    current = next(c for c in fresh if c['code']=='UNSEARCHABLE_PAGE')
    assert current['target_revision'] != findings[0]['target_revision']
    assert current['input_snapshot'] != snapshot
    assert findings[0]['page_index']==0  # An old position was not relabelled as stable identity.


def test_queued_ocr_rejects_changed_revision_without_committing(client, auth, matter, make_pdf, monkeypatch):
    doc = upload(client,auth,matter,make_pdf)
    pending = []
    def capture(matter_id, kind, task, **kwargs):
        pending.append(task)
        return {'id':'queued-test','kind':kind}
    monkeypatch.setattr(client.app.state.jobs,'submit',capture)
    response = client.post(f'/documents/{doc["id"]}/ocr',headers=auth,json={'language':'eng','expected_revision':doc['revision']})
    assert response.status_code==202
    updated = client.post(f'/documents/{doc["id"]}/pages/rotate',headers=auth,json={'page_index':0,'degrees':90,'expected_revision':doc['revision']}).json()
    before = client.app.state.store.sha256(client.app.state.store.working_path(updated))
    with pytest.raises(ProcessingError,match='changed before OCR'):
        pending[0]('job',lambda *args:None,lambda:False)
    assert client.app.state.store.get_document(doc['id'])['revision']==updated['revision']
    assert client.app.state.store.sha256(client.app.state.store.working_path(updated))==before
    assert client.post(f'/documents/{doc["id"]}/ocr',headers=auth,json={'expected_revision':doc['revision']}).status_code==409


def test_queued_build_rejects_changed_settings_and_retained_build_stays_frozen(client, auth, matter, make_pdf, monkeypatch):
    upload(client,auth,matter,make_pdf)
    path=f'/matters/{matter["id"]}'
    job=wait_for_job(client,auth,client.post(path+'/build',headers=auth).json()['id'])
    assert job['status']=='completed'
    manifest=client.get(path+'/exports',headers=auth).json()[0]
    pdf=client.get(path+f'/exports/{manifest["id"]}/combined.pdf',headers=auth).content
    pending=[]
    def capture(matter_id,kind,task,**kwargs):
        pending.append(task)
        return {'id':'queued-test'}
    monkeypatch.setattr(client.app.state.jobs,'submit',capture)
    assert client.post(path+'/build',headers=auth).status_code==202
    assert client.patch(path,headers=auth,json={'max_pages_per_volume':200}).status_code==200
    with pytest.raises(ProcessingError,match='inputs changed while queued'):
        pending[0]('job',lambda *args:None,lambda:False)
    retained=client.get(path+'/exports',headers=auth).json()[0]
    assert retained['stale']
    assert retained['page_map_snapshot']==manifest['page_map_snapshot']
    assert retained['build_input_digest']==manifest['build_input_digest']
    assert client.get(path+f'/exports/{manifest["id"]}/combined.pdf',headers=auth).content==pdf
    assert client.delete(path+f'/exports/{manifest["id"]}/combined.pdf',headers=auth).status_code==405
