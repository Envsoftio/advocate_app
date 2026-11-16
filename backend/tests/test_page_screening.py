"""Read-only screening, measured evidence, cancellation and stale-input isolation."""
import time
import pymupdf
from app import page_analysis
from app.file_io import read_bytes
from app.pdf_ops import ProcessingError
from test_api import upload


def fixture_pdf():
    with pymupdf.open() as image_source:
        page = image_source.new_page(width=300, height=200)
        page.insert_text((20, 60), 'SYNTHETIC OCR REVIEW', fontsize=18)
        image = page.get_pixmap(matrix=pymupdf.Matrix(2, 2)).tobytes('png')
    with pymupdf.open() as pdf:
        pdf.new_page(width=300, height=200)
        page = pdf.new_page(width=300, height=200)
        page.insert_image(page.rect, stream=image)
        page = pdf.new_page(width=300, height=200)
        page.insert_text((20, 60), 'Native text remains native')
        return pdf.tobytes()


def wait_job(client, auth, job):
    for _ in range(400):
        result = client.get('/jobs/' + job['id'], headers=auth).json()
        if result['status'] not in ('queued', 'running'):
            return result
        time.sleep(.03)
    raise AssertionError('Screening did not finish')


def test_screen_measures_candidates_preserves_bytes_and_invalidates(client, auth, matter, monkeypatch):
    blob = fixture_pdf()
    doc = client.post(f'/matters/{matter["id"]}/documents?included=true', headers=auth,
                      files={'files': ('screen.pdf', blob, 'application/pdf')}).json()[0]
    monkeypatch.setattr(page_analysis, 'inspect_ocr_page', lambda *a, **kw: {
        'available': True, 'confidence': 58.5, 'regions': [{'text': 'DO_NOT_STORE_RECOGNIZED_TEXT'}]})
    url = f'/matters/{matter["id"]}'
    checks = client.get(url + '/validate', headers=auth).json()
    digest = checks[0]['effective_input_digest']
    response = client.post(url + '/preflight/screen', headers=auth, json={'expected_snapshot': digest})
    assert response.status_code == 202, response.text
    job = wait_job(client, auth, response.json())
    assert job['status'] == 'completed', job
    assert job['result']['pages_screened'] == 3 and job['result']['blank_candidates'] == 1
    findings = client.get(url + '/validate', headers=auth).json()
    blank = next(f for f in findings if f['code'] == 'POSSIBLE_BLANK_PAGE')
    low = next(f for f in findings if f['code'] == 'LOW_OCR_CONFIDENCE')
    assert blank['page_index'] == 0 and low['page_index'] == 1
    assert '58.5%' in low['message'] and low['severity'] == 'warning'
    report = read_bytes(page_analysis.report_path(client.app.state.store, matter['id'], digest))
    assert b'DO_NOT_STORE_RECOGNIZED_TEXT' not in report
    assert client.get(f'/documents/{doc["id"]}/original', headers=auth).content == blob
    assert client.get(f'/documents/{doc["id"]}/pdf', headers=auth).content == blob
    client.patch(f'/documents/{doc["id"]}', headers=auth, json={'title': 'Changed inputs'})
    findings = client.get(url + '/validate', headers=auth).json()
    assert any(f['code'] == 'PAGE_SCREENING_NOT_RUN' for f in findings)
    assert not any(f['code'] == 'LOW_OCR_CONFIDENCE' for f in findings)
    assert read_bytes(page_analysis.report_path(client.app.state.store, matter['id'], digest)) == report
    assert client.post(url + '/preflight/screen', headers=auth, json={'expected_snapshot': digest}).status_code == 409


def test_cancelled_scan_does_not_replace_previous_evidence(client, auth, matter, make_pdf):
    upload(client, auth, matter, make_pdf)
    store = client.app.state.store
    digest = store.build_input_digest(matter['id'], effective=True)
    page_analysis.scan(store, matter['id'], digest, lambda *a: None, lambda: False)
    path = page_analysis.report_path(store, matter['id'], digest)
    before = read_bytes(path)
    import pytest
    with pytest.raises(ProcessingError, match='cancelled'):
        page_analysis.scan(store, matter['id'], digest, lambda *a: None, lambda: True)
    assert read_bytes(path) == before


def test_unavailable_and_no_words_never_pass(client, auth, matter, monkeypatch):
    blob = fixture_pdf()
    client.post(f'/matters/{matter["id"]}/documents?included=true', headers=auth,
                files={'files': ('screen.pdf', blob, 'application/pdf')})
    store = client.app.state.store
    digest = store.build_input_digest(matter['id'], effective=True)
    for measured, expected in [
        ({'available': False, 'confidence': None, 'regions': []}, 'PAGE_SCREENING_UNAVAILABLE'),
        ({'available': True, 'confidence': None, 'regions': []}, 'OCR_NO_WORDS'),
    ]:
        monkeypatch.setattr(page_analysis, 'inspect_ocr_page', lambda *a, **kw: measured)
        page_analysis.scan(store, matter['id'], digest, lambda *a: None, lambda: False)
        findings = page_analysis.findings(store, matter['id'])
        assert any(f['code'] == expected and f['page_index'] == 1 for f in findings)


def test_real_tesseract_screening_returns_measured_provenance(client, auth, matter):
    import shutil
    import pytest
    if not shutil.which('tesseract'):
        pytest.skip('Tesseract CLI unavailable on this host')
    client.post(f'/matters/{matter["id"]}/documents?included=true', headers=auth,
                files={'files': ('actual-ocr.pdf', fixture_pdf(), 'application/pdf')})
    store = client.app.state.store
    digest = store.build_input_digest(matter['id'], effective=True)
    page_analysis.scan(store, matter['id'], digest, lambda *a: None, lambda: False)
    report = page_analysis.current_report(store, matter['id'])
    row = report['pages'][1]
    assert row['ocr_outcome'] in ('measured', 'low_confidence'), row
    assert 0 <= row['confidence'] <= 100
    assert report['providers']['eng']['tesseract_cli_version']
    assert report['providers']['eng']['model_sha256']['eng']


def test_visible_text_and_marks_veto_blank_classification():
    with pymupdf.open() as pdf:
        page = pdf.new_page()
        assert page_analysis.screen_page(page)['blank_candidate']
        page.insert_text((30, 30), 'Visible text')
        assert not page_analysis.screen_page(page)['blank_candidate']
        page = pdf.new_page()
        page.draw_line((30, 30), (200, 200), width=3)
        assert not page_analysis.screen_page(page)['blank_candidate']
        page = pdf.new_page()
        page.insert_text((30, 30), 'Invisible OCR text', render_mode=3)
        assert not page_analysis.screen_page(page)['blank_candidate']


def test_active_tesseract_can_be_cancelled_without_writing(tmp_path):
    import shutil
    import pytest
    from app.pdf_ops import inspect_ocr_page
    if not shutil.which('tesseract'):
        pytest.skip('Tesseract CLI unavailable')
    path = tmp_path / 'cancel.pdf'
    blob = fixture_pdf()
    path.write_bytes(blob)
    with pytest.raises(ProcessingError, match='cancelled'):
        inspect_ocr_page(path, 1, cancelled=lambda: True)
    assert path.read_bytes() == blob


def test_researched_profiles_remain_drafts_and_do_not_borrow_portal_limits(client, auth, make_pdf):
    profiles = {p['id']: p for p in client.get('/profiles', headers=auth).json()}
    for key in ('supreme-court-source-draft', 'punjab-haryana-source-draft'):
        profile = profiles[key]
        assert profile['status'] == 'SOURCE_LINKED_DRAFT'
        assert profile['approval'] == {'status': 'not_reviewed', 'reviewer': None, 'reviewed_at': None}
        assert profile['max_file_mb'] is None
        assert profile['version'] == 'research-2026-11-17.1'
        matter = client.post('/matters', headers=auth, json={'name': 'Synthetic draft court', 'court_profile': key}).json()
        upload(client, auth, matter, make_pdf)
        findings = client.get(f'/matters/{matter["id"]}/validate', headers=auth).json()
        assert any(f['code'] == 'PROFILE_NOT_APPROVED' for f in findings)
        assert not any(f['severity'] == 'error' for f in findings)
    assert profiles['delhi-hc-illustrative']['status'] == 'NOT_LEGALLY_APPROVED'
    assert profiles['general-demo']['max_file_mb'] is None
