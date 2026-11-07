import io
import hashlib
import json
import shutil
import subprocess
import time
from pathlib import Path
import pytest
from PIL import Image, ImageDraw, ImageFont
import pymupdf
from fastapi.testclient import TestClient
from app.main import create_app
from test_api import wait_for_job

SAMPLES = Path(__file__).resolve().parents[2] / 'samples'


@pytest.mark.skipif(not shutil.which('tesseract'), reason='Tesseract not installed')
def test_ocr_scanned_image(client, auth, matter):
    image = Image.new('RGB', (1300, 500), 'white')
    draw = ImageDraw.Draw(image)
    try:
        font = ImageFont.truetype('/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf', 65)
    except OSError:
        font = ImageFont.load_default()
    draw.text((80, 170), 'FILING OCR TEST 2026', font=font, fill='black')
    blob = io.BytesIO()
    image.save(blob, format='PNG')
    r = client.post(f'/matters/{matter["id"]}/documents?included=true', headers=auth,
                    files=[('files', ('scan.png', blob.getvalue(), 'image/png'))])
    assert r.status_code == 201, r.text
    doc_id = r.json()[0]['id']
    assert r.json()[0]['ocr_status'] == 'needs_ocr'
    job = client.post(f'/documents/{doc_id}/ocr', headers=auth).json()
    done = wait_for_job(client, auth, job['id'], timeout=40)
    assert done['status'] == 'completed', done['error']
    assert done['started_at']
    with pymupdf.open(stream=client.get(f'/documents/{doc_id}/pdf', headers=auth).content,filetype='pdf') as pdf:
        assert 'FILING' in pdf[0].get_text().upper()
    selectable = client.get(f'/documents/{doc_id}/pages/0/text-runs', headers=auth)
    assert selectable.status_code == 200, selectable.text
    assert selectable.json()['runs'] == []
    matches = client.get(f'/matters/{matter["id"]}/search', headers=auth,
                         params={'q': 'FILING'}).json()['matches']
    assert len(matches) == 1
    assert matches[0]['document_id'] == doc_id and matches[0]['page'] == 1

    review_url = f'/documents/{doc_id}/ocr-review'
    review = client.get(review_url, headers=auth, params={'page_index': 0, 'revision': 2})
    assert review.status_code == 200, review.text
    measured = review.json()
    assert measured['available'] is True
    assert measured['confidence'] is None or 0 <= measured['confidence'] <= 100
    assert measured['accepted_at'] is None and measured['can_rerun'] is True
    assert client.post(review_url + '/accept', headers=auth,
                       json={'page_index': 0, 'expected_revision': 1}).status_code == 409
    accepted = client.post(review_url + '/accept', headers=auth,
                           json={'page_index': 0, 'expected_revision': 2})
    assert accepted.status_code == 200, accepted.text
    assert client.get(review_url, headers=auth,
                      params={'page_index': 0, 'revision': 2}).json()['accepted_at']
    backup = client.get(f'/matters/{matter["id"]}/portable', headers=auth)
    assert backup.status_code == 200, backup.text
    restored = client.post('/matters/restore', headers=auth,
                           files={'file': ('ocr-review.cbmatter', backup.content, 'application/zip')})
    assert restored.status_code == 201, restored.text
    restored_doc = client.get(f'/matters/{restored.json()["id"]}/documents', headers=auth).json()[0]
    assert client.get(f'/documents/{restored_doc["id"]}/ocr-review', headers=auth,
                      params={'page_index': 0, 'revision': 2}).json()['accepted_at']
    original_before = client.get(f'/documents/{doc_id}/original', headers=auth).content
    printed_original = client.post(f'/documents/{doc_id}/original/print', headers=auth,
                                   json={'pages':'all','orientation':'source','scale':'fit'})
    assert printed_original.status_code == 200, printed_original.text
    with pymupdf.open(stream=printed_original.content, filetype='pdf') as printed:
        assert printed.page_count == 1
    rerun = client.post(f'/documents/{doc_id}/ocr-rerun', headers=auth,
                        json={'page_index': 0, 'expected_revision': 2, 'language': 'eng'})
    assert rerun.status_code == 202, rerun.text
    rerun_done = wait_for_job(client, auth, rerun.json()['id'], timeout=40)
    assert rerun_done['status'] == 'completed', rerun_done['error']
    assert client.get(review_url, headers=auth,
                      params={'page_index': 0, 'revision': 2}).status_code == 409
    assert client.get(review_url, headers=auth,
                      params={'page_index': 0, 'revision': 3}).json()['accepted_at'] is None
    with pymupdf.open(stream=client.get(f'/documents/{doc_id}/pdf', headers=auth).content,
                      filetype='pdf') as rerun_pdf:
        assert rerun_pdf[0].get_text().upper().count('FILING') == 1
    again = client.post(f'/documents/{doc_id}/ocr-rerun', headers=auth,
                        json={'page_index': 0, 'expected_revision': 3, 'language': 'eng'})
    assert again.status_code == 202, again.text
    again_done = wait_for_job(client, auth, again.json()['id'], timeout=40)
    assert again_done['status'] == 'completed', again_done['error']
    with pymupdf.open(stream=client.get(f'/documents/{doc_id}/pdf', headers=auth).content,
                      filetype='pdf') as rerun_pdf:
        assert rerun_pdf[0].get_text().upper().count('FILING') == 1
    assert client.get(f'/documents/{doc_id}/original', headers=auth).content == original_before


@pytest.mark.skipif('hin' not in subprocess.run(
    ['tesseract', '--list-langs'], capture_output=True, text=True).stdout.split()
    if shutil.which('tesseract') else True, reason='Hindi Tesseract model not installed')
@pytest.mark.parametrize('name, expected_pages, expected_rotation', [
    ('05_hindi_scan.pdf', 1, 0),
    ('06_hindi_mixed.pdf', 2, 90),
    ('07_hindi_poor_scan.pdf', 1, 0),
])
def test_hindi_mixed_script_ocr_preserves_source_and_page_geometry(
        client, auth, matter, name, expected_pages, expected_rotation):
    source = (SAMPLES / name).read_bytes()
    imported = client.post(f'/matters/{matter["id"]}/documents?included=true', headers=auth,
        files=[('files', (name, source, 'application/pdf'))])
    assert imported.status_code == 201, imported.text
    doc = imported.json()[0]
    stored_original = client.app.state.store.original_path(doc)
    assert hashlib.sha256(stored_original.read_bytes()).digest() == hashlib.sha256(source).digest()
    with pymupdf.open(stream=source, filetype='pdf') as original_pdf:
        visible_before = [page.get_pixmap().samples for page in original_pdf]
    started = client.post(f'/documents/{doc["id"]}/ocr', headers=auth,
                          json={'language': 'eng+hin'})
    assert started.status_code == 202, started.text
    finished = wait_for_job(client, auth, started.json()['id'], timeout=40)
    assert finished['status'] == 'completed', finished['error']
    assert finished['result']['language'] == 'eng+hin'
    with pymupdf.open(stream=client.get(f'/documents/{doc["id"]}/pdf',
                                   headers=auth).content, filetype='pdf') as pdf:
        assert len(pdf) == expected_pages
        assert [page.get_pixmap().samples for page in pdf] == visible_before
        assert pdf[0].rotation == expected_rotation
        assert tuple(pdf[0].mediabox) == (0, 0, 595, 842)
        text = pdf[0].get_text().replace(' ', '')
        assert 'हिंदीपरीक्षणदस्तावेज' in text
        assert 'CASE2026' in text
        if expected_pages == 2:
            assert pdf[0].get_text().count('Native Header 2026') == 1
            assert 'Native page two' in pdf[1].get_text()
    assert 'हिंदी' in str(client.get(f'/matters/{matter["id"]}/search',
                                  headers=auth, params={'q': 'हिंदी'}).json())
    assert stored_original.read_bytes() == source


def test_ocr_rejects_unlisted_language(client, auth, matter, make_pdf):
    imported = client.post(f'/matters/{matter["id"]}/documents?included=true', headers=auth,
        files=[('files', ('native.pdf', make_pdf(), 'application/pdf'))]).json()[0]
    response = client.post(f'/documents/{imported["id"]}/ocr', headers=auth,
                           json={'language': 'ara'})
    assert response.status_code == 422


@pytest.mark.skipif(not shutil.which('tesseract'), reason='Tesseract not installed')
def test_page_scoped_ocr_preserves_other_pages_original_and_revision_guard(client, auth, matter):
    image = Image.new('RGB', (1300, 500), 'white')
    ImageDraw.Draw(image).text((80, 170), 'SCOPED OCR PAGE 2026', fill='black',
                              font=ImageFont.truetype('/System/Library/Fonts/Supplemental/Arial.ttf', 65)
                              if Path('/System/Library/Fonts/Supplemental/Arial.ttf').exists()
                              else ImageFont.load_default())
    blob = io.BytesIO()
    image.save(blob, format='PNG')
    with pymupdf.open() as pdf:
        for _ in range(2):
            page = pdf.new_page(width=595, height=842)
            page.insert_image(pymupdf.Rect(40, 80, 555, 340), stream=blob.getvalue())
        source = pdf.tobytes()
    uploaded = client.post(f'/matters/{matter["id"]}/documents?included=true', headers=auth,
        files=[('files', ('two-scans.pdf', source, 'application/pdf'))])
    assert uploaded.status_code == 201, uploaded.text
    doc = uploaded.json()[0]
    assert doc['ocr_status'] == 'needs_ocr'
    printed_original = client.post(f'/documents/{doc["id"]}/original/print', headers=auth,
                                   json={'pages':'1','orientation':'source','scale':'fit'})
    assert printed_original.status_code == 200, printed_original.text
    with pymupdf.open(stream=printed_original.content, filetype='pdf') as printed:
        assert printed.page_count == 1
    assert client.app.state.store.original_path(doc).read_bytes() == source
    endpoint = f'/documents/{doc["id"]}/ocr'
    assert client.post(endpoint, headers=auth, json={'expected_revision': 1, 'page_index': 2}).status_code == 422
    started = client.post(endpoint, headers=auth,
        json={'expected_revision': 1, 'page_index': 1, 'language': 'eng'})
    assert started.status_code == 202, started.text
    finished = wait_for_job(client, auth, started.json()['id'], timeout=40)
    assert finished['status'] == 'completed', finished['error']
    assert finished['result']['pages_checked'] == 1
    assert finished['result']['requested_page_index'] == 1
    with pymupdf.open(stream=client.get(f'/documents/{doc["id"]}/pdf', headers=auth).content,
                      filetype='pdf') as working:
        assert working[0].get_text().strip() == ''
        assert 'SCOPED' in working[1].get_text().upper()
    current = client.get(f'/matters/{matter["id"]}/documents', headers=auth).json()[0]
    assert current['revision'] == 2
    assert current['ocr_status'] == 'needs_ocr'
    assert client.app.state.store.original_path(current).read_bytes() == source
    assert client.post(endpoint, headers=auth,
        json={'expected_revision': 1, 'page_index': 0}).status_code == 409


@pytest.mark.skipif(not shutil.which('tesseract'), reason='Tesseract not installed')
def test_ocr_provenance_survives_history_restart_and_portable(tmp_path):
    root = tmp_path / 'store'
    auth = {'Authorization': 'Bearer test-token'}
    source = (SAMPLES / '04_scanned_annexure.jpg').read_bytes()
    with TestClient(create_app(data_dir=root, token='test-token')) as client:
        matter = client.post('/matters', headers=auth, json={'name': 'OCR provenance fixture'}).json()
        uploaded = client.post(f'/matters/{matter["id"]}/documents?included=true', headers=auth,
            files=[('files', ('scan.jpg', source, 'image/jpeg'))])
        assert uploaded.status_code == 201, uploaded.text
        doc = uploaded.json()[0]
        started = client.post(f'/documents/{doc["id"]}/ocr', headers=auth,
                              json={'language': 'eng'})
        assert started.status_code == 202, started.text
        finished = wait_for_job(client, auth, started.json()['id'], timeout=40)
        assert finished['status'] == 'completed', finished.get('error')
        assert finished['result']['pages_ocr_applied'] == 1
        current = client.get(f'/matters/{matter["id"]}/documents', headers=auth).json()[0]
        run = json.loads(current['ocr_provenance_json'])[0]
        assert run['language'] == 'eng'
        assert run['provider'] == 'PyMuPDF+Tesseract'
        assert run['pymupdf_version'] == pymupdf.VersionBind
        assert run['model_sha256']['eng'] == hashlib.sha256(
            (Path(pymupdf.get_tessdata()) / 'eng.traineddata').read_bytes()).hexdigest()
        assert run['applied_revision'] == current['revision'] == 2
        assert run['pages_ocr_applied'] == 1
        with client.app.state.store.connect() as con:
            saved = con.execute('''SELECT ocr_provenance_json FROM document_revisions
                WHERE document_id=? AND revision=2''', (doc['id'],)).fetchone()[0]
        assert json.loads(saved) == [run]
        rotated = client.post(f'/documents/{doc["id"]}/pages/rotate', headers=auth,
                              json={'page_index': 0, 'degrees': 90})
        assert rotated.status_code == 200, rotated.text
        assert json.loads(rotated.json()['ocr_provenance_json']) == [run]
        undone = client.post(f'/matters/{matter["id"]}/history/undo', headers=auth)
        assert undone.status_code == 200, undone.text
        assert json.loads(client.get(f'/matters/{matter["id"]}/documents', headers=auth)
                          .json()[0]['ocr_provenance_json']) == [run]
        undone_ocr = client.post(f'/matters/{matter["id"]}/history/undo', headers=auth)
        assert undone_ocr.status_code == 200, undone_ocr.text
        assert json.loads(client.get(f'/matters/{matter["id"]}/documents', headers=auth)
                          .json()[0]['ocr_provenance_json']) == []
        redone_ocr = client.post(f'/matters/{matter["id"]}/history/redo', headers=auth)
        assert redone_ocr.status_code == 200, redone_ocr.text
        assert json.loads(client.get(f'/matters/{matter["id"]}/documents', headers=auth)
                          .json()[0]['ocr_provenance_json']) == [run]
        backup = client.get(f'/matters/{matter["id"]}/portable', headers=auth)
        assert backup.status_code == 200, backup.text
        restored = client.post('/matters/restore', headers=auth,
            files={'file': ('ocr.cbmatter', backup.content, 'application/zip')})
        assert restored.status_code == 201, restored.text
        restored_doc = client.get(f'/matters/{restored.json()["id"]}/documents',
                                  headers=auth).json()[0]
        assert json.loads(restored_doc['ocr_provenance_json']) == [run]
        assert client.app.state.store.original_path(current).read_bytes() == source
    with TestClient(create_app(data_dir=root, token='test-token')) as reopened:
        persisted = reopened.get(f'/matters/{matter["id"]}/documents', headers=auth).json()[0]
        assert json.loads(persisted['ocr_provenance_json']) == [run]
        imported = reopened.get(f'/matters/{restored.json()["id"]}/documents',
                                headers=auth).json()[0]
        assert json.loads(imported['ocr_provenance_json']) == [run]
