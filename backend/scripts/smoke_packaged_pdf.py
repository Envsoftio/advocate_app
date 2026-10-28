"""Exercise a frozen sidecar with disposable synthetic PDFs and bundled OCR.

Usage: python backend/scripts/smoke_packaged_pdf.py /path/to/filing-api [--protected]
This verifies the sidecar API, not the native WebView or Save dialog.
"""
import hashlib
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import sys
import tempfile
import time

import httpx
import pymupdf


def main():
    executable = str(Path(sys.argv[1]).resolve())
    protected = '--protected' in sys.argv[2:]
    with tempfile.TemporaryDirectory(prefix='matterdraft-packaged-qa-') as temporary:
        root = Path(temporary)
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            port = listener.getsockname()[1]
        environment = dict(os.environ, MATTERDRAFT_API_TOKEN=secrets.token_hex(24), PATH='/usr/bin:/bin')
        if protected:
            environment['MATTERDRAFT_REQUIRE_VAULT'] = '1'
        environment.pop('TESSDATA_PREFIX', None)
        with (root / 'engine.log').open('wb') as log:
            process = subprocess.Popen([executable, '--port', str(port), '--data-dir', str(root / 'data')],
                                       env=environment, stdout=log, stderr=log)
            try:
                with httpx.Client(base_url=f'http://127.0.0.1:{port}', timeout=60,
                                  headers={'Authorization': 'Bearer ' + environment['MATTERDRAFT_API_TOKEN']},
                                  trust_env=False) as client:
                    start = time.monotonic()
                    while True:
                        try:
                            client.get('/health').raise_for_status()
                            break
                        except httpx.HTTPError:
                            if process.poll() is not None or time.monotonic() - start > 60:
                                diagnostic = (root / 'engine.log').read_text(errors='replace')[-8000:]
                                raise RuntimeError(f'Packaged engine failed to become ready '
                                                   f'(exit={process.poll()}): {diagnostic}')
                            time.sleep(.2)
                    startup = time.monotonic() - start
                    assert client.get('/health').json()['app'] == 'MatterDraft Studio'
                    if protected:
                        assert client.get('/health').json()['status'] == 'locked'
                        assert client.get('/matters').status_code == 423
                        setup = client.post('/vault/setup', json={'password': 'synthetic packaged password 123'})
                        setup.raise_for_status()
                        recovery_key = setup.json()['recovery_key']
                        assert len(recovery_key) == 64
                    matter = client.post('/matters', json={'name': 'Synthetic packaged PDF QA',
                                                          'court_profile': 'general-demo'}).json()
                    base = f'/matters/{matter["id"]}'
                    with pymupdf.open() as scan:
                        page = scan.new_page(width=500, height=140)
                        page.insert_text((25, 80), 'PACKAGED SCANNED EVIDENCE', fontsize=22)
                        pix = page.get_pixmap(dpi=240)
                    with pymupdf.open() as pdf:
                        page = pdf.new_page()
                        page.insert_text((60, 60), 'Native header')
                        page.insert_image(pymupdf.Rect(40, 150, 540, 290), pixmap=pix)
                        original = pdf.tobytes()
                    response = (client.post(base + '/documents/raw?included=true', content=original,
                        headers={'Content-Type': 'application/octet-stream', 'X-CourtBundle-Filename': 'mixed.pdf'})
                        if protected else client.post(base + '/documents?included=true',
                            files=[('files', ('mixed.pdf', original, 'application/pdf'))]))
                    response.raise_for_status()
                    doc = response.json()[0]
                    # The frozen service must treat malformed acknowledgment as false,
                    # rather than reject an otherwise valid unsigned page edit.
                    overlay = client.post(f'/documents/{doc["id"]}/pages/text-overlay', json={
                        'page_index': 0, 'x': .15, 'y': .6, 'text': 'QA MARK',
                        'acknowledge_signature_risk': {}})
                    overlay.raise_for_status()
                    assert overlay.json()['revision'] == doc['revision'] + 1
                    marked = client.patch(f'/documents/{doc["id"]}',
                                          json={'visible_signature_status': 'present'})
                    marked.raise_for_status()
                    challenge = client.post(f'/documents/{doc["id"]}/pages/text-overlay',
                        headers={'X-CourtBundle-Review-Protocol': 'challenge'},
                        json={'page_index': 0, 'x': .15, 'y': .65, 'text': 'REVIEWED MARK',
                              'acknowledge_signature_risk': False})
                    challenge.raise_for_status()
                    assert challenge.json()['review_required'] is True
                    assert client.get(base + '/documents').json()[0]['revision'] == overlay.json()['revision']
                    edit_review = client.post(f'/documents/{doc["id"]}/pages/replace-text-run',
                        headers={'X-CourtBundle-Review-Protocol': 'challenge'},
                        json={'page_index': 0, 'span_index': 0,
                              'expected_revision': overlay.json()['revision'],
                              'expected_text': 'Native header', 'replacement': 'Reviewed header',
                              'acknowledge_signature_risk': False})
                    edit_review.raise_for_status()
                    assert edit_review.json()['review_required'] is True
                    assert client.get(base + '/documents').json()[0]['revision'] == overlay.json()['revision']
                    accepted = client.post(f'/documents/{doc["id"]}/pages/text-overlay',
                        json={'page_index': 0, 'x': .15, 'y': .65, 'text': 'REVIEWED MARK',
                              'acknowledge_signature_risk': True})
                    accepted.raise_for_status()
                    assert accepted.json()['revision'] == overlay.json()['revision'] + 1
                    overlay = accepted
                    client.patch(f'/documents/{doc["id"]}',
                                 json={'visible_signature_status': 'absent'}).raise_for_status()

                    def finish(response):
                        response.raise_for_status()
                        job = response.json()
                        deadline = time.monotonic() + 60
                        while job['status'] in ('queued', 'running'):
                            if time.monotonic() > deadline:
                                raise RuntimeError('Packaged operation exceeded smoke-test timeout')
                            time.sleep(.1)
                            job = client.get(f'/jobs/{job["id"]}').json()
                        assert job['status'] == 'completed', job['error']
                        return job

                    compressed = finish(client.post(f'/documents/{doc["id"]}/compress',
                        json={'preset': 'small', 'expected_revision': overlay.json()['revision']}))
                    finish(client.post(f'/documents/{doc["id"]}/ocr'))
                    current = client.get(base + '/documents').json()[0]
                    client.put(base + '/bookmarks', json={'entries': [{
                        'id': 'a' * 32, 'title': 'Reviewed synthetic target', 'document_id': doc['id'],
                        'page_index': 0, 'target_revision': current['revision'], 'parent_id': None}]}).raise_for_status()
                    built = finish(client.post(base + '/build'))
                    response = client.get(base + f'/exports/{built["id"]}/combined.pdf')
                    response.raise_for_status()
                    with pymupdf.open(stream=response.content, filetype='pdf') as pdf:
                        assert pdf.page_count == 2
                        assert 'PACKAGED SCANNED EVIDENCE' in pdf[1].get_text()
                        assert pdf[0].get_links()[0]['page'] == 1
                        assert [2, 'Reviewed synthetic target', 2] in pdf.get_toc()
                    original_path = root / 'data' / 'matters' / matter['id'] / doc['source_relpath']
                    if protected:
                        assert original_path.read_bytes().startswith(b'CBENC1\n')
                        assert original_path.read_bytes() != original
                        backup = client.get(base + '/portable')
                        backup.raise_for_status()
                        assert backup.content.startswith(b'CBMAT4\n')
                        assert b'PACKAGED SCANNED EVIDENCE' not in backup.content
                        restored = client.post('/matters/restore', content=backup.content,
                                               headers={'Content-Type': 'application/octet-stream'})
                        restored.raise_for_status()
                        assert restored.json()['id'] != matter['id']
                        assert len(client.get('/matters').json()) == 2
                        rotated = client.post('/vault/rotate',
                                              json={'new_password': 'replacement synthetic password 123'})
                        rotated.raise_for_status()
                        new_recovery_key = rotated.json()['recovery_key']
                        assert len(new_recovery_key) == 64
                        forbidden = (b'PACKAGED SCANNED EVIDENCE',
                                     b'synthetic packaged password 123',
                                     b'replacement synthetic password 123',
                                     recovery_key.encode(), new_recovery_key.encode())
                        assert all(all(secret not in path.read_bytes() for secret in forbidden)
                                   for path in root.rglob('*') if path.is_file())
                    else:
                        assert hashlib.sha256(original_path.read_bytes()).digest() == hashlib.sha256(original).digest()
                    hindi_source = (Path(__file__).resolve().parents[2] / 'samples' /
                                    '05_hindi_scan.pdf').read_bytes()
                    hindi_response = (client.post(base + '/documents/raw', content=hindi_source,
                        headers={'Content-Type': 'application/octet-stream',
                                 'X-CourtBundle-Filename': '05_hindi_scan.pdf'})
                        if protected else client.post(base + '/documents',
                            files=[('files', ('05_hindi_scan.pdf', hindi_source, 'application/pdf'))]))
                    hindi_response.raise_for_status()
                    hindi_doc = hindi_response.json()[0]
                    assert hindi_doc['source_sha256'] == hashlib.sha256(hindi_source).hexdigest()
                    hindi_job = finish(client.post(f'/documents/{hindi_doc["id"]}/ocr',
                                                   json={'language': 'eng+hin'}))
                    assert hindi_job['result']['language'] == 'eng+hin'
                    assert hindi_job['result']['pages_ocr_applied'] == 1
                    hindi_pdf = client.get(f'/documents/{hindi_doc["id"]}/pdf')
                    hindi_pdf.raise_for_status()
                    with pymupdf.open(stream=hindi_pdf.content, filetype='pdf') as pdf:
                        recognized = pdf[0].get_text().replace(' ', '')
                        assert 'हिंदीपरीक्षणदस्तावेज' in recognized
                        assert 'CASE2026' in recognized
                        assert pdf[0].rect == pymupdf.Rect(0, 0, 595, 842)
                    print(json.dumps({'startup_seconds': round(startup, 2), 'bundled_ocr': 'passed',
                                      'bundled_hindi_ocr': 'passed',
                                      'compression': compressed['result'], 'bookmarked_export': 'passed',
                                      'original_integrity': 'passed',
                                      'protected_backup_restore_rotation': 'passed' if protected else 'not run'},
                                     indent=2))
            finally:
                process.terminate()
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)


if __name__ == '__main__':
    main()
