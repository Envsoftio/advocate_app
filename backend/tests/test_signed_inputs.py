import json
import time

import pymupdf


def _pdf_with_signature_dictionary(tmp_path, complete: bool) -> bytes:
    path = tmp_path / ('signed-indicators.pdf' if complete else 'inconclusive-signature.pdf')
    with pymupdf.open() as document:
        page = document.new_page(width=595, height=842)
        page.insert_text((70, 90), 'Synthetic signature inspection fixture', fontsize=16)
        widget = pymupdf.Widget()
        widget.field_name = 'synthetic_signature'
        widget.field_type = pymupdf.PDF_WIDGET_TYPE_SIGNATURE
        widget.rect = pymupdf.Rect(70, 120, 250, 175)
        page.add_widget(widget)
        document.save(path)
    with pymupdf.open(path) as document:
        widget = next(document[0].widgets())
        signature_xref = document.get_new_xref()
        if complete:
            raw = ('<< /Type /Sig /Filter /Adobe.PPKLite '
                   '/SubFilter /adbe.pkcs7.detached '
                   '/ByteRange [0 100 200 300] /Contents <01020304> >>')
        else:
            raw = '<< /Type /Sig /Filter /SyntheticIncomplete >>'
        document.update_object(signature_xref, raw)
        document.xref_set_key(widget.xref, 'V', f'{signature_xref} 0 R')
        document.save(path, incremental=True, encryption=pymupdf.PDF_ENCRYPT_KEEP)
    return path.read_bytes()


def _upload(client, auth, matter_id, name, content):
    response = client.post(f'/matters/{matter_id}/documents?included=true', headers=auth,
        files=[('files', (name, content, 'application/pdf'))])
    assert response.status_code == 201, response.text
    return response.json()[0]


def _wait(client, auth, job_id):
    for _ in range(300):
        job = client.get(f'/jobs/{job_id}', headers=auth).json()
        if job['status'] in ('completed', 'failed', 'cancelled'):
            return job
        time.sleep(.02)
    raise AssertionError('job did not finish')


def test_signature_statuses_are_distinct_and_originals_are_preserved(
        client, auth, matter, make_pdf, tmp_path):
    store = client.app.state.store
    unsigned_bytes = make_pdf(1, 'Unsigned synthetic fixture')
    unsigned = _upload(client, auth, matter['id'], 'unsigned.pdf', unsigned_bytes)
    assert unsigned['signature_status'] == 'no_certificate_signature_detected'
    assert unsigned['certificate_signature_status'] == 'not_detected'
    assert store.sha256(store.original_path(unsigned)) == unsigned['source_sha256']
    assert store.sha256(store.working_path(unsigned)) == unsigned['source_sha256']

    reviewed_unsigned = client.patch(f'/documents/{unsigned["id"]}', headers=auth,
        json={'visible_signature_status': 'absent'})
    assert reviewed_unsigned.status_code == 200, reviewed_unsigned.text
    assert reviewed_unsigned.json()['signature_status'] == 'unsigned'

    scanned = _upload(client, auth, matter['id'], 'scanned-signature.pdf',
                      make_pdf(1, 'Visible autograph image represented synthetically'))
    visible = client.patch(f'/documents/{scanned["id"]}', headers=auth,
        json={'visible_signature_status': 'present'})
    assert visible.status_code == 200, visible.text
    assert visible.json()['signature_status'] == 'visible_signature_only'

    signed_bytes = _pdf_with_signature_dictionary(tmp_path, complete=True)
    signed = _upload(client, auth, matter['id'], 'certificate-signed.pdf', signed_bytes)
    assert signed['signature_status'] == 'certificate_signature_detected'
    assert signed['certificate_signature_status'] == 'detected'
    assert signed['certificate_signature_count'] == 1
    assert store.sha256(store.original_path(signed)) == signed['source_sha256']
    assert store.sha256(store.working_path(signed)) == signed['source_sha256']

    inconclusive = _upload(client, auth, matter['id'], 'inconclusive.pdf',
                           _pdf_with_signature_dictionary(tmp_path, complete=False))
    assert inconclusive['signature_status'] == 'inspection_inconclusive'
    assert inconclusive['certificate_signature_status'] == 'unknown'

    statuses = {item['signature_status'] for item in
                client.get(f'/matters/{matter["id"]}/documents', headers=auth).json()}
    assert {'unsigned', 'visible_signature_only', 'certificate_signature_detected',
            'inspection_inconclusive'} <= statuses


def test_signed_transformations_require_acknowledgement_and_record_build_warning(
        client, auth, matter, tmp_path):
    content = _pdf_with_signature_dictionary(tmp_path, complete=True)
    document = _upload(client, auth, matter['id'], 'certificate-signed.pdf', content)
    store = client.app.state.store
    original_hash = store.sha256(store.original_path(document))

    denied = client.post(f'/documents/{document["id"]}/pages/rotate', headers=auth,
                         json={'page_index': 0, 'degrees': 90})
    assert denied.status_code == 409
    assert 'Signature-risk acknowledgment required' in denied.json()['detail']
    assert store.get_document(document['id'])['revision'] == 1

    for invalid_acknowledgment in (None, {}, 'true', 1):
        denied_overlay = client.post(f'/documents/{document["id"]}/pages/text-overlay',
            headers=auth, json={'page_index': 0, 'x': .2, 'y': .2, 'text': 'TEST',
                                'acknowledge_signature_risk': invalid_acknowledgment})
        assert denied_overlay.status_code == 409
    assert store.get_document(document['id'])['revision'] == 1

    review = client.post(f'/documents/{document["id"]}/pages/text-overlay',
        headers={**auth, 'X-CourtBundle-Review-Protocol': 'challenge'},
        json={'page_index': 0, 'x': .2, 'y': .2, 'text': 'TEST',
              'acknowledge_signature_risk': False})
    assert review.status_code == 200
    assert review.json()['review_required'] is True
    assert 'Signature-risk acknowledgment required' in review.json()['detail']
    assert store.get_document(document['id'])['revision'] == 1

    replacement = {'page_index': 0, 'span_index': 0, 'expected_revision': 1,
                   'expected_text': 'Synthetic signature inspection fixture',
                   'replacement': 'Reviewed text', 'acknowledge_signature_risk': False}
    move = {'page_index': 0, 'span_index': 0, 'expected_revision': 1,
            'expected_text': replacement['expected_text'], 'vertical_offset': .02,
            'acknowledge_signature_risk': False}
    for endpoint, payload in (('replace-text-run', replacement),
                              ('paragraph-trial', replacement), ('move-text-run', move)):
        review = client.post(f'/documents/{document["id"]}/pages/{endpoint}',
            headers={**auth, 'X-CourtBundle-Review-Protocol': 'challenge'}, json=payload)
        assert review.status_code == 200, review.text
        assert review.json()['review_required'] is True
        assert store.get_document(document['id'])['revision'] == 1

    allowed = client.post(f'/documents/{document["id"]}/pages/rotate', headers=auth,
        json={'page_index': 0, 'degrees': 90, 'acknowledge_signature_risk': True})
    assert allowed.status_code == 200, allowed.text
    assert allowed.json()['revision'] == 2
    assert store.sha256(store.original_path(document)) == original_hash

    ocr_denied = client.post(f'/documents/{document["id"]}/ocr', headers=auth)
    assert ocr_denied.status_code == 409

    build_denied = client.post(f'/matters/{matter["id"]}/build', headers=auth)
    assert build_denied.status_code == 409
    build = client.post(f'/matters/{matter["id"]}/build', headers=auth,
                        json={'acknowledge_signature_risk': True})
    assert build.status_code == 202, build.text
    completed = _wait(client, auth, build.json()['id'])
    assert completed['status'] == 'completed', completed
    assert completed['result']['signature_risk_acknowledged'] is True
    assert completed['result']['signature_inputs'][0]['signature_status'] == \
        'certificate_signature_detected'

    report_path = store.exports_root(matter['id']) / completed['result']['id'] / 'validation-report.json'
    report = json.loads(report_path.read_text())
    assert report['signature_risk_acknowledged'] is True
    assert any(check['code'] == 'CERTIFICATE_SIGNATURE_INDICATORS'
               for check in report['checks'])
    assert store.sha256(store.original_path(document)) == original_hash


def test_inconclusive_and_visible_only_inputs_are_also_gated(client, auth, matter,
                                                              make_pdf, tmp_path):
    inconclusive = _upload(client, auth, matter['id'], 'inconclusive.pdf',
                           _pdf_with_signature_dictionary(tmp_path, complete=False))
    denied = client.post(f'/documents/{inconclusive["id"]}/pages/rotate', headers=auth,
                         json={'page_index': 0, 'degrees': 90})
    assert denied.status_code == 409

    visible = _upload(client, auth, matter['id'], 'visible.pdf', make_pdf(1, 'Visible mark'))
    visible = client.patch(f'/documents/{visible["id"]}', headers=auth,
                           json={'visible_signature_status': 'present'}).json()
    denied = client.post(f'/documents/{visible["id"]}/pages/delete', headers=auth,
                         json={'page_index': 0})
    assert denied.status_code == 409
    checks = client.get(f'/matters/{matter["id"]}/validate', headers=auth).json()
    assert any(check['code'] == 'SIGNATURE_INSPECTION_INCONCLUSIVE' for check in checks)
    assert any(check['code'] == 'VISIBLE_SIGNATURE_ONLY' for check in checks)
