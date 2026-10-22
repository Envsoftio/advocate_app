"""Bounded PDF transfer for immutable generated artifacts, including vault files."""
from __future__ import annotations

import re
from pathlib import Path
from urllib.parse import quote

from fastapi import Request
from fastapi.responses import Response, StreamingResponse

from .file_io import protected
from .vault import EncryptedReader, VaultError

TRANSFER_CHUNK_BYTES = 256 * 1024


def _range(header: str, size: int) -> tuple[int, int] | None:
    """Inclusive single byte range; PDF.js does not need multipart ranges."""
    match = re.fullmatch(r'bytes=(\d*)-(\d*)', header.strip())
    if not match or not any(match.groups()) or size <= 0:
        return None
    first, last = match.groups()
    if len(first) > 20 or len(last) > 20:
        return None
    if not first:
        suffix = int(last)
        return (max(0, size - suffix), size - 1) if suffix > 0 else None
    start, end = int(first), int(last) if last else size - 1
    if start >= size or end < start:
        return None
    return start, min(end, size - 1)


class _ClosingStreamResponse(StreamingResponse):
    """Starlette's normal background task is skipped on an ASGI send failure."""

    def __init__(self, *args, close, **kwargs):
        super().__init__(*args, **kwargs)
        self._close = close

    async def __call__(self, scope, receive, send):
        try:
            await super().__call__(scope, receive, send)
        finally:
            self._close()


def pdf_stream_response(path: Path, request: Request, filename: str,
                        data_key: bytes | None = None) -> Response:
    """Stream one range without a plaintext scratch file or whole-PDF buffer.

    EncryptedReader authenticates the complete chunk index before serving data,
    then retains one decrypted vault chunk. Artifact hash/identity validation
    remains the export store's responsibility and runs before this function.
    """
    if protected(path):
        if data_key is None:
            raise VaultError('The protected PDF is unavailable while the vault is locked')
        stream = EncryptedReader(path, data_key)
    else:
        stream = path.open('rb')
    try:
        size = stream.seek(0, 2)
        headers = {'Accept-Ranges': 'bytes', 'Cache-Control': 'no-store',
                   'Content-Disposition': f"attachment; filename*=UTF-8''{quote(filename, safe='')}"}
        # Without an ETag/Last-Modified validator, an If-Range request must fall
        # back to the complete representation rather than assert a match.
        requested = request.headers.get('range') if not request.headers.get('if-range') else None
        interval = _range(requested, size) if requested else (0, size - 1)
        if interval is None:
            stream.close()
            return Response(status_code=416, headers={**headers, 'Content-Range': f'bytes */{size}'})
        start, end = interval
        length = max(0, end - start + 1)
        headers['Content-Length'] = str(length)
        if requested:
            headers['Content-Range'] = f'bytes {start}-{end}/{size}'
        status = 206 if requested else 200
        if request.method == 'HEAD':
            stream.close()
            return Response(status_code=status, media_type='application/pdf', headers=headers)
        stream.seek(start)

        def chunks():
            remaining = length
            try:
                while remaining:
                    block = stream.read(min(TRANSFER_CHUNK_BYTES, remaining))
                    if not block:
                        raise OSError('Generated PDF ended before its declared length')
                    remaining -= len(block)
                    yield block
            finally:
                stream.close()

        return _ClosingStreamResponse(chunks(), close=stream.close, status_code=status,
                                      media_type='application/pdf', headers=headers)
    except BaseException:
        stream.close()
        raise
