import sqlite3

import pytest

from app.secure_db import (EncryptedDatabaseError, connect_encrypted,
                           is_plain_sqlite, migrate_plain_database)
from app.storage import Store


def test_sqlcipher_migration_preserves_committed_wal_and_rejects_wrong_key(tmp_path):
    source = tmp_path / 'legacy.sqlite3'
    destination = tmp_path / 'protected.sqlite3'
    data_key = bytes(range(32))
    con = sqlite3.connect(source)
    try:
        con.execute('PRAGMA journal_mode=WAL')
        con.execute('PRAGMA user_version=6')
        con.execute('CREATE TABLE matters (name TEXT)')
        con.execute('INSERT INTO matters VALUES (?)', ('Synthetic matter',))
        con.commit()
        assert is_plain_sqlite(source)
        migrate_plain_database(source, destination, data_key)
        assert not is_plain_sqlite(destination)
        with connect_encrypted(destination, data_key) as protected:
            assert protected.execute('SELECT name FROM matters').fetchone()[0] == 'Synthetic matter'
            assert protected.execute('PRAGMA user_version').fetchone()[0] == 6
        with pytest.raises(Exception):
            connect_encrypted(destination, b'X' * 32)
        assert con.execute('SELECT name FROM matters').fetchone()[0] == 'Synthetic matter'
    finally:
        con.close()


def test_migration_never_overwrites_prior_destination(tmp_path):
    source = tmp_path / 'legacy.sqlite3'
    destination = tmp_path / 'protected.sqlite3'
    with sqlite3.connect(source) as con:
        con.execute('CREATE TABLE synthetic (value TEXT)')
    destination.write_bytes(b'previous encrypted file')
    with pytest.raises(EncryptedDatabaseError):
        migrate_plain_database(source, destination, b'K' * 32)
    assert destination.read_bytes() == b'previous encrypted file'
    assert list(tmp_path.glob('.protected.sqlite3.migration-*')) == []


def test_store_can_use_protected_database_and_requires_key_to_reopen(tmp_path):
    key = b'K' * 32
    store = Store(tmp_path, data_key=key)
    try:
        matter = store.create_matter({'name': 'Synthetic protected metadata',
            'court_profile': 'delhi-hc-illustrative', 'case_type': 'Synthetic',
            'petitioner': '', 'respondent': '', 'advocate': '', 'case_number': '',
            'max_pages_per_volume': 10, 'keep_groups_intact': True})
    finally:
        store.close()
    assert not is_plain_sqlite(tmp_path / 'court_bundle.sqlite3')
    assert b'Synthetic protected metadata' not in (tmp_path / 'court_bundle.sqlite3').read_bytes()
    with pytest.raises(RuntimeError, match='unlock key'):
        Store(tmp_path)
    reopened = Store(tmp_path, data_key=key)
    try:
        assert reopened.get_matter(matter['id'])['name'] == 'Synthetic protected metadata'
    finally:
        reopened.close()


def test_existing_store_metadata_can_be_migrated_without_losing_source(tmp_path):
    legacy = Store(tmp_path)
    try:
        matter = legacy.create_matter({'name': 'Synthetic legacy matter',
            'court_profile': 'delhi-hc-illustrative', 'case_type': 'Synthetic',
            'petitioner': '', 'respondent': '', 'advocate': '', 'case_number': '',
            'max_pages_per_volume': 10, 'keep_groups_intact': True})
    finally:
        legacy.close()
    source = tmp_path / 'court_bundle.sqlite3'
    encrypted = tmp_path / 'encrypted-candidate.sqlite3'
    key = b'M' * 32
    migrate_plain_database(source, encrypted, key)
    assert is_plain_sqlite(source)
    source.rename(tmp_path / 'retained-legacy.sqlite3')
    encrypted.rename(source)
    migrated = Store(tmp_path, data_key=key)
    try:
        assert migrated.get_matter(matter['id'])['name'] == 'Synthetic legacy matter'
    finally:
        migrated.close()
