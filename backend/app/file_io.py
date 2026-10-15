"""Matter-file access boundary for protected and synthetic development stores."""
from __future__ import annotations

import hashlib
import shutil
import threading
from pathlib import Path

import pymupdf

from .vault import MAGIC, VaultError, decrypt_bytes, encrypt_bytes

_keys: dict[Path, bytes] = {}
_registry_lock = threading.RLock()


def register(root: Path, key: bytes) -> None:
    with _registry_lock:
        _keys[root.resolve()] = key


def unregister(root: Path) -> None:
    with _registry_lock:
        _keys.pop(root.resolve(), None)


def _key(path: Path) -> bytes | None:
    lexical = Path(path).absolute()
    resolved = lexical.resolve()
    with _registry_lock:
        for root, key in _keys.items():
            if lexical.is_relative_to(root) and not resolved.is_relative_to(root):
                raise VaultError('Matter path escapes protected data root')
            if lexical.is_relative_to(root) and resolved != root:
                return key
    return None


def protected(path: Path) -> bool:
    return _key(Path(path)) is not None


def read_bytes(path: Path) -> bytes:
    path = Path(path)
    key = _key(path)
    if key is None:
        return path.read_bytes()
    with path.open('rb') as stream:
        if stream.read(len(MAGIC)) != MAGIC:
            raise VaultError('Unencrypted matter file in protected store')
    return decrypt_bytes(path, key)


def write_bytes(path: Path, data: bytes) -> None:
    path = Path(path)
    key = _key(path)
    if key is None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    else:
        encrypt_bytes(data, path, key)


def copy_file(source: Path, destination: Path) -> None:
    source, destination = Path(source), Path(destination)
    source_key, dest_key = _key(source), _key(destination)
    if source_key is not None or dest_key is not None:
        write_bytes(destination, read_bytes(source))
    else:
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)


def sha256(path: Path) -> str:
    return hashlib.sha256(read_bytes(path)).hexdigest()


def size(path: Path) -> int:
    if not protected(path):
        return Path(path).stat().st_size
    return len(read_bytes(path))


def open_pdf(path: Path | str):
    path = Path(path)
    if not protected(path):
        return pymupdf.open(path)
    suffix = path.suffix.lower().lstrip('.')
    filetype = 'jpeg' if suffix in ('jpg', 'jpeg') else suffix if suffix in ('pdf', 'png') else 'pdf'
    return pymupdf.open(stream=read_bytes(path), filetype=filetype)


def save_pdf(doc, path: Path, **options) -> None:
    path = Path(path)
    if protected(path):
        write_bytes(path, doc.tobytes(**options))
    else:
        doc.save(path, **options)
