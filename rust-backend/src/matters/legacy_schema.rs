//! Bring a private copy of a Python database to the last Python schema.
use rusqlite::{Connection, TransactionBehavior};
use std::collections::HashSet;

pub(super) const VERSION: u32 = 29;

fn add_missing(con: &Connection, table: &str, columns: &[(&str, &str)]) -> rusqlite::Result<()> {
    let mut query = con.prepare(&format!("PRAGMA table_info({table})"))?;
    let current = query
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    drop(query);
    for &(name, definition) in columns {
        if !current.contains(name) {
            con.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {name} {definition}"
            ))?;
        }
    }
    Ok(())
}

pub(super) fn upgrade(connection: &mut Connection, version: u32) -> rusqlite::Result<()> {
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(include_str!("legacy_schema.sql"))?;
    add_missing(
        &tx,
        "matters",
        &[
            ("archived_at", "TEXT"),
            ("court_forum", "TEXT NOT NULL DEFAULT ''"),
            ("client_reference", "TEXT NOT NULL DEFAULT ''"),
            ("internal_reference", "TEXT NOT NULL DEFAULT ''"),
            ("notes", "TEXT NOT NULL DEFAULT ''"),
            ("parts_json", "TEXT NOT NULL DEFAULT '[1,2,3,4]'"),
            ("next_part", "INTEGER NOT NULL DEFAULT 5"),
        ],
    )?;
    add_missing(
        &tx,
        "documents",
        &[
            ("revision", "INTEGER NOT NULL DEFAULT 1"),
            ("archived_at", "TEXT"),
            ("document_date", "TEXT NOT NULL DEFAULT ''"),
            ("date_status", "TEXT NOT NULL DEFAULT 'unknown'"),
            ("date_source", "TEXT NOT NULL DEFAULT 'none'"),
            ("date_note", "TEXT NOT NULL DEFAULT ''"),
            ("source_file_mtime", "TEXT NOT NULL DEFAULT ''"),
            ("label_status", "TEXT NOT NULL DEFAULT 'unreviewed'"),
            ("label_source", "TEXT NOT NULL DEFAULT 'none'"),
            ("group_role", "TEXT NOT NULL DEFAULT 'member'"),
            (
                "certificate_signature_status",
                "TEXT NOT NULL DEFAULT 'unknown'",
            ),
            ("certificate_signature_count", "INTEGER NOT NULL DEFAULT 0"),
            ("signature_inspection_note", "TEXT NOT NULL DEFAULT ''"),
            (
                "visible_signature_status",
                "TEXT NOT NULL DEFAULT 'unreviewed'",
            ),
            (
                "signature_status",
                "TEXT NOT NULL DEFAULT 'inspection_inconclusive'",
            ),
            ("ocr_provenance_json", "TEXT NOT NULL DEFAULT '[]'"),
            (
                "bundle_included",
                "INTEGER NOT NULL DEFAULT 1 CHECK(bundle_included IN (0,1))",
            ),
            ("notes", "TEXT NOT NULL DEFAULT ''"),
        ],
    )?;
    add_missing(
        &tx,
        "document_revisions",
        &[("ocr_provenance_json", "TEXT NOT NULL DEFAULT '[]'")],
    )?;
    add_missing(
        &tx,
        "page_annotations",
        &[
            ("output_scope", "TEXT NOT NULL DEFAULT 'private'"),
            ("created_at", "TEXT"),
            ("updated_at", "TEXT"),
            ("text_style_json", "TEXT NOT NULL DEFAULT '{}'"),
        ],
    )?;
    add_missing(
        &tx,
        "jobs",
        &[
            ("started_at", "TEXT"),
            ("context_json", "TEXT"),
            ("events_json", "TEXT"),
        ],
    )?;
    if version < 9 {
        tx.execute_batch(
            "UPDATE documents SET label_source='legacy' WHERE label<>'' AND label_source='none'",
        )?;
    }
    if version < 18 {
        tx.execute_batch("UPDATE page_annotations SET output_scope='pdf' WHERE kind='image'")?;
    }
    tx.execute_batch(
        "INSERT OR IGNORE INTO document_revisions
         (document_id,revision,working_relpath,page_count,ocr_status,created_at)
         SELECT id,revision,working_relpath,page_count,ocr_status,updated_at FROM documents",
    )?;
    tx.pragma_update(None, "user_version", VERSION)?;
    tx.commit()
}
