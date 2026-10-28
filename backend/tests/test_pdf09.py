import hashlib
import time

import pymupdf
from fastapi.testclient import TestClient

from app.file_io import read_bytes
from app.main import create_app


def _encrypted_pdf(plain: bytes, *, permissions: int = 4095) -> bytes:
    with pymupdf.open(stream=plain, filetype='pdf') as document:
        return document.tobytes(encryption=pymupdf.PDF_ENCRYPT_AES_256,
                                owner_pw='synthetic owner secret',
                                user_pw='synthetic input secret',
                                permissions=permissions)


def _finish(client, auth, response):
    response.raise_for_status()
    job = response.json()
    deadline = time.monotonic() + 20
    while job['status'] in ('queued', 'running') and time.monotonic() < deadline:
        time.sleep(.03)
        job = client.get(f'/jobs/{job["id"]}', headers=auth).json()
    assert job['status'] == 'completed', job.get('error')
    return job


def test_protected_pdf_import_export_and_profile_restriction(tmp_path, make_pdf):
    root = tmp_path / 'protected'
    auth = {'Authorization': 'Bearer pdf09-test'}
    plain = make_pdf(2, 'SYNTHETIC PASSWORD PDF')
    encrypted_source = _encrypted_pdf(plain)
    with TestClient(create_app(root, 'pdf09-test', require_vault=True)) as client:
        client.post('/vault/setup', headers=auth,
                    json={'password': 'synthetic local vault password'}).raise_for_status()
        matter = client.post('/matters', headers=auth, json={
            'name': 'PDF-09 synthetic', 'court_profile': 'general-demo'}).json()
        path = f'/matters/{matter["id"]}/documents/raw?included=true'
        headers = {**auth, 'Content-Type': 'application/octet-stream',
                   'X-CourtBundle-Filename': 'locked.pdf'}
        missing = client.post(path, headers=headers, content=encrypted_source)
        assert missing.status_code == 422 and 'password required' in missing.text
        wrong = client.post(path, headers={**headers, 'X-CourtBundle-Pdf-Password': 'incorrect'},
                            content=encrypted_source)
        assert wrong.status_code == 422 and 'Incorrect PDF password' in wrong.text
        assert client.get(f'/matters/{matter["id"]}/documents', headers=auth).json() == []
        imported = client.post(path, headers={**headers,
            'X-CourtBundle-Pdf-Password': 'synthetic input secret'}, content=encrypted_source)
        assert imported.status_code == 201, imported.text
        document = imported.json()[0]
        store = client.app.state.store
        original = store.original_path(document)
        working = store.working_path(document)
        assert read_bytes(original) == encrypted_source
        assert document['source_sha256'] == hashlib.sha256(encrypted_source).hexdigest()
        with pymupdf.open(stream=read_bytes(working), filetype='pdf') as check:
            assert not check.needs_pass and check.page_count == 2
            assert 'SYNTHETIC PASSWORD PDF' in check[0].get_text()
        assert original.read_bytes().startswith(b'CBENC1\n')
        assert working.read_bytes().startswith(b'CBENC1\n')
        assert all(b'synthetic input secret' not in file.read_bytes()
                   for file in root.rglob('*') if file.is_file())
        build = _finish(client, auth, client.post(f'/matters/{matter["id"]}/build', headers=auth))
        export_id = build['result']['id']
        protected = client.post(
            f'/matters/{matter["id"]}/exports/{export_id}/combined.pdf/protected',
            headers=auth, json={'password': 'synthetic output secret'})
        assert protected.status_code == 200, protected.text
        with pymupdf.open(stream=protected.content, filetype='pdf') as output:
            assert output.needs_pass
            assert output.authenticate('') == 0
            assert output.authenticate('incorrect') == 0
            assert output.authenticate('synthetic output secret')
            assert output.page_count == build['result']['pages']
        assert b'synthetic output secret' not in protected.content
        assert client.post(
            f'/matters/{matter["id"]}/exports/{export_id}/combined.pdf/protected',
            headers=auth, json={}).status_code == 422
        assert all(b'synthetic output secret' not in file.read_bytes()
                   for file in root.rglob('*') if file.is_file())
        assert client.post(f'/matters/{matter["id"]}/exports/{export_id}/filing-package.zip/protected',
                           headers=auth, json={'password': 'secret'}).status_code == 422

        restricted = client.post('/matters', headers=auth, json={
            'name': 'Illustrative profile', 'court_profile': 'delhi-hc-illustrative'}).json()
        uploaded = client.post(f'/matters/{restricted["id"]}/documents/raw?included=true',
            headers={**auth, 'Content-Type': 'application/octet-stream',
                     'X-CourtBundle-Filename': 'plain.pdf'}, content=plain)
        assert uploaded.status_code == 201, uploaded.text
        restricted_build = _finish(client, auth,
                                   client.post(f'/matters/{restricted["id"]}/build', headers=auth))
        denied = client.post(
            f'/matters/{restricted["id"]}/exports/{restricted_build["result"]["id"]}/combined.pdf/protected',
            headers=auth, json={'password': 'synthetic output secret'})
        assert denied.status_code == 409
        assert 'illustrative profile' in denied.text


def test_protected_pdf_respects_user_permissions(tmp_path, make_pdf):
    auth = {'Authorization': 'Bearer pdf09-test'}
    restricted = _encrypted_pdf(make_pdf(1), permissions=pymupdf.PDF_PERM_PRINT)
    with TestClient(create_app(tmp_path, 'pdf09-test')) as client:
        matter = client.post('/matters', headers=auth,
                             json={'name': 'Permissions test', 'court_profile': 'general-demo'}).json()
        response = client.post(f'/matters/{matter["id"]}/documents/raw?included=true',
            headers={**auth, 'Content-Type': 'application/octet-stream',
                     'X-CourtBundle-Filename': 'restricted.pdf',
                     'X-CourtBundle-Pdf-Password': 'synthetic input secret'}, content=restricted)
        assert response.status_code == 422
        assert 'permissions do not allow' in response.text
        assert client.get(f'/matters/{matter["id"]}/documents', headers=auth).json() == []


def test_protected_pdf_signature_requires_review_before_working_copy(tmp_path):
    source = tmp_path / 'signed.pdf'
    with pymupdf.open() as pdf:
        page = pdf.new_page()
        widget = pymupdf.Widget()
        widget.field_name = 'synthetic_signature'
        widget.field_type = pymupdf.PDF_WIDGET_TYPE_SIGNATURE
        widget.rect = pymupdf.Rect(70, 120, 250, 175)
        page.add_widget(widget)
        pdf.save(source)
    with pymupdf.open(source) as pdf:
        widget = next(pdf[0].widgets())
        xref = pdf.get_new_xref()
        pdf.update_object(xref, '<< /Type /Sig /ByteRange [0 100 200 300] /Contents <01020304> >>')
        pdf.xref_set_key(widget.xref, 'V', f'{xref} 0 R')
        pdf.save(source, incremental=True, encryption=pymupdf.PDF_ENCRYPT_KEEP)
    encrypted = _encrypted_pdf(source.read_bytes())
    auth = {'Authorization': 'Bearer pdf09-test'}
    with TestClient(create_app(tmp_path / 'store', 'pdf09-test', require_vault=True)) as client:
        client.post('/vault/setup', headers=auth,
                    json={'password': 'synthetic vault password'}).raise_for_status()
        matter = client.post('/matters', headers=auth, json={
            'name': 'Signed protected PDF', 'court_profile': 'general-demo'}).json()
        endpoint = f'/matters/{matter["id"]}/documents/raw?included=true'
        headers = {**auth, 'Content-Type': 'application/octet-stream',
                   'X-CourtBundle-Filename': 'signed.pdf',
                   'X-CourtBundle-Pdf-Password': 'synthetic input secret'}
        blocked = client.post(endpoint, headers=headers, content=encrypted)
        assert blocked.status_code == 409
        assert 'signature review required' in blocked.text
        assert client.get(f'/matters/{matter["id"]}/documents', headers=auth).json() == []
        accepted = client.post(endpoint, headers={**headers,
            'X-CourtBundle-Signature-Acknowledge': 'true'}, content=encrypted)
        assert accepted.status_code == 201, accepted.text
        document = accepted.json()[0]
        assert document['certificate_signature_status'] == 'detected'
        assert read_bytes(client.app.state.store.original_path(document)) == encrypted
