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
    with tempfile.TemporaryDirectory(prefix='courtbundle-packaged-qa-') as temporary:
        root = Path(temporary)
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            port = listener.getsockname()[1]
        environment = dict(os.environ, COURTBUNDLE_API_TOKEN=secrets.token_hex(24), PATH='/usr/bin:/bin')
        if protected:
            environment['COURTBUNDLE_REQUIRE_VAULT'] = '1'
        environment.pop('TESSDATA_PREFIX', None)
        with (root / 'engine.log').open('wb') as log:
            process = subprocess.Popen([executable, '--port', str(port), '--data-dir', str(root / 'data')],
                                       env=environment, stdout=log, stderr=log)
            try:
                with httpx.Client(base_url=f'http://127.0.0.1:{port}', timeout=60,
                                  headers={'Authorization': 'Bearer ' + environment['COURTBUNDLE_API_TOKEN']},
                                  trust_env=False) as client:
                    start = time.monotonic()
                    while True:
                        try:
                            client.get('/health').raise_for_status()
                            break
                        except httpx.HTTPError:
                            if process.poll() is not None or time.monotonic() - start > 60:
                                raise RuntimeError('Packaged engine failed to become ready; inspect local build')
                            time.sleep(.2)
                    startup = time.monotonic() - start
                    if protected:
                        assert client.get('/health').json()['status'] == 'locked'
                        assert client.get('/matters').status_code == 423
                        setup = client.post('/vault/setup', json={'password': 'synthetic packaged password 123'})
                        setup.raise_for_status()
                        assert len(setup.json()['recovery_key']) == 64
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
                    response = (client.post(base + '/documents/raw', content=original,
                        headers={'Content-Type': 'application/octet-stream', 'X-CourtBundle-Filename': 'mixed.pdf'})
                        if protected else client.post(base + '/documents',
                            files=[('files', ('mixed.pdf', original, 'application/pdf'))]))
                    response.raise_for_status()
                    doc = response.json()[0]

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
                        json={'preset': 'small', 'expected_revision': 1}))
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
                    else:
                        assert hashlib.sha256(original_path.read_bytes()).digest() == hashlib.sha256(original).digest()
                    print(json.dumps({'startup_seconds': round(startup, 2), 'bundled_ocr': 'passed',
                                      'compression': compressed['result'], 'bookmarked_export': 'passed',
                                      'original_integrity': 'passed'}, indent=2))
            finally:
                process.terminate()
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)


if __name__ == '__main__':
    main()
