"""Set up, unlock, and resume migration of a local protected matter root."""
from __future__ import annotations

import json
import os
import re
import sqlite3
import threading
from contextlib import contextmanager
from pathlib import Path

from .secure_db import connect_encrypted, is_plain_sqlite, migrate_plain_database
from .vault import (MAGIC, VaultError, create_key_envelope, encrypt_file,
                    rotate_key_envelope, unlock_key)


class VaultStore:
    def __init__(self, root: Path):
        self.root = Path(root).expanduser().resolve()
        self.root.mkdir(mode=0o700, parents=True, exist_ok=True)
        if os.name != 'nt':
            self.root.chmod(0o700)
        self.metadata = self.root / 'vault.json'
        self._lock = threading.RLock()

    def _load(self) -> dict | None:
        if not self.metadata.exists():
            return None
        try:
            value = json.loads(self.metadata.read_text(encoding='utf-8'))
            if value.get('format') != 'courtbundle-vault' or value.get('version') != 1 or value.get('state') not in ('migrating', 'active'):
                raise VaultError('Unsupported vault metadata')
            return value
        except (OSError, ValueError, TypeError, AttributeError) as exc:
            raise VaultError('Vault metadata is damaged; do not create a new vault over this data') from exc

    def _save(self, value: dict) -> None:
        candidate = self.root / ('.vault-' + os.urandom(12).hex())
        try:
            descriptor = os.open(candidate, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(descriptor, 'w', encoding='utf-8') as stream:
                json.dump(value, stream, separators=(',', ':'))
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(candidate, self.metadata)
        finally:
            candidate.unlink(missing_ok=True)

    def status(self) -> str:
        with self._lock:
            metadata = self._load()
            return 'setup' if metadata is None else 'locked'

    def setup(self, password: str) -> tuple[bytes, str]:
        with self._lock:
            if self._load() is not None:
                raise VaultError('Vault already exists; unlock it instead')
            envelope, recovery_key, data_key = create_key_envelope(password)
            with self._migration_lock():
                if self._load() is not None:
                    raise VaultError('Vault already exists; unlock it instead')
                self._save({'format': 'courtbundle-vault', 'version': 1,
                            'state': 'migrating', 'envelope': envelope})
                self._migrate(data_key)
            return data_key, recovery_key

    def unlock(self, credential: str, *, recovery: bool = False) -> bytes:
        with self._lock:
            metadata = self._load()
            if metadata is None:
                raise VaultError('Set up a vault first')
            data_key = unlock_key(metadata['envelope'], credential, recovery=recovery)
            if metadata['state'] == 'migrating':
                with self._migration_lock():
                    self._migrate(data_key)
            database = self.root / 'court_bundle.sqlite3'
            if database.exists():
                with connect_encrypted(database, data_key) as con:
                    if con.execute('PRAGMA integrity_check').fetchone()[0] != 'ok':
                        raise VaultError('Protected database integrity check failed')
            return data_key

    def rotate(self, data_key: bytes, new_password: str) -> str:
        with self._lock:
            metadata = self._load()
            if metadata is None or metadata['state'] != 'active':
                raise VaultError('Vault is not ready')
            envelope, recovery_key = rotate_key_envelope(data_key, new_password)
            metadata['envelope'] = envelope
            self._save(metadata)
            return recovery_key

    def envelope(self) -> dict:
        metadata = self._load()
        if metadata is None or metadata['state'] != 'active':
            raise VaultError('Vault is not ready')
        return metadata['envelope']

    @contextmanager
    def _migration_lock(self):
        """Exclude another engine while plaintext files and SQLite are moved."""
        with (self.root / '.sidecar.lock').open('a+b') as stream:
            try:
                if os.name == 'nt':
                    import msvcrt
                    stream.seek(0)
                    if stream.read(1) != b'1':
                        stream.seek(0)
                        stream.write(b'1')
                        stream.flush()
                    stream.seek(0)
                    msvcrt.locking(stream.fileno(), msvcrt.LK_NBLCK, 1)
                else:
                    import fcntl
                    fcntl.flock(stream.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            except OSError as exc:
                raise VaultError('Matter data is open in another MatterDraft engine') from exc
            try:
                yield
            finally:
                if os.name == 'nt':
                    stream.seek(0)
                    msvcrt.locking(stream.fileno(), msvcrt.LK_UNLCK, 1)
                else:
                    fcntl.flock(stream.fileno(), fcntl.LOCK_UN)

    def _migrate(self, data_key: bytes) -> None:
        metadata = self._load()
        if metadata is None or metadata['state'] != 'migrating':
            raise VaultError('No migration is pending')
        database = self.root / 'court_bundle.sqlite3'
        old_db = self.root / '.migration-plain.sqlite3'
        encrypted_db = self.root / '.migration-encrypted.sqlite3'
        # Recover a rename interrupted after the old database moved aside.
        if not database.exists() and old_db.exists():
            if encrypted_db.exists():
                os.replace(encrypted_db, database)
            else:
                os.replace(old_db, database)
        excluded = {'vault.json', '.sidecar.lock', database.name,
                    database.name + '-wal', database.name + '-shm',
                    old_db.name, encrypted_db.name}
        for path in sorted(self.root.rglob('*')):
            if path.is_symlink() or not path.resolve().is_relative_to(self.root):
                raise VaultError('Unsafe symlink in matter store; migration stopped')
            if not path.is_file() or path.name in excluded:
                continue
            if (path.name.startswith('.vault-') or path.name.endswith('.vault-stage') or
                    re.fullmatch(r'\..+\.encrypted-[0-9a-f]{24}', path.name)):
                path.unlink(missing_ok=True)
                continue
            if path.name.startswith(('.portable-', '.pdf-edit-')):
                path.unlink(missing_ok=True)
                continue
            with path.open('rb') as stream:
                if stream.read(len(MAGIC)) == MAGIC:
                    continue
            candidate = path.with_name('.' + path.name + '.vault-stage')
            candidate.unlink(missing_ok=True)
            encrypt_file(path, candidate, data_key)
            os.replace(candidate, path)
        if database.exists() and is_plain_sqlite(database):
            with sqlite3.connect(database) as con:
                con.execute('PRAGMA wal_checkpoint(TRUNCATE)')
            encrypted_db.unlink(missing_ok=True)
            migrate_plain_database(database, encrypted_db, data_key)
            os.replace(database, old_db)
            os.replace(encrypted_db, database)
        if database.exists():
            with connect_encrypted(database, data_key) as con:
                if con.execute('PRAGMA integrity_check').fetchone()[0] != 'ok':
                    raise VaultError('Encrypted database migration failed integrity check')
                con.execute('PRAGMA wal_checkpoint(TRUNCATE)')
        old_db.unlink(missing_ok=True)
        encrypted_db.unlink(missing_ok=True)
        (self.root / 'court_bundle.sqlite3-wal').unlink(missing_ok=True)
        (self.root / 'court_bundle.sqlite3-shm').unlink(missing_ok=True)
        metadata['state'] = 'active'
        self._save(metadata)
