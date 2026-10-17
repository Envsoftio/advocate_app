import io
import hashlib
import shutil
import subprocess
import time
from pathlib import Path
import pytest
from PIL import Image, ImageDraw, ImageFont
import pymupdf
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
    r = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
                    files=[('files', ('scan.png', blob.getvalue(), 'image/png'))])
    assert r.status_code == 201, r.text
    doc_id = r.json()[0]['id']
    job = client.post(f'/documents/{doc_id}/ocr', headers=auth).json()
    done = wait_for_job(client, auth, job['id'], timeout=40)
    assert done['status'] == 'completed', done['error']
    with pymupdf.open(stream=client.get(f'/documents/{doc_id}/pdf', headers=auth).content,filetype='pdf') as pdf:
        assert 'FILING' in pdf[0].get_text().upper()
    selectable = client.get(f'/documents/{doc_id}/pages/0/text-runs', headers=auth)
    assert selectable.status_code == 200, selectable.text
    assert selectable.json()['runs'] == []
    matches = client.get(f'/matters/{matter["id"]}/search', headers=auth,
                         params={'q': 'FILING'}).json()['matches']
    assert len(matches) == 1
    assert matches[0]['document_id'] == doc_id and matches[0]['page'] == 1


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
    imported = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
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
    imported = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
        files=[('files', ('native.pdf', make_pdf(), 'application/pdf'))]).json()[0]
    response = client.post(f'/documents/{imported["id"]}/ocr', headers=auth,
                           json={'language': 'ara'})
    assert response.status_code == 422
