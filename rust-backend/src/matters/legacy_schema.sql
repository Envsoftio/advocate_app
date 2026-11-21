-- Tables used by the Python reference at schema 29. Applied only to a staged encrypted copy.
CREATE TABLE IF NOT EXISTS documents (
    id TEXT PRIMARY KEY, matter_id TEXT NOT NULL REFERENCES matters(id) ON DELETE CASCADE,
    title TEXT NOT NULL, kind TEXT NOT NULL, part INTEGER NOT NULL,
    label TEXT NOT NULL,
    label_status TEXT NOT NULL DEFAULT 'unreviewed',
    label_source TEXT NOT NULL DEFAULT 'none',
    group_key TEXT NOT NULL,
    group_role TEXT NOT NULL DEFAULT 'member',
    position INTEGER NOT NULL,
    page_count INTEGER NOT NULL, ocr_status TEXT NOT NULL,
    ocr_provenance_json TEXT NOT NULL DEFAULT '[]',
    original_name TEXT NOT NULL, source_sha256 TEXT NOT NULL,
    source_relpath TEXT NOT NULL, working_relpath TEXT NOT NULL,
    certificate_signature_status TEXT NOT NULL DEFAULT 'unknown',
    certificate_signature_count INTEGER NOT NULL DEFAULT 0,
    signature_inspection_note TEXT NOT NULL DEFAULT '',
    visible_signature_status TEXT NOT NULL DEFAULT 'unreviewed',
    signature_status TEXT NOT NULL DEFAULT 'inspection_inconclusive',
    document_date TEXT NOT NULL DEFAULT '',
    date_status TEXT NOT NULL DEFAULT 'unknown',
    date_source TEXT NOT NULL DEFAULT 'none',
    date_note TEXT NOT NULL DEFAULT '',
    notes TEXT NOT NULL DEFAULT '',
    source_file_mtime TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS matters (
    id TEXT PRIMARY KEY, name TEXT NOT NULL,
    court_forum TEXT NOT NULL DEFAULT '', client_reference TEXT NOT NULL DEFAULT '',
    internal_reference TEXT NOT NULL DEFAULT '', notes TEXT NOT NULL DEFAULT '',
    court_profile TEXT NOT NULL, case_type TEXT NOT NULL,
    petitioner TEXT NOT NULL, respondent TEXT NOT NULL,
    advocate TEXT NOT NULL, case_number TEXT NOT NULL,
    max_pages_per_volume INTEGER NOT NULL,
    keep_groups_intact INTEGER NOT NULL,
    parts_json TEXT NOT NULL DEFAULT '[1,2,3,4]',
    next_part INTEGER NOT NULL DEFAULT 5,
    created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS document_revisions (
                document_id TEXT NOT NULL, revision INTEGER NOT NULL,
                working_relpath TEXT NOT NULL, page_count INTEGER NOT NULL,
                ocr_status TEXT NOT NULL, ocr_provenance_json TEXT NOT NULL DEFAULT '[]',
                created_at TEXT NOT NULL,
                PRIMARY KEY(document_id, revision)
            );

CREATE TABLE IF NOT EXISTS jobs (
                id TEXT PRIMARY KEY, matter_id TEXT NOT NULL, kind TEXT NOT NULL,
                target_id TEXT, status TEXT NOT NULL, progress INTEGER NOT NULL,
                message TEXT NOT NULL, error TEXT, result_json TEXT,
                retry_job_id TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            );

CREATE TABLE IF NOT EXISTS memo_parties (
                matter_id TEXT PRIMARY KEY REFERENCES matters(id) ON DELETE CASCADE,
                payload_json TEXT NOT NULL, updated_at TEXT NOT NULL
            );

CREATE TABLE IF NOT EXISTS matter_values (
                matter_id TEXT PRIMARY KEY REFERENCES matters(id) ON DELETE CASCADE,
                payload_json TEXT NOT NULL, updated_at TEXT NOT NULL
            );

CREATE TABLE IF NOT EXISTS edit_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                matter_id TEXT NOT NULL REFERENCES matters(id) ON DELETE CASCADE,
                kind TEXT NOT NULL, before_json TEXT NOT NULL, after_json TEXT NOT NULL,
                applied INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL
            );

CREATE TABLE IF NOT EXISTS audit_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                matter_id TEXT, document_id TEXT, action TEXT NOT NULL, created_at TEXT NOT NULL
            );

CREATE TABLE IF NOT EXISTS manual_bookmarks (
                id TEXT PRIMARY KEY, matter_id TEXT NOT NULL REFERENCES matters(id) ON DELETE CASCADE,
                title TEXT NOT NULL, document_id TEXT NOT NULL REFERENCES documents(id),
                page_index INTEGER NOT NULL, target_revision INTEGER NOT NULL,
                parent_id TEXT, position INTEGER NOT NULL
            );

CREATE TABLE IF NOT EXISTS reviewed_links (
                id TEXT PRIMARY KEY, matter_id TEXT NOT NULL REFERENCES matters(id) ON DELETE CASCADE,
                title TEXT NOT NULL,
                source_document_id TEXT NOT NULL REFERENCES documents(id),
                source_page_index INTEGER NOT NULL, source_revision INTEGER NOT NULL,
                rect_x REAL NOT NULL, rect_y REAL NOT NULL,
                rect_width REAL NOT NULL, rect_height REAL NOT NULL,
                kind TEXT NOT NULL,
                target_document_id TEXT REFERENCES documents(id),
                target_page_index INTEGER, target_revision INTEGER,
                uri TEXT NOT NULL, position INTEGER NOT NULL
            );

CREATE TABLE IF NOT EXISTS output_marks (
                matter_id TEXT PRIMARY KEY REFERENCES matters(id) ON DELETE CASCADE,
                payload_json TEXT NOT NULL, updated_at TEXT NOT NULL
            );

CREATE TABLE IF NOT EXISTS output_presets (
                id TEXT PRIMARY KEY, name TEXT NOT NULL COLLATE NOCASE,
                version INTEGER NOT NULL, payload_json TEXT NOT NULL,
                created_at TEXT NOT NULL, UNIQUE(name,version)
            );

CREATE TABLE IF NOT EXISTS matter_output_presets (
                matter_id TEXT PRIMARY KEY REFERENCES matters(id) ON DELETE CASCADE,
                snapshot_json TEXT NOT NULL, updated_at TEXT NOT NULL
            );

CREATE TABLE IF NOT EXISTS page_annotations (
                id TEXT PRIMARY KEY, matter_id TEXT NOT NULL REFERENCES matters(id) ON DELETE CASCADE,
                document_id TEXT NOT NULL REFERENCES documents(id),
                page_index INTEGER NOT NULL, target_revision INTEGER NOT NULL,
                kind TEXT NOT NULL, rect_x REAL NOT NULL, rect_y REAL NOT NULL,
                rect_width REAL NOT NULL, rect_height REAL NOT NULL,
                body TEXT NOT NULL, color TEXT NOT NULL,
                asset_relpath TEXT, asset_mime TEXT, position INTEGER NOT NULL
            );

CREATE TABLE IF NOT EXISTS reader_positions (
                resource_key TEXT PRIMARY KEY,
                matter_id TEXT NOT NULL REFERENCES matters(id) ON DELETE CASCADE,
                payload_json TEXT NOT NULL, updated_at TEXT NOT NULL
            );

CREATE TABLE IF NOT EXISTS redaction_records (
                id TEXT PRIMARY KEY, document_id TEXT NOT NULL REFERENCES documents(id),
                page_index INTEGER NOT NULL, rect_x REAL NOT NULL, rect_y REAL NOT NULL,
                rect_width REAL NOT NULL, rect_height REAL NOT NULL,
                restore_x REAL NOT NULL, restore_y REAL NOT NULL,
                restore_width REAL NOT NULL, restore_height REAL NOT NULL,
                before_revision INTEGER NOT NULL, after_revision INTEGER NOT NULL,
                active INTEGER NOT NULL DEFAULT 1, removed_revision INTEGER,
                structure_changed INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );

CREATE TABLE IF NOT EXISTS source_page_identities (
    id TEXT PRIMARY KEY, matter_id TEXT NOT NULL REFERENCES matters(id),
    document_id TEXT NOT NULL REFERENCES documents(id),
    working_relpath TEXT NOT NULL, revision INTEGER NOT NULL,
    page_index INTEGER NOT NULL, working_sha256 TEXT NOT NULL,
    UNIQUE(document_id,working_relpath,page_index)
);

CREATE TABLE IF NOT EXISTS supplemental_plans (
    id TEXT PRIMARY KEY, matter_id TEXT NOT NULL REFERENCES matters(id),
    payload_json TEXT NOT NULL, updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS bundle_membership_state (
                matter_id TEXT PRIMARY KEY REFERENCES matters(id) ON DELETE CASCADE,
                revision INTEGER NOT NULL DEFAULT 1
            );

CREATE TABLE IF NOT EXISTS audience_variants (
    id TEXT PRIMARY KEY, matter_id TEXT NOT NULL REFERENCES matters(id) ON DELETE CASCADE,
    payload_json TEXT NOT NULL, updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS ocr_page_reviews (
                document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
                revision INTEGER NOT NULL, page_index INTEGER NOT NULL,
                accepted_at TEXT NOT NULL,
                PRIMARY KEY(document_id,revision,page_index)
            );

CREATE TABLE IF NOT EXISTS export_archive (matter_id TEXT NOT NULL, build_id TEXT NOT NULL, archived_at TEXT NOT NULL, PRIMARY KEY(matter_id,build_id));
