import io
import shutil
import time
import pytest
from PIL import Image, ImageDraw, ImageFont
import pymupdf
from test_api import wait_for_job


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
