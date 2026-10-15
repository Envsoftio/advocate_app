"""SQLCipher connections and a staged plaintext SQLite migration.

The database key is a random 256-bit data key from the vault envelope, not a
user password. The caller must hold the engine's exclusive data-root lock.
"""
from __future__ import annotations

import os
from pathlib import Path

import sqlcipher3


class EncryptedDatabaseError(ValueError):
    pass


class SecureRow:
    """sqlite3.Row-compatible access for the bundled SQLCipher cursor type."""
    def __init__(self, cursor, values):
        self._values = values
        self._names = [column[0] for column in cursor.description]

    def __getitem__(self, key):
        return self._values[key] if isinstance(key, int) else self._values[self._names.index(key)]

    def __iter__(self):
        return iter(self._values)

    def keys(self):
        return self._names

    def __len__(self):
        return len(self._values)


def is_plain_sqlite(path: Path) -> bool:
    if not path.is_file():
        return False
    with path.open('rb') as stream:
        return stream.read(16) == b'SQLite format 3\x00'


def connect_encrypted(path: Path, data_key: bytes):
    if len(data_key) != 32:
        raise EncryptedDatabaseError('Invalid database key')
    if is_plain_sqlite(path):
        raise EncryptedDatabaseError('Plaintext database requires migration before protected opening')
    con = sqlcipher3.connect(path, timeout=30)
    try:
        con.execute(f'''PRAGMA key = "x'{data_key.hex()}'"''')
        if not con.execute('PRAGMA cipher_version').fetchone():
            raise EncryptedDatabaseError('SQLCipher is unavailable')
        # This forces wrong keys and corrupted databases to fail before the
        # caller can create or modify any table.
        con.execute('SELECT count(*) FROM sqlite_master').fetchone()
        con.row_factory = SecureRow
        return con
    except BaseException:
        con.close()
        raise


def migrate_plain_database(source: Path, destination: Path, data_key: bytes) -> None:
    """Publish an encrypted DB after integrity checks while retaining source.

    The source is intentionally retained; the caller must not remove it until
    every matter file has crossed the same protected migration boundary.
    """
    if not is_plain_sqlite(source) or destination.exists() or source.resolve() == destination.resolve():
        raise EncryptedDatabaseError('Expected a plaintext source and unused destination')
    if len(data_key) != 32:
        raise EncryptedDatabaseError('Invalid database key')
    destination.parent.mkdir(parents=True, exist_ok=True)
    candidate = destination.with_name('.' + destination.name + '.migration-' + os.urandom(12).hex())
    try:
        # Hold an exclusive engine lock outside this function so the plaintext
        # database and its WAL cannot change during the export.
        with sqlcipher3.connect(source) as snapshot:
            snapshot.execute('SELECT count(*) FROM sqlite_master').fetchone()
            snapshot.execute(f'''ATTACH DATABASE ? AS protected KEY "x'{data_key.hex()}'"''',
                             (str(candidate),))
            snapshot.execute("SELECT sqlcipher_export('protected')")
            snapshot.execute('PRAGMA protected.user_version = ' +
                             str(snapshot.execute('PRAGMA user_version').fetchone()[0]))
            snapshot.execute('DETACH DATABASE protected')
        if os.name != 'nt':
            candidate.chmod(0o600)
        with connect_encrypted(candidate, data_key) as check:
            result = check.execute('PRAGMA integrity_check').fetchone()
            if result is None or result[0] != 'ok':
                raise EncryptedDatabaseError('Encrypted database integrity check failed')
        os.replace(candidate, destination)
    finally:
        candidate.unlink(missing_ok=True)
