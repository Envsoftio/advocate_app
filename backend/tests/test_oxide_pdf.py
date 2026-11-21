"""Real-worker migration checks, opt-in until the worker is packaged."""
import hashlib
import os
import sys
from pathlib import Path
from io import BytesIO
from zipfile import ZipFile

import pytest

from app import file_io
from app.office_export import word_draft, powerpoint_draft
from app.oxide_pdf import extract_pages, stream_pages


def test_bundled_discovery_is_explicit(monkeypatch, tmp_path):
    from app.oxide_pdf import configured_worker
    monkeypatch.delenv('MATTERDRAFT_PDF_OXIDE_WORKER', raising=False)
    monkeypatch.delenv('MATTERDRAFT_PDF_OXIDE_BUNDLED', raising=False)
    monkeypatch.setattr(sys, 'frozen', True, raising=False)
    monkeypatch.setattr(sys, '_MEIPASS', str(tmp_path), raising=False)
    assert configured_worker() is None
    monkeypatch.setenv('MATTERDRAFT_PDF_OXIDE_BUNDLED', '1')
    suffix = '.exe' if sys.platform == 'win32' else ''
    assert configured_worker() == str(tmp_path / 'pdf-tools' / ('matterdraft-pdf-oxide' + suffix))
    monkeypatch.setenv('MATTERDRAFT_PDF_OXIDE_WORKER', '/explicit-worker')
    assert configured_worker() == '/explicit-worker'
    monkeypatch.delenv('MATTERDRAFT_PDF_OXIDE_WORKER')
    monkeypatch.setattr(sys, 'frozen', False)
    with pytest.raises(ValueError, match='packaged backend'):
        configured_worker()


@pytest.fixture
def worker(monkeypatch):
    binary = Path(__file__).resolve().parents[1] / 'pdf-oxide-worker/target/debug/matterdraft-pdf-oxide'
    if not binary.exists():
        pytest.skip('Build the Rust PDF Oxide worker first')
    monkeypatch.setenv('MATTERDRAFT_PDF_OXIDE_WORKER', str(binary))


def test_real_worker_office_and_protected_input(worker, tmp_path, monkeypatch):
    source = Path(__file__).resolve().parents[2] / 'samples/01_petition.pdf'
    original = source.read_bytes()
    pages = extract_pages(source)
    assert pages and any('petition' in text.lower() for text in pages)
    # Prove selected Office extraction does not call the legacy PDF reader.
    monkeypatch.setattr('app.office_export.open_pdf', lambda _: pytest.fail('legacy reader called'))
    with ZipFile(BytesIO(word_draft(source))) as archive:
        assert b'editable review draft' in archive.read('word/document.xml')
    with ZipFile(BytesIO(powerpoint_draft(source))) as archive:
        assert archive.testzip() is None
        assert 'ppt/slides/slide1.xml' in archive.namelist()
    root = tmp_path / 'vault'
    root.mkdir()
    file_io.register(root, os.urandom(32))
    try:
        protected = root / 'input.pdf'
        file_io.write_bytes(protected, original)
        assert protected.read_bytes() != original
        assert extract_pages(protected) == pages
        assert list(root.iterdir()) == [protected]
    finally:
        file_io.unregister(root)
    assert hashlib.sha256(source.read_bytes()).digest() == hashlib.sha256(original).digest()


def test_malformed_pdf_does_not_fallback(worker, tmp_path):
    source = tmp_path / 'broken.pdf'
    source.write_bytes(b'not a PDF')
    with pytest.raises(ValueError, match='no review draft'):
        word_draft(source)


def test_missing_worker_does_not_fallback(monkeypatch, tmp_path):
    monkeypatch.setenv('MATTERDRAFT_PDF_OXIDE_WORKER', str(tmp_path / 'missing'))
    source = tmp_path / 'input.pdf'
    source.write_bytes(b'%PDF-1.7')
    with pytest.raises(ValueError, match='no review draft'):
        word_draft(source)


def test_selected_page_matches_full_extraction(worker):
    source = Path(__file__).resolve().parents[2] / 'samples/01_petition.pdf'
    pages = extract_pages(source)
    assert extract_pages(source, page=1) == [pages[1]]
    with pytest.raises(ValueError):
        extract_pages(source, page=len(pages))
    with pytest.raises(ValueError):
        extract_pages(source, page=-1)


@pytest.mark.parametrize('response', [b'[]', b'null', b'{}', b'not json',
    b'{"protocol":1,"engine":"pdf_oxide","version":"0.3.78","pages":[12]}'])
def test_invalid_protocol_is_sanitized(worker, monkeypatch, tmp_path, response):
    source = tmp_path / 'input.pdf'
    source.write_bytes(b'private document content')
    monkeypatch.setattr('app.oxide_pdf._run', lambda *args: response)
    with pytest.raises(ValueError, match='PDF Oxide extraction failed') as error:
        extract_pages(source)
    assert 'private' not in str(error.value)


def test_search_endpoint_parity_and_guards(worker, client, auth, matter, monkeypatch):
    source = Path(__file__).resolve().parents[2] / 'samples/01_petition.pdf'
    response = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
        files=[('files', ('petition.pdf', source.read_bytes(), 'application/pdf'))])
    assert response.status_code == 201
    doc = response.json()[0]
    urls = [f'/matters/{matter["id"]}/search', f'/documents/{doc["id"]}/search']
    monkeypatch.delenv('MATTERDRAFT_PDF_OXIDE_SEARCH', raising=False)
    expected = [client.get(url, headers=auth, params={'q': 'PETITION'}).json() for url in urls]
    assert expected[0]['matches']
    monkeypatch.setenv('MATTERDRAFT_PDF_OXIDE_SEARCH', '1')
    monkeypatch.setattr('app.pdf_search.open_pdf', lambda _: pytest.fail('legacy search called'))
    for url, body in zip(urls, expected):
        assert client.get(url, headers=auth, params={'q': 'PETITION'}).json() == body
        assert client.get(url, headers=auth, params={'q': 'x'}).status_code == 422
        assert client.get(url, params={'q': 'petition'}).status_code == 401
    assert client.get(urls[1], headers=auth, params={'q': 'petition', 'revision': -1}).status_code == 409
    monkeypatch.setenv('MATTERDRAFT_PDF_OXIDE_WORKER', '/missing-worker')
    assert client.get(urls[1], headers=auth, params={'q': 'petition'}).status_code == 409


@pytest.mark.parametrize('body', ['import time; time.sleep(30)',
                                  'import sys; sys.stdout.write("x" * 100000)'])
def test_worker_timeout_and_output_limit(monkeypatch, tmp_path, body):
    binary = tmp_path / 'worker'
    binary.write_text(f'#!{sys.executable}\n{body}\n')
    binary.chmod(0o700)
    source = tmp_path / 'input.pdf'
    source.write_bytes(b'%PDF-1.7')
    monkeypatch.setenv('MATTERDRAFT_PDF_OXIDE_WORKER', str(binary))
    monkeypatch.setattr('app.oxide_pdf.WORKER_TIMEOUT', 0.2)
    monkeypatch.setattr('app.oxide_pdf.MAX_RESPONSE_BYTES', 1024)
    with pytest.raises(ValueError, match='PDF Oxide extraction failed'):
        extract_pages(source)


def test_search_result_cap(worker, client, auth, matter, monkeypatch, make_pdf):
    response = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
        files=[('files', ('many.pdf', make_pdf(pages=101, text='NEEDLE'), 'application/pdf'))])
    assert response.status_code == 201
    doc = response.json()[0]
    monkeypatch.setenv('MATTERDRAFT_PDF_OXIDE_SEARCH', '1')
    for url in [f'/matters/{matter["id"]}/search', f'/documents/{doc["id"]}/search']:
        result = client.get(url, headers=auth, params={'q': 'needle'})
        assert result.status_code == 200
        assert result.json()['truncated'] is True
        assert [r['page'] for r in result.json()['matches']] == list(range(1, 101))


def test_stream_beyond_office_page_limit(worker, tmp_path, make_pdf):
    source = tmp_path / 'large.pdf'
    source.write_bytes(make_pdf(pages=2001, text=''))
    original = source.read_bytes()
    with stream_pages(source) as pages:
        assert sum(1 for text in pages if text == '') == 2001
    with pytest.raises(ValueError):
        extract_pages(source)
    assert source.read_bytes() == original


def test_stream_protected_and_early_close(worker, tmp_path, monkeypatch):
    import subprocess
    root = tmp_path / 'vault'
    root.mkdir()
    file_io.register(root, os.urandom(32))
    spawned = []
    real_popen = subprocess.Popen
    def capture(*args, **kwargs):
        child = real_popen(*args, **kwargs)
        spawned.append(child)
        return child
    monkeypatch.setattr('app.oxide_pdf.subprocess.Popen', capture)
    try:
        source = root / 'source.pdf'
        original = (Path(__file__).resolve().parents[2] / 'samples/01_petition.pdf').read_bytes()
        file_io.write_bytes(source, original)
        with stream_pages(source) as pages:
            assert 'petition' in next(pages).lower()
        assert spawned[-1].poll() is not None
        with stream_pages(source) as pages:
            assert len(list(pages)) == 3
        assert list(root.iterdir()) == [source]
        assert file_io.read_bytes(source) == original
    finally:
        file_io.unregister(root)


@pytest.mark.parametrize('ending', [
    '',
    '{"kind":"page","page":1,"text":"wrong order"}\n',
    '{"kind":"end","page_count":1}\n',
])
def test_stream_rejects_incomplete_or_out_of_order(tmp_path, monkeypatch, ending):
    import json
    header = json.dumps(dict(protocol=2, engine='pdf_oxide', version='0.3.78',
                             kind='header', page_count=1)) + '\n'
    binary = tmp_path / 'worker'
    binary.write_text(f'#!{sys.executable}\nimport sys\nsys.stdin.buffer.read()\nsys.stdout.write({header + ending!r})\n')
    binary.chmod(0o700)
    source = tmp_path / 'input.pdf'
    source.write_bytes(b'%PDF')
    monkeypatch.setenv('MATTERDRAFT_PDF_OXIDE_WORKER', str(binary))
    with pytest.raises(ValueError, match='search extraction failed'):
        with stream_pages(source) as pages:
            list(pages)


@pytest.mark.parametrize('body', [
    'import time; time.sleep(30)',
    'import sys; sys.stdout.write("x" * 100000); sys.stdout.flush()',
])
def test_stream_timeout_and_frame_limit_reap_child(tmp_path, monkeypatch, body):
    import subprocess
    children = []
    real_popen = subprocess.Popen
    def capture(*args, **kwargs):
        child = real_popen(*args, **kwargs)
        children.append(child)
        return child
    binary = tmp_path / 'worker'
    binary.write_text(f'#!{sys.executable}\n{body}\n')
    binary.chmod(0o700)
    source = tmp_path / 'input.pdf'
    source.write_bytes(b'%PDF')
    monkeypatch.setenv('MATTERDRAFT_PDF_OXIDE_WORKER', str(binary))
    monkeypatch.setattr('app.oxide_pdf.WORKER_TIMEOUT', 0.2)
    monkeypatch.setattr('app.oxide_pdf.MAX_RESPONSE_BYTES', 1024)
    monkeypatch.setattr('app.oxide_pdf.subprocess.Popen', capture)
    with pytest.raises(ValueError, match='search extraction failed'):
        with stream_pages(source) as pages:
            list(pages)
    assert children and all(child.poll() is not None for child in children)
