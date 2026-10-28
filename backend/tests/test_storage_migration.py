import sqlite3

import pytest

from app.storage import Store


def test_legacy_document_schema_migrates_without_changing_records(tmp_path):
    db = tmp_path / 'court_bundle.sqlite3'
    with sqlite3.connect(db) as con:
        con.execute('''CREATE TABLE documents (
            id TEXT PRIMARY KEY, matter_id TEXT NOT NULL, title TEXT NOT NULL,
            kind TEXT NOT NULL, part INTEGER NOT NULL, label TEXT NOT NULL,
            group_key TEXT NOT NULL, position INTEGER NOT NULL, page_count INTEGER NOT NULL,
            ocr_status TEXT NOT NULL, original_name TEXT NOT NULL, source_sha256 TEXT NOT NULL,
            source_relpath TEXT NOT NULL, working_relpath TEXT NOT NULL,
            created_at TEXT NOT NULL, updated_at TEXT NOT NULL)''')
        con.execute('''INSERT INTO documents VALUES
            ('d','m','Synthetic','other',1,'P-1','',0,2,'not_requested',
             'source.pdf','hash','originals/d.pdf','working/d.pdf','start','last')''')
    store = Store(tmp_path)
    with store.connect() as con:
        row = con.execute('SELECT * FROM documents WHERE id=?', ('d',)).fetchone()
        revision = con.execute('SELECT * FROM document_revisions WHERE document_id=?', ('d',)).fetchone()
        assert con.execute('PRAGMA user_version').fetchone()[0] == 19
    assert row['bundle_included'] == 1
    assert row['revision'] == 1
    assert row['working_relpath'] == 'working/d.pdf'
    assert row['document_date'] == ''
    assert row['date_status'] == 'unknown'
    assert row['date_note'] == ''
    assert row['label_status'] == 'unreviewed'
    assert row['label_source'] == 'legacy'
    assert row['label'] == 'P-1'
    assert row['group_role'] == 'member'
    assert row['certificate_signature_status'] == 'unknown'
    assert row['visible_signature_status'] == 'unreviewed'
    assert row['signature_status'] == 'inspection_inconclusive'
    assert row['ocr_provenance_json'] == '[]'
    assert revision['working_relpath'] == row['working_relpath']
    assert revision['ocr_provenance_json'] == '[]'
    store.close()


def test_v5_store_gains_reusable_values_without_changing_matter(tmp_path):
    first = Store(tmp_path)
    matter = first.create_matter(dict(name='Synthetic matter', court_profile='delhi-hc-illustrative',
        case_type='Demo', petitioner='', respondent='', advocate='', case_number='',
        max_pages_per_volume=10, keep_groups_intact=True))
    first.close()
    with sqlite3.connect(tmp_path / 'court_bundle.sqlite3') as con:
        con.execute('DROP TABLE matter_values')
        con.execute('PRAGMA user_version=5')
    reopened = Store(tmp_path)
    assert reopened.get_matter(matter['id'])['name'] == 'Synthetic matter'
    assert reopened.get_matter_values(matter['id']) == {}
    with reopened.connect() as con:
        assert con.execute('PRAGMA user_version').fetchone()[0] == 19
    reopened.close()


def test_second_engine_cannot_open_active_matter_store(tmp_path):
    first = Store(tmp_path)
    try:
        with pytest.raises(RuntimeError, match='already open'):
            Store(tmp_path)
    finally:
        first.close()
    reopened = Store(tmp_path)
    reopened.close()


def test_future_schema_refused_without_downgrade(tmp_path):
    db = tmp_path / 'court_bundle.sqlite3'
    with sqlite3.connect(db) as con:
        con.execute('PRAGMA user_version=99')
    with pytest.raises(RuntimeError, match='Unsupported matter database schema'):
        Store(tmp_path)
    with sqlite3.connect(db) as con:
        assert con.execute('PRAGMA user_version').fetchone()[0] == 99
