"""Repeatable synthetic 1,000-page API benchmark; no existing matter data used.

Run from backend: .venv312/bin/python scripts/benchmark_pdf_core.py
"""
import json
from pathlib import Path
import platform
try:
    import resource
except ImportError:  # Windows has no resource module.
    resource = None
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import pymupdf
from fastapi.testclient import TestClient
from app.main import create_app


def main():
    headers = {'Authorization': 'Bearer synthetic-benchmark'}
    measurements = {'platform': platform.platform(), 'machine': platform.machine(),
                    'python': platform.python_version(), 'pymupdf': pymupdf.VersionBind,
                    'input_pages': 1000, 'input_documents': 100,
                    'fixture': '100 native-text documents, 10 A4 pages each; no scanned pages'}
    with tempfile.TemporaryDirectory(prefix='courtbundle-benchmark-') as root:
        with TestClient(create_app(root, 'synthetic-benchmark')) as client:
            response = client.post('/matters', headers=headers, json={
                'name': 'Synthetic 1000-page benchmark', 'court_profile': 'general-demo',
                'max_pages_per_volume': 300})
            response.raise_for_status()
            matter = response.json()
            base = f'/matters/{matter["id"]}'
            start = time.perf_counter()
            for number in range(100):
                with pymupdf.open() as pdf:
                    for page_index in range(10):
                        page = pdf.new_page()
                        page.insert_text((60, 80), f'SYNTHETIC BENCHMARK document {number} page {page_index}', fontsize=18)
                        page.insert_textbox(pymupdf.Rect(60, 110, 530, 700),
                                            ('Sample searchable evidence.\n' * 25), fontsize=12)
                    blob = pdf.tobytes()
                client.post(base + '/documents', headers=headers,
                            files=[('files', (f'document-{number:03}.pdf', blob, 'application/pdf'))]).raise_for_status()
            measurements['generate_and_import_seconds'] = round(time.perf_counter() - start, 3)
            start = time.perf_counter()
            job = client.post(base + '/build', headers=headers).json()
            deadline = time.monotonic() + 180
            while True:
                current = client.get(f'/jobs/{job["id"]}', headers=headers).json()
                if current['status'] not in ('queued', 'running'):
                    break
                if time.monotonic() > deadline:
                    raise RuntimeError('Build exceeded 180-second benchmark timeout')
                time.sleep(.05)
            assert current['status'] == 'completed', current
            measurements['build_verify_and_zip_seconds'] = round(time.perf_counter() - start, 3)
            manifest = current['result']
            measurements['output_pages'] = manifest['pages']
            measurements['volumes'] = manifest['volumes']
            measurements['artifact_bytes'] = {item['name']: item['bytes'] for item in manifest['files']}
            start = time.perf_counter()
            client.get(base + '/search', headers=headers, params={'q': 'ABSENT_MARKER'}).raise_for_status()
            measurements['full_matter_search_seconds'] = round(time.perf_counter() - start, 3)
            usage = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss if resource else None
            measurements['process_peak_rss_mib'] = (round(usage / (1024 ** 2 if sys.platform == 'darwin' else 1024), 2)
                                                    if usage is not None else None)
    print(json.dumps(measurements, indent=2))


if __name__ == '__main__':
    main()
