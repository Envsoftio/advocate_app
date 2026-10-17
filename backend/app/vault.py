"""Versioned encryption primitives for the future protected matter store.

This module deliberately does not make the current Store encrypted. Callers
must not describe an encrypted file as protection for adjacent plaintext data.
"""
from __future__ import annotations

import base64
import binascii
from bisect import bisect_right
import hashlib
import io
import os
import secrets
import struct
from contextlib import contextmanager
from pathlib import Path

from cryptography.exceptions import InvalidTag
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

MAGIC = b'CBENC1\n'
CHUNK = 1024 * 1024
KDF_N = 1 << 15
KDF_R = 8
KDF_P = 1


class VaultError(ValueError):
    """Invalid credential, damaged encrypted data, or unsupported format."""


def _b64(data: bytes) -> str:
    return base64.b64encode(data).decode('ascii')


def _unb64(value: str, size: int) -> bytes:
    try:
        data = base64.b64decode(value, validate=True)
    except (ValueError, TypeError, binascii.Error) as exc:
        raise VaultError('Invalid vault metadata') from exc
    if len(data) != size:
        raise VaultError('Invalid vault metadata')
    return data


def _derive(secret: bytes, salt: bytes) -> bytes:
    return hashlib.scrypt(secret, salt=salt, n=KDF_N, r=KDF_R, p=KDF_P,
                          dklen=32, maxmem=128 * 1024 * 1024)


def _slot(data_key: bytes, secret: bytes, kind: str) -> dict:
    salt, nonce = secrets.token_bytes(16), secrets.token_bytes(12)
    wrapped = AESGCM(_derive(secret, salt)).encrypt(
        nonce, data_key, b'CourtBundle vault key v1:' + kind.encode('ascii'))
    return {'salt': _b64(salt), 'nonce': _b64(nonce), 'wrapped': _b64(wrapped)}


def create_key_envelope(password: str) -> tuple[dict, str, bytes]:
    """Return public metadata, a one-time recovery secret, and the data key.

    The caller must show the recovery secret once and never persist it beside
    the envelope. The data key must stay in process memory only.
    """
    if not isinstance(password, str) or len(password) < 6:
        raise VaultError('Password must contain at least 6 characters')
    data_key = secrets.token_bytes(32)
    recovery = secrets.token_bytes(32)
    envelope = {'format': 'courtbundle-key-envelope', 'version': 1,
                'kdf': {'name': 'scrypt', 'n': KDF_N, 'r': KDF_R, 'p': KDF_P},
                'password': _slot(data_key, password.encode('utf-8'), 'password'),
                'recovery': _slot(data_key, recovery, 'recovery')}
    return envelope, recovery.hex(), data_key


def unlock_key(envelope: dict, credential: str, *, recovery: bool = False) -> bytes:
    if not isinstance(envelope, dict) or envelope.get('format') != 'courtbundle-key-envelope' or envelope.get('version') != 1 or envelope.get('kdf') != {'name': 'scrypt', 'n': KDF_N, 'r': KDF_R, 'p': KDF_P}:
        raise VaultError('Unsupported vault key format')
    kind = 'recovery' if recovery else 'password'
    try:
        slot = envelope[kind]
        salt = _unb64(slot['salt'], 16)
        nonce = _unb64(slot['nonce'], 12)
        wrapped = _unb64(slot['wrapped'], 48)
        secret = bytes.fromhex(credential) if recovery else credential.encode('utf-8')
        if recovery and len(secret) != 32:
            raise ValueError('Invalid recovery length')
        return AESGCM(_derive(secret, salt)).decrypt(
            nonce, wrapped, b'CourtBundle vault key v1:' + kind.encode('ascii'))
    except (KeyError, TypeError, AttributeError, ValueError, InvalidTag) as exc:
        raise VaultError('Invalid credential or damaged vault key') from exc


def rotate_key_envelope(data_key: bytes, password: str) -> tuple[dict, str]:
    """Replace both credential slots, revoking the previous recovery key."""
    if len(data_key) != 32:
        raise VaultError('Invalid data key')
    if not isinstance(password, str) or len(password) < 6:
        raise VaultError('Password must contain at least 6 characters')
    recovery = secrets.token_bytes(32)
    envelope = {'format': 'courtbundle-key-envelope', 'version': 1,
                'kdf': {'name': 'scrypt', 'n': KDF_N, 'r': KDF_R, 'p': KDF_P},
                'password': _slot(data_key, password.encode('utf-8'), 'password'),
                'recovery': _slot(data_key, recovery, 'recovery')}
    return envelope, recovery.hex()


def _aad(index: int, final: bool) -> bytes:
    return MAGIC + struct.pack('>Q?', index, final)


def _private_candidate(path: Path):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    return os.fdopen(descriptor, 'wb')


def _encrypt_stream(reader, writer, data_key: bytes) -> None:
    cipher = AESGCM(data_key)
    writer.write(MAGIC)
    index = 0
    while chunk := reader.read(CHUNK):
        nonce = secrets.token_bytes(12)
        encrypted = cipher.encrypt(nonce, chunk, _aad(index, False))
        writer.write(struct.pack('>I', len(encrypted)))
        writer.write(nonce + encrypted)
        index += 1
    nonce = secrets.token_bytes(12)
    final = cipher.encrypt(nonce, b'', _aad(index, True))
    writer.write(struct.pack('>I', len(final)))
    writer.write(nonce + final)


def _decrypt_chunks(reader, data_key: bytes):
    cipher = AESGCM(data_key)
    if reader.read(len(MAGIC)) != MAGIC:
        raise VaultError('Unsupported encrypted file format')
    index = 0
    while True:
        length_bytes = reader.read(4)
        if len(length_bytes) != 4:
            raise VaultError('Truncated encrypted file')
        length = struct.unpack('>I', length_bytes)[0]
        if not 16 <= length <= CHUNK + 16:
            raise VaultError('Invalid encrypted chunk length')
        nonce = reader.read(12)
        encrypted = reader.read(length)
        if len(nonce) != 12 or len(encrypted) != length:
            raise VaultError('Truncated encrypted file')
        final = length == 16
        try:
            chunk = cipher.decrypt(nonce, encrypted, _aad(index, final))
        except InvalidTag as exc:
            raise VaultError('Encrypted file authentication failed') from exc
        if final:
            if reader.read(1):
                raise VaultError('Trailing encrypted data')
            return
        yield chunk
        index += 1


def encrypt_bytes(data: bytes, destination: Path, data_key: bytes) -> None:
    if len(data_key) != 32:
        raise VaultError('Invalid data key')
    destination.parent.mkdir(parents=True, exist_ok=True)
    candidate = destination.with_name('.' + destination.name + '.encrypted-' + secrets.token_hex(12))
    try:
        with _private_candidate(candidate) as writer:
            _encrypt_stream(io.BytesIO(data), writer, data_key)
            writer.flush()
            os.fsync(writer.fileno())
        os.replace(candidate, destination)
    finally:
        candidate.unlink(missing_ok=True)


def decrypt_bytes(source: Path, data_key: bytes) -> bytes:
    if len(data_key) != 32:
        raise VaultError('Invalid data key')
    with source.open('rb') as reader:
        return b''.join(_decrypt_chunks(reader, data_key))


def decrypted_chunks(source: Path, data_key: bytes):
    """Yield authenticated plaintext chunks without buffering the whole file."""
    if len(data_key) != 32:
        raise VaultError('Invalid data key')
    with source.open('rb') as reader:
        yield from _decrypt_chunks(reader, data_key)


def decrypt_blob(data: bytes, data_key: bytes) -> bytes:
    if len(data_key) != 32:
        raise VaultError('Invalid data key')
    return b''.join(_decrypt_chunks(io.BytesIO(data), data_key))


class EncryptedReader:
    """Seekable, authenticated view of a chunk-encrypted file.

    ZIP needs seeks into its central directory. Indexing authenticates every
    chunk and the final marker without publishing plaintext to disk; later
    reads hold at most one decrypted chunk in memory. A nested reader can be
    used for an encrypted portable package inside encrypted upload scratch.
    """

    def __init__(self, source, data_key: bytes, *, offset: int = 0):
        if len(data_key) != 32:
            raise VaultError('Invalid data key')
        self._owned = isinstance(source, (str, Path))
        self._source = Path(source).open('rb') if self._owned else source
        self._cipher = AESGCM(data_key)
        self._offset = offset
        self._position = 0
        self._starts: list[int] = []
        self._chunks: list[tuple[int, int]] = []
        self._cached_index = -1
        self._cached = b''
        self._length = 0
        try:
            self._index()
        except BaseException:
            self.close()
            raise

    def _index(self) -> None:
        source = self._source
        source.seek(self._offset)
        if source.read(len(MAGIC)) != MAGIC:
            raise VaultError('Unsupported encrypted file format')
        index = 0
        while True:
            length_bytes = source.read(4)
            if len(length_bytes) != 4:
                raise VaultError('Truncated encrypted file')
            length = struct.unpack('>I', length_bytes)[0]
            if not 16 <= length <= CHUNK + 16:
                raise VaultError('Invalid encrypted chunk length')
            payload_offset = source.tell()
            nonce = source.read(12)
            encrypted = source.read(length)
            if len(nonce) != 12 or len(encrypted) != length:
                raise VaultError('Truncated encrypted file')
            final = length == 16
            try:
                plaintext = self._cipher.decrypt(nonce, encrypted, _aad(index, final))
            except InvalidTag as exc:
                raise VaultError('Encrypted file authentication failed') from exc
            if final:
                if source.read(1):
                    raise VaultError('Trailing encrypted data')
                return
            self._starts.append(self._length)
            self._chunks.append((payload_offset, length))
            self._length += len(plaintext)
            index += 1

    def read(self, size: int = -1) -> bytes:
        if size is None or size < 0:
            size = self._length - self._position
        size = max(0, min(size, self._length - self._position))
        parts = []
        while size:
            index = bisect_right(self._starts, self._position) - 1
            if index != self._cached_index:
                payload_offset, length = self._chunks[index]
                self._source.seek(payload_offset)
                nonce = self._source.read(12)
                encrypted = self._source.read(length)
                if len(nonce) != 12 or len(encrypted) != length:
                    raise VaultError('Truncated encrypted file')
                try:
                    self._cached = self._cipher.decrypt(nonce, encrypted, _aad(index, False))
                except InvalidTag as exc:
                    raise VaultError('Encrypted file authentication failed') from exc
                self._cached_index = index
            within = self._position - self._starts[index]
            part = self._cached[within:within + size]
            parts.append(part)
            self._position += len(part)
            size -= len(part)
        return b''.join(parts)

    def seek(self, offset: int, whence: int = 0) -> int:
        base = 0 if whence == 0 else self._position if whence == 1 else self._length if whence == 2 else None
        if base is None:
            raise ValueError('Invalid seek mode')
        position = base + offset
        if position < 0:
            raise ValueError('Negative seek position')
        self._position = position
        return position

    def tell(self) -> int:
        return self._position

    def seekable(self) -> bool:
        return True

    def readable(self) -> bool:
        return True

    def close(self) -> None:
        if self._owned:
            self._source.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()


def encrypt_file(source: Path, destination: Path, data_key: bytes) -> None:
    """Encrypt a file in bounded chunks, atomically publishing the result."""
    if len(data_key) != 32 or source.resolve() == destination.resolve():
        raise VaultError('Invalid encryption input')
    destination.parent.mkdir(parents=True, exist_ok=True)
    candidate = destination.with_name('.' + destination.name + '.encrypted-' + secrets.token_hex(12))
    try:
        with source.open('rb') as reader, _private_candidate(candidate) as writer:
            _encrypt_stream(reader, writer, data_key)
            writer.flush()
            os.fsync(writer.fileno())
        os.replace(candidate, destination)
    finally:
        candidate.unlink(missing_ok=True)


class _EncryptedSink:
    """Unseekable ZIP output that authenticates bounded plaintext chunks."""
    def __init__(self, writer, data_key: bytes):
        self.writer = writer
        self.cipher = AESGCM(data_key)
        self.pending = bytearray()
        self.index = 0
        self.position = 0
        writer.write(MAGIC)

    def write(self, data: bytes) -> int:
        self.pending.extend(data)
        self.position += len(data)
        while len(self.pending) >= CHUNK:
            chunk = bytes(self.pending[:CHUNK])
            del self.pending[:CHUNK]
            self._chunk(chunk, False)
        return len(data)

    def tell(self) -> int:
        return self.position

    def flush(self) -> None:
        self.writer.flush()

    def _chunk(self, data: bytes, final: bool) -> None:
        nonce = secrets.token_bytes(12)
        encrypted = self.cipher.encrypt(nonce, data, _aad(self.index, final))
        self.writer.write(struct.pack('>I', len(encrypted)))
        self.writer.write(nonce + encrypted)
        self.index += 1

    def finish(self) -> None:
        if self.pending:
            self._chunk(bytes(self.pending), False)
            self.pending.clear()
        self._chunk(b'', True)
        self.writer.flush()
        os.fsync(self.writer.fileno())


@contextmanager
def encrypted_sink(destination: Path, data_key: bytes):
    """Publish an encrypted stream only after its final authenticated marker."""
    if len(data_key) != 32:
        raise VaultError('Invalid data key')
    destination.parent.mkdir(parents=True, exist_ok=True)
    candidate = destination.with_name('.' + destination.name + '.encrypted-' + secrets.token_hex(12))
    try:
        with _private_candidate(candidate) as writer:
            sink = _EncryptedSink(writer, data_key)
            yield sink
            sink.finish()
        os.replace(candidate, destination)
    finally:
        candidate.unlink(missing_ok=True)


def decrypt_file(source: Path, destination: Path, data_key: bytes) -> None:
    """Verify every chunk and the end marker before publishing plaintext.

    Decrypted output is sensitive. The caller must use a protected destination
    and remove it promptly; this function is not an at-rest storage design.
    """
    if len(data_key) != 32 or source.resolve() == destination.resolve():
        raise VaultError('Invalid decryption input')
    destination.parent.mkdir(parents=True, exist_ok=True)
    candidate = destination.with_name('.' + destination.name + '.decrypted-' + secrets.token_hex(12))
    try:
        with source.open('rb') as reader, _private_candidate(candidate) as writer:
            for chunk in _decrypt_chunks(reader, data_key):
                writer.write(chunk)
            writer.flush()
            os.fsync(writer.fileno())
        os.replace(candidate, destination)
    finally:
        candidate.unlink(missing_ok=True)
