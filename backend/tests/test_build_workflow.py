"""B01B: actual assembly jobs, recovery, frozen output and effective freshness."""
import hashlib
import threading
import pymupdf
from fastapi.testclient import TestClient
from app.main import create_app
from app.storage import Store
from test_api import upload, wait_for_job


def start(client, auth, matter):
    response = client.post(f'/matters/{matter["id"]}/build', headers=auth)
    assert response.status_code == 202, response.text
    return wait_for_job(client, auth, response.json()['id'])


def test_completed_export_reopens_and_reference_edits_do_not_stale(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, pages=3)
    ref = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
        files={'files': ('reference.pdf', make_pdf(2), 'application/pdf')}).json()[0]
    first = start(client, auth, matter)
    assert first['status'] == 'completed', first
    build = first['result']; base = f'/matters/{matter["id"]}/exports'
    original = client.get(f'/documents/{doc["id"]}/original', headers=auth).content
    pdf = client.get(f'{base}/{build["id"]}/combined.pdf', headers=auth).content
    assert hashlib.sha256(pdf).hexdigest() == next(f['sha256'] for f in build['files'] if f['name']=='combined.pdf')
    with pymupdf.open(stream=pdf, filetype='pdf') as opened:
        assert opened.page_count == build['pages'] == 4
        assert opened.get_toc()
        assert all(1 <= row[2] <= opened.page_count for row in opened.get_toc())
        links = [link for page in opened for link in page.get_links()]
        assert links and all(0 <= link['page'] < opened.page_count for link in links if link['kind']==pymupdf.LINK_GOTO)
    client.patch(f'/documents/{ref["id"]}', headers=auth, json={'title':'Unrelated reference title'})
    assert not client.get(base, headers=auth).json()[0]['stale']
    client.post(f'/documents/{ref["id"]}/pages/rotate', headers=auth,
        json={'page_index':0,'degrees':90,'expected_revision':1})
    assert not client.get(base, headers=auth).json()[0]['stale']
    client.patch(f'/documents/{doc["id"]}', headers=auth, json={'title':'Changed included title'})
    assert client.get(base, headers=auth).json()[0]['stale']
    second = start(client, auth, matter)
    assert second['status']=='completed' and second['id'] != first['id']
    assert client.get(f'{base}/{build["id"]}/combined.pdf', headers=auth).content == pdf
    assert client.get(f'/documents/{doc["id"]}/original', headers=auth).content == original
    root = client.app.state.store.root
    # A fresh Store reads persisted manifests and jobs without the originating UI.
    client.app.state.store.close()
    reopened = Store(root)
    try:
        assert reopened.get_job(second['id'])['status']=='completed'
        assert {e['id'] for e in reopened.list_exports(matter['id'])} == {first['id'],second['id']}
        assert reopened.get_export_file(matter['id'],first['id'],'combined.pdf').read_bytes() == pdf
    finally:
        reopened.close()


def test_real_build_cancel_and_failure_retain_previous_bytes(client, auth, matter, make_pdf, monkeypatch):
    import app.engine as engine
    doc = upload(client, auth, matter, make_pdf, pages=3)
    first = start(client, auth, matter)
    assert first['status']=='completed'
    store = client.app.state.store
    prior = store.get_export_file(matter['id'],first['id'],'combined.pdf').read_bytes()
    original = store.original_path(store.get_document(doc['id'])).read_bytes()
    entered, release = threading.Event(), threading.Event()
    actual = engine.render_volume
    def gated(*args, **kwargs):
        entered.set()
        assert release.wait(5), 'test did not release actual volume assembly'
        return actual(*args, **kwargs)
    monkeypatch.setattr(engine, 'render_volume', gated)
    submitted = client.post(f'/matters/{matter["id"]}/build', headers=auth).json()
    assert entered.wait(5)
    try:
        response = client.post(f'/jobs/{submitted["id"]}/cancel', headers=auth)
        assert response.json()['cancel_requested']
        assert client.get(f'/jobs/{submitted["id"]}', headers=auth).json()['status']=='running'
    finally:
        release.set()
    stopped = wait_for_job(client,auth,submitted['id'])
    assert stopped['status']=='cancelled'
    assert len(store.list_exports(matter['id']))==1
    monkeypatch.setattr(engine,'render_volume',actual)
    client.patch(f'/matters/{matter["id"]}',headers=auth,json={'max_pages_per_volume':2})
    failed=start(client,auth,matter)
    assert failed['status']=='failed' and failed['error']
    client.patch(f'/matters/{matter["id"]}',headers=auth,json={'max_pages_per_volume':300})
    retry=start(client,auth,matter)
    assert retry['status']=='completed'
    assert store.get_export_file(matter['id'],first['id'],'combined.pdf').read_bytes()==prior
    assert store.original_path(store.get_document(doc['id'])).read_bytes()==original
    assert not list(store.exports_root(matter['id']).glob('.build-*'))


def test_interrupted_build_retry_produces_new_artifact(client, auth, matter, make_pdf):
    upload(client,auth,matter,make_pdf)
    first=start(client,auth,matter)
    assert first['status']=='completed'
    store=client.app.state.store
    store.save_job(dict(id='interrupted-build',matter_id=matter['id'],kind='build',target_id=None,
        status='interrupted',progress=35,message='Engine restart',error=None,result=None))
    retry=client.post('/jobs/interrupted-build/retry',headers=auth)
    assert retry.status_code==202
    done=wait_for_job(client,auth,retry.json()['id'])
    assert done['status']=='completed' and done['id']!=first['id']
    assert client.post('/jobs/interrupted-build/retry',headers=auth).status_code==409
    assert len(store.list_exports(matter['id']))==2
