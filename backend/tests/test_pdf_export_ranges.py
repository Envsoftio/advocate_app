import asyncio
import hashlib
import io
import json

import pytest
from fastapi.testclient import TestClient
from starlette.requests import Request
from starlette.requests import ClientDisconnect

from app.file_io import write_bytes
from app.main import create_app
from app.pdf_stream import TRANSFER_CHUNK_BYTES, pdf_stream_response
from app.vault import EncryptedReader


def _artifact(client, auth, content):
    matter = client.post('/matters', headers=auth, json={
        'name': 'Synthetic range transfer', 'court_profile': 'general-demo'}).json()
    store = client.app.state.store
    export_id = 'a' * 32
    folder = store.exports_root(matter['id']) / export_id
    folder.mkdir(parents=True)
    path = folder / 'combined.pdf'
    write_bytes(path, content)
    write_bytes(folder / 'export_manifest.json', json.dumps({'files': [{
        'name': 'combined.pdf', 'bytes': len(content), 'sha256': hashlib.sha256(content).hexdigest(),
    }]}).encode())
    return f'/matters/{matter["id"]}/exports/{export_id}/inspection/combined.pdf', path


@pytest.mark.parametrize('encrypted', [False, True])
def test_export_ranges_are_authenticated_exact_and_bounded(tmp_path, monkeypatch, encrypted, make_pdf):
    auth = {'Authorization': 'Bearer range-test'}
    content = b'%PDF-synthetic\n' + bytes(range(256)) * 10000
    with TestClient(create_app(tmp_path / 'store', 'range-test', data_key=b'r' * 32 if encrypted else None)) as client:
        url, path = _artifact(client, auth, content)
        # The export endpoint must never fall back to its former whole-PDF
        # materialization path, even when the vault is enabled.
        import app.main as main
        original_read = main.read_bytes
        def bounded_read(candidate):
            assert candidate != path, 'Full PDF bytes materialized by the response'
            return original_read(candidate)
        monkeypatch.setattr(main, 'read_bytes', bounded_read)
        sizes = []
        original_encrypted_read = EncryptedReader.read
        def measured_read(self, size=-1):
            sizes.append(size)
            return original_encrypted_read(self, size)
        monkeypatch.setattr(EncryptedReader, 'read', measured_read)

        assert client.get(url).status_code == 401
        response = client.get(url, headers={**auth, 'Range': 'bytes=1048500-1048700', 'Origin': 'http://127.0.0.1:1420'})
        assert response.status_code == 206
        assert response.content == content[1048500:1048701]
        assert response.headers['content-range'] == f'bytes 1048500-1048700/{len(content)}'
        assert response.headers['content-length'] == '201'
        assert response.headers['cache-control'] == 'no-store'
        assert response.headers['accept-ranges'] == 'bytes'
        assert 'Content-Range' in response.headers['access-control-expose-headers']
        assert client.get(url, headers={**auth, 'Range': 'bytes=-19'}).content == content[-19:]
        assert client.get(url, headers={**auth, 'Range': f'bytes={len(content)-7}-'}).content == content[-7:]
        assert client.get(url, headers={**auth, 'Range': f'bytes={len(content)-7}-{len(content)+100}'}).content == content[-7:]
        head = client.head(url, headers=auth)
        assert head.status_code == 200 and not head.content
        assert int(head.headers['content-length']) == len(content)
        full = client.get(url, headers=auth)
        assert full.status_code == 200 and full.content == content
        if encrypted:
            assert sizes and max(sizes) <= TRANSFER_CHUNK_BYTES and min(sizes) > 0
            assert path.read_bytes().startswith(b'CBENC1\n')

        matter = client.post('/matters', headers=auth, json={
            'name': 'Synthetic working ranges', 'court_profile': 'general-demo'}).json()
        uploaded = client.post(f'/matters/{matter["id"]}/documents?included=true', headers=auth,
                               files=[('files', ('synthetic.pdf', make_pdf(1), 'application/pdf'))])
        assert uploaded.status_code == 201
        document = uploaded.json()[0]
        working_path = client.app.state.store.working_path(document)
        # Synthetic byte fixture exercises ranges spanning vault chunk edges.
        write_bytes(working_path, content)
        working_url = f'/documents/{document["id"]}/pdf?revision=1'
        assert client.get(working_url).status_code == 401
        response = client.get(working_url, headers={**auth, 'Range': 'bytes=1048500-1048700'})
        assert response.status_code == 206 and response.content == content[1048500:1048701]
        assert response.headers['cache-control'] == 'no-store'
        assert response.headers['accept-ranges'] == 'bytes'
        assert client.get(f'/documents/{document["id"]}/pdf?revision=2', headers={**auth, 'Range': 'bytes=0-1'}).status_code == 409
        assert client.head(working_url, headers=auth).headers['content-length'] == str(len(content))


def test_invalid_ranges_and_stale_artifacts_cannot_return_partial_bytes(client, auth):
    content = b'%PDF-synthetic-range'
    url, path = _artifact(client, auth, content)
    for value in ['bytes=999-', 'bytes=8-2', 'bytes=-0', 'bytes=-', 'bytes=0-1,5-8', 'bytes=' + '9' * 5000 + '-']:
        response = client.get(url, headers={**auth, 'Range': value})
        assert response.status_code == 416
        assert response.headers['content-range'] == f'bytes */{len(content)}'
        assert not response.content
    # Unknown conditional validator means a complete response, not a claimed
    # identity match. Existing manifest hash verification still protects it.
    response = client.get(url, headers={**auth, 'Range': 'bytes=0-1', 'If-Range': 'unknown'})
    assert response.status_code == 200 and response.content == content
    path.write_bytes(b'X' + content[1:])
    assert client.get(url, headers={**auth, 'Range': 'bytes=0-1'}).status_code == 404


def test_export_preflight_allows_only_expected_browser_range_headers(client, auth):
    url, _ = _artifact(client, auth, b'%PDF-synthetic')
    response = client.options(url, headers={
        'Origin': 'http://127.0.0.1:1420', 'Access-Control-Request-Method': 'GET',
        'Access-Control-Request-Headers': 'Authorization,Range',
    })
    assert response.status_code == 200
    assert 'Range' in response.headers['access-control-allow-headers']


def test_pdf_stream_closes_open_file_when_browser_disconnects(tmp_path, monkeypatch):
    path = tmp_path / 'synthetic.pdf'
    stream = io.BytesIO(b'%PDF-synthetic' * 100000)
    monkeypatch.setattr(type(path), 'open', lambda *_args, **_kwargs: stream)
    request = Request({'type': 'http', 'method': 'GET', 'headers': []})
    response = pdf_stream_response(path, request, 'synthetic.pdf')
    blocks = []
    async def send(message):
        if message['type'] == 'http.response.body':
            blocks.append(len(message['body']))
            raise OSError('Synthetic client disconnected')
    async def receive():
        return {'type': 'http.disconnect'}
    with pytest.raises(ClientDisconnect):
        asyncio.run(response({'type': 'http', 'asgi': {'spec_version': '2.4'}}, receive, send))
    assert stream.closed
    assert blocks == [TRANSFER_CHUNK_BYTES]
