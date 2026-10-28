from __future__ import annotations
import hashlib
import json
import os
import re
import shutil
import sqlite3
import threading
import uuid
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
from pathlib import Path

# PyMuPDF's shared native state must not be used concurrently by request and
# background threads, including requests for different matters or stores.
PDF_PROCESSING_LOCK = threading.Lock()

def utcnow() -> str:
    return datetime.now(timezone.utc).isoformat(timespec='microseconds')


def new_id() -> str:
    return uuid.uuid4().hex


class Store:
    HISTORY_LIMIT = 100
    def __init__(self, root: Path, data_key: bytes | None = None):
        self.root = Path(root).expanduser().resolve()
        self._data_key = data_key
        self.root.mkdir(mode=0o700, parents=True, exist_ok=True)
        if os.name != 'nt':
            # The data root is an application-owned trust boundary. Existing stores
            # may predate this setting, so tighten them when the engine opens.
            self.root.chmod(0o700)
        self._lock_file = (self.root / '.sidecar.lock').open('a+b')
        try:
            if os.name == 'nt':
                import msvcrt
                self._lock_file.seek(0)
                if self._lock_file.read(1) != b'1':
                    self._lock_file.seek(0)
                    self._lock_file.write(b'1')
                    self._lock_file.flush()
                self._lock_file.seek(0)
                msvcrt.locking(self._lock_file.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(self._lock_file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as exc:
            self._lock_file.close()
            raise RuntimeError('Matter data is already open in another CourtBundle engine') from exc
        self.dbpath = self.root / 'court_bundle.sqlite3'
        if self._data_key is not None:
            from .file_io import register
            register(self.root, self._data_key)
        try:
            self._initialize()
            self.recover_temporary_packages()
            self.recover_orphan_working_revisions()
        except BaseException:
            self.close()
            raise

    def close(self):
        if self._data_key is not None:
            from .file_io import unregister
            unregister(self.root)
        if not self._lock_file.closed:
            if os.name == 'nt':
                import msvcrt
                self._lock_file.seek(0)
                msvcrt.locking(self._lock_file.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                import fcntl
                fcntl.flock(self._lock_file.fileno(), fcntl.LOCK_UN)
            self._lock_file.close()

    def matter_lock(self, matter_id: str) -> threading.Lock:
        """Serialize PDF work process-wide as well as protecting matter revisions."""
        return PDF_PROCESSING_LOCK

    @contextmanager
    def connect(self):
        if self._data_key is None:
            from .secure_db import is_plain_sqlite
            if self.dbpath.exists() and not is_plain_sqlite(self.dbpath):
                raise RuntimeError('Encrypted matter database requires an unlock key')
            con = sqlite3.connect(self.dbpath, timeout=30)
        else:
            from .secure_db import connect_encrypted
            con = connect_encrypted(self.dbpath, self._data_key)
        if self._data_key is None:
            con.row_factory = sqlite3.Row
        con.execute('PRAGMA foreign_keys=ON')
        con.execute('PRAGMA busy_timeout=30000')
        try:
            yield con
            con.commit()
        except BaseException:
            con.rollback()
            raise
        finally:
            con.close()

    def _initialize(self):
        with self.connect() as con:
            version = con.execute('PRAGMA user_version').fetchone()[0]
            if version > 19:
                raise RuntimeError(f'Unsupported matter database schema version {version}')
            con.execute('PRAGMA journal_mode=WAL')
            con.executescript('''
                CREATE TABLE IF NOT EXISTS matters (
                    id TEXT PRIMARY KEY, name TEXT NOT NULL,
                    court_profile TEXT NOT NULL, case_type TEXT NOT NULL,
                    petitioner TEXT NOT NULL, respondent TEXT NOT NULL,
                    advocate TEXT NOT NULL, case_number TEXT NOT NULL,
                    max_pages_per_volume INTEGER NOT NULL,
                    keep_groups_intact INTEGER NOT NULL,
                    created_at TEXT NOT NULL, updated_at TEXT NOT NULL
                );
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
                    source_file_mtime TEXT NOT NULL DEFAULT '',
                    created_at TEXT NOT NULL, updated_at TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_docs_matter ON documents(matter_id,position);
            ''')
            con.execute('BEGIN IMMEDIATE')
            if version < 1:
                columns = {row['name'] for row in con.execute('PRAGMA table_info(documents)')}
                if 'revision' not in columns:
                    con.execute('ALTER TABLE documents ADD COLUMN revision INTEGER NOT NULL DEFAULT 1')
            con.execute('''CREATE TABLE IF NOT EXISTS document_revisions (
                document_id TEXT NOT NULL, revision INTEGER NOT NULL,
                working_relpath TEXT NOT NULL, page_count INTEGER NOT NULL,
                ocr_status TEXT NOT NULL, ocr_provenance_json TEXT NOT NULL DEFAULT '[]',
                created_at TEXT NOT NULL,
                PRIMARY KEY(document_id, revision)
            )''')
            con.execute('''INSERT OR IGNORE INTO document_revisions
                (document_id,revision,working_relpath,page_count,ocr_status,created_at)
                SELECT id,revision,working_relpath,page_count,ocr_status,updated_at FROM documents''')
            con.execute('''CREATE TABLE IF NOT EXISTS jobs (
                id TEXT PRIMARY KEY, matter_id TEXT NOT NULL, kind TEXT NOT NULL,
                target_id TEXT, status TEXT NOT NULL, progress INTEGER NOT NULL,
                message TEXT NOT NULL, error TEXT, result_json TEXT,
                retry_job_id TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            )''')
            con.execute('''CREATE TABLE IF NOT EXISTS memo_parties (
                matter_id TEXT PRIMARY KEY REFERENCES matters(id) ON DELETE CASCADE,
                payload_json TEXT NOT NULL, updated_at TEXT NOT NULL
            )''')
            con.execute('''CREATE TABLE IF NOT EXISTS matter_values (
                matter_id TEXT PRIMARY KEY REFERENCES matters(id) ON DELETE CASCADE,
                payload_json TEXT NOT NULL, updated_at TEXT NOT NULL
            )''')
            con.execute('''CREATE TABLE IF NOT EXISTS edit_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                matter_id TEXT NOT NULL REFERENCES matters(id) ON DELETE CASCADE,
                kind TEXT NOT NULL, before_json TEXT NOT NULL, after_json TEXT NOT NULL,
                applied INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL
            )''')
            con.execute('CREATE INDEX IF NOT EXISTS idx_edit_history_matter ON edit_history(matter_id,id)')
            con.execute('''CREATE TABLE IF NOT EXISTS audit_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                matter_id TEXT, document_id TEXT, action TEXT NOT NULL, created_at TEXT NOT NULL
            )''')
            con.execute('CREATE INDEX IF NOT EXISTS idx_audit_matter ON audit_events(matter_id,id)')
            con.execute('''CREATE TABLE IF NOT EXISTS manual_bookmarks (
                id TEXT PRIMARY KEY, matter_id TEXT NOT NULL REFERENCES matters(id) ON DELETE CASCADE,
                title TEXT NOT NULL, document_id TEXT NOT NULL REFERENCES documents(id),
                page_index INTEGER NOT NULL, target_revision INTEGER NOT NULL,
                parent_id TEXT, position INTEGER NOT NULL
            )''')
            con.execute('CREATE INDEX IF NOT EXISTS idx_bookmarks_matter ON manual_bookmarks(matter_id,position)')
            con.execute('''CREATE TABLE IF NOT EXISTS reviewed_links (
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
            )''')
            con.execute('CREATE INDEX IF NOT EXISTS idx_reviewed_links_matter ON reviewed_links(matter_id,position)')
            con.execute('''CREATE TABLE IF NOT EXISTS output_marks (
                matter_id TEXT PRIMARY KEY REFERENCES matters(id) ON DELETE CASCADE,
                payload_json TEXT NOT NULL, updated_at TEXT NOT NULL
            )''')
            con.execute('''CREATE TABLE IF NOT EXISTS output_presets (
                id TEXT PRIMARY KEY, name TEXT NOT NULL COLLATE NOCASE,
                version INTEGER NOT NULL, payload_json TEXT NOT NULL,
                created_at TEXT NOT NULL, UNIQUE(name,version)
            )''')
            con.execute('''CREATE TABLE IF NOT EXISTS matter_output_presets (
                matter_id TEXT PRIMARY KEY REFERENCES matters(id) ON DELETE CASCADE,
                snapshot_json TEXT NOT NULL, updated_at TEXT NOT NULL
            )''')
            con.execute('''CREATE TABLE IF NOT EXISTS page_annotations (
                id TEXT PRIMARY KEY, matter_id TEXT NOT NULL REFERENCES matters(id) ON DELETE CASCADE,
                document_id TEXT NOT NULL REFERENCES documents(id),
                page_index INTEGER NOT NULL, target_revision INTEGER NOT NULL,
                kind TEXT NOT NULL, rect_x REAL NOT NULL, rect_y REAL NOT NULL,
                rect_width REAL NOT NULL, rect_height REAL NOT NULL,
                body TEXT NOT NULL, color TEXT NOT NULL,
                asset_relpath TEXT, asset_mime TEXT, position INTEGER NOT NULL
            )''')
            con.execute('CREATE INDEX IF NOT EXISTS idx_page_annotations_matter ON page_annotations(matter_id,position)')
            if version < 3:
                matter_columns = {row['name'] for row in con.execute('PRAGMA table_info(matters)')}
                document_columns = {row['name'] for row in con.execute('PRAGMA table_info(documents)')}
                if 'archived_at' not in matter_columns:
                    con.execute('ALTER TABLE matters ADD COLUMN archived_at TEXT')
                if 'archived_at' not in document_columns:
                    con.execute('ALTER TABLE documents ADD COLUMN archived_at TEXT')
            if version < 8:
                document_columns = {row['name'] for row in con.execute('PRAGMA table_info(documents)')}
                additions = {
                    'document_date': "TEXT NOT NULL DEFAULT ''",
                    'date_status': "TEXT NOT NULL DEFAULT 'unknown'",
                    'date_source': "TEXT NOT NULL DEFAULT 'none'",
                    'date_note': "TEXT NOT NULL DEFAULT ''",
                    'source_file_mtime': "TEXT NOT NULL DEFAULT ''",
                }
                for column, definition in additions.items():
                    if column not in document_columns:
                        con.execute(f'ALTER TABLE documents ADD COLUMN {column} {definition}')
                con.execute('PRAGMA user_version=8')
            if version < 9:
                document_columns = {row['name'] for row in con.execute('PRAGMA table_info(documents)')}
                if 'label_status' not in document_columns:
                    con.execute("ALTER TABLE documents ADD COLUMN label_status TEXT NOT NULL DEFAULT 'unreviewed'")
                if 'label_source' not in document_columns:
                    con.execute("ALTER TABLE documents ADD COLUMN label_source TEXT NOT NULL DEFAULT 'none'")
                con.execute("""UPDATE documents SET label_source='legacy'
                    WHERE label<>'' AND label_source='none'""")
                con.execute('PRAGMA user_version=9')
            if version < 10:
                document_columns = {row['name'] for row in con.execute('PRAGMA table_info(documents)')}
                if 'group_role' not in document_columns:
                    con.execute("ALTER TABLE documents ADD COLUMN group_role TEXT NOT NULL DEFAULT 'member'")
                con.execute('PRAGMA user_version=10')
            if version < 11:
                document_columns = {row['name'] for row in con.execute('PRAGMA table_info(documents)')}
                additions = {
                    'certificate_signature_status': "TEXT NOT NULL DEFAULT 'unknown'",
                    'certificate_signature_count': 'INTEGER NOT NULL DEFAULT 0',
                    'signature_inspection_note': "TEXT NOT NULL DEFAULT ''",
                    'visible_signature_status': "TEXT NOT NULL DEFAULT 'unreviewed'",
                    'signature_status': "TEXT NOT NULL DEFAULT 'inspection_inconclusive'",
                }
                for column, definition in additions.items():
                    if column not in document_columns:
                        con.execute(f'ALTER TABLE documents ADD COLUMN {column} {definition}')
                con.execute('PRAGMA user_version=11')
            if version < 12:
                con.execute('PRAGMA user_version=12')
            if version < 13:
                con.execute('PRAGMA user_version=13')
            if version < 14:
                document_columns = {row['name'] for row in con.execute('PRAGMA table_info(documents)')}
                revision_columns = {row['name'] for row in con.execute('PRAGMA table_info(document_revisions)')}
                if 'ocr_provenance_json' not in document_columns:
                    con.execute("ALTER TABLE documents ADD COLUMN ocr_provenance_json TEXT NOT NULL DEFAULT '[]'")
                if 'ocr_provenance_json' not in revision_columns:
                    con.execute("ALTER TABLE document_revisions ADD COLUMN ocr_provenance_json TEXT NOT NULL DEFAULT '[]'")
                con.execute('PRAGMA user_version=14')
            if version < 15:
                con.execute('PRAGMA user_version=15')
            con.execute('''CREATE TABLE IF NOT EXISTS reader_positions (
                resource_key TEXT PRIMARY KEY,
                matter_id TEXT NOT NULL REFERENCES matters(id) ON DELETE CASCADE,
                payload_json TEXT NOT NULL, updated_at TEXT NOT NULL
            )''')
            if version < 16:
                con.execute('PRAGMA user_version=16')
            con.execute('''CREATE TABLE IF NOT EXISTS redaction_records (
                id TEXT PRIMARY KEY, document_id TEXT NOT NULL REFERENCES documents(id),
                page_index INTEGER NOT NULL, rect_x REAL NOT NULL, rect_y REAL NOT NULL,
                rect_width REAL NOT NULL, rect_height REAL NOT NULL,
                restore_x REAL NOT NULL, restore_y REAL NOT NULL,
                restore_width REAL NOT NULL, restore_height REAL NOT NULL,
                before_revision INTEGER NOT NULL, after_revision INTEGER NOT NULL,
                active INTEGER NOT NULL DEFAULT 1, removed_revision INTEGER,
                structure_changed INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            )''')
            con.execute('CREATE INDEX IF NOT EXISTS idx_redactions_document ON redaction_records(document_id,page_index)')
            if version < 17:
                con.execute('PRAGMA user_version=17')
            if version < 18:
                columns = {row['name'] for row in con.execute('PRAGMA table_info(page_annotations)')}
                if 'output_scope' not in columns:
                    con.execute("ALTER TABLE page_annotations ADD COLUMN output_scope TEXT NOT NULL DEFAULT 'private'")
                con.execute("UPDATE page_annotations SET output_scope='pdf' WHERE kind='image'")
                con.execute('PRAGMA user_version=18')
            if version < 19:
                # Every previously active document was effectively included.
                if 'bundle_included' not in {row['name'] for row in con.execute('PRAGMA table_info(documents)')}:
                    con.execute('ALTER TABLE documents ADD COLUMN bundle_included INTEGER NOT NULL DEFAULT 1 CHECK(bundle_included IN (0,1))')
                con.execute('PRAGMA user_version=19')
            con.execute('''CREATE TABLE IF NOT EXISTS bundle_membership_state (
                matter_id TEXT PRIMARY KEY REFERENCES matters(id) ON DELETE CASCADE,
                revision INTEGER NOT NULL DEFAULT 1
            )''')

    def list_redactions(self, doc_id: str, active_only: bool = True) -> list[dict]:
        with self.connect() as con:
            rows = con.execute('''SELECT * FROM redaction_records WHERE document_id=?
                AND (?=0 OR active=1) ORDER BY created_at,id''', (doc_id, int(active_only))).fetchall()
        return [dict(row) for row in rows]

    def get_redaction(self, doc_id: str, redaction_id: str) -> dict | None:
        with self.connect() as con:
            row = con.execute('SELECT * FROM redaction_records WHERE id=? AND document_id=?',
                              (redaction_id, doc_id)).fetchone()
        return dict(row) if row else None

    def redaction_source_path(self, doc: dict, revision: int) -> Path:
        with self.connect() as con:
            row = con.execute('''SELECT working_relpath FROM document_revisions
                WHERE document_id=? AND revision=?''', (doc['id'], revision)).fetchone()
        if row is None:
            raise ValueError('The working revision needed to restore this redaction is missing')
        return self.matter_dir(doc['matter_id']) / row['working_relpath']

    def record_redaction(self, doc: dict, page_index: int, region: tuple[float,float,float,float],
                         restore_region: tuple[float,float,float,float], before_revision: int) -> dict:
        item = dict(id=new_id(), document_id=doc['id'], page_index=page_index,
                    rect_x=region[0], rect_y=region[1], rect_width=region[2], rect_height=region[3],
                    restore_x=restore_region[0], restore_y=restore_region[1],
                    restore_width=restore_region[2], restore_height=restore_region[3],
                    before_revision=before_revision, after_revision=doc['revision'],
                    active=1, removed_revision=None, structure_changed=0, created_at=utcnow())
        with self.connect() as con:
            con.execute('INSERT INTO redaction_records (' + ','.join(item) + ') VALUES (' +
                        ','.join('?' for _ in item) + ')', tuple(item.values()))
        return item

    def mark_redaction_removed(self, doc_id: str, redaction_id: str, revision: int) -> None:
        with self.connect() as con:
            con.execute('''UPDATE redaction_records SET active=0,removed_revision=?
                WHERE id=? AND document_id=? AND active=1''', (revision, redaction_id, doc_id))

    def invalidate_redaction_geometry(self, doc_id: str) -> None:
        with self.connect() as con:
            con.execute('''UPDATE redaction_records SET structure_changed=1
                WHERE document_id=? AND active=1''', (doc_id,))

    def reader_position(self, key: str) -> dict | None:
        with self.connect() as con:
            row = con.execute('SELECT payload_json FROM reader_positions WHERE resource_key=?', (key,)).fetchone()
        return json.loads(row['payload_json']) if row else None

    def save_reader_position(self, key: str, matter_id: str, state: dict):
        # Reading does not modify matter/build freshness or create Undo history.
        with self.connect() as con:
            con.execute('''INSERT INTO reader_positions VALUES (?,?,?,?)
                ON CONFLICT(resource_key) DO UPDATE SET payload_json=excluded.payload_json,
                updated_at=excluded.updated_at''', (key, matter_id, json.dumps(state), utcnow()))
            con.execute('''DELETE FROM reader_positions WHERE resource_key NOT IN
                (SELECT resource_key FROM reader_positions ORDER BY updated_at DESC LIMIT 100)''')

    def matter_dir(self, matter_id: str) -> Path:
        # Never accept caller-controlled paths. All ids must have been found in DB.
        return self.root / 'matters' / matter_id

    def recover_temporary_packages(self) -> int:
        """Remove incomplete portable transfer files after an interrupted process."""
        removed = 0
        for path in self.root.glob('.portable-*.cbmatter*'):
            if path.is_file() and not path.is_symlink():
                path.unlink()
                removed += 1
        return removed

    def recover_orphan_working_revisions(self) -> int:
        """Quarantine revision candidates left before their SQLite commit."""
        with self.connect() as con:
            matters = [row[0] for row in con.execute('SELECT id FROM matters')]
            referenced = {row[0] for row in con.execute(
                'SELECT working_relpath FROM documents UNION SELECT working_relpath FROM document_revisions')}
        recovered = 0
        for matter_id in matters:
            working = self.matter_dir(matter_id) / 'working'
            if not working.is_dir() or working.is_symlink():
                continue
            for candidate in working.iterdir():
                if (candidate.is_file() and not candidate.is_symlink() and
                        re.fullmatch(r'\.pdf-edit-[A-Za-z0-9_]+\.pdf', candidate.name)):
                    candidate.unlink()
                    recovered += 1
                    self.audit('interrupted_pdf_edit_temporary_removed', matter_id)
                    continue
                if (not candidate.is_file() or candidate.is_symlink() or
                        not re.fullmatch(r'[0-9a-f]{32}-r[1-9][0-9]*\.pdf', candidate.name) or
                        f'working/{candidate.name}' in referenced):
                    continue
                quarantine = self.matter_dir(matter_id) / 'quarantine' / 'working'
                quarantine.mkdir(parents=True, exist_ok=True)
                destination = quarantine / candidate.name
                if destination.exists():
                    destination = quarantine / f'{candidate.stem}-{new_id()}.pdf'
                candidate.replace(destination)
                recovered += 1
                self.audit('orphan_working_revision_quarantined', matter_id)
        return recovered

    def create_matter(self, fields: dict) -> dict:
        matter_id = new_id()
        now = utcnow()
        item = {'id': matter_id, **fields, 'keep_groups_intact': int(fields['keep_groups_intact']), 'created_at': now, 'updated_at': now}
        with self.connect() as con:
            cols = ','.join(item)
            marks = ','.join('?' for _ in item)
            con.execute(f'INSERT INTO matters ({cols}) VALUES ({marks})', tuple(item.values()))
            self._audit(con, 'matter_created', matter_id)
        self.matter_dir(matter_id).mkdir(parents=True, exist_ok=True)
        return self.get_matter(matter_id)

    def list_matters(self) -> list[dict]:
        with self.connect() as con:
            rows = con.execute('SELECT * FROM matters WHERE archived_at IS NULL ORDER BY updated_at DESC').fetchall()
        return [self._matter(row) for row in rows]

    def list_archived_matters(self) -> list[dict]:
        with self.connect() as con:
            rows = con.execute('SELECT * FROM matters WHERE archived_at IS NOT NULL ORDER BY archived_at DESC').fetchall()
        return [self._matter(row) for row in rows]

    @staticmethod
    def _matter(row) -> dict:
        d = dict(row)
        d['keep_groups_intact'] = bool(d['keep_groups_intact'])
        return d

    def get_matter(self, matter_id: str, include_archived: bool = False) -> dict | None:
        with self.connect() as con:
            row = con.execute('SELECT * FROM matters WHERE id=?' +
                              ('' if include_archived else ' AND archived_at IS NULL'),
                              (matter_id,)).fetchone()
        return self._matter(row) if row else None

    @staticmethod
    def _record_edit(con, matter_id: str, kind: str, before: dict, after: dict):
        if before == after:
            return
        con.execute('DELETE FROM edit_history WHERE matter_id=? AND applied=0', (matter_id,))
        con.execute('''INSERT INTO edit_history
            (matter_id,kind,before_json,after_json,created_at) VALUES (?,?,?,?,?)''',
            (matter_id, kind, json.dumps(before), json.dumps(after), utcnow()))
        con.execute('''DELETE FROM edit_history WHERE matter_id=? AND id NOT IN
            (SELECT id FROM edit_history WHERE matter_id=? ORDER BY id DESC LIMIT ?)''',
            (matter_id, matter_id, Store.HISTORY_LIMIT))

    @staticmethod
    def _audit(con, action: str, matter_id: str | None = None, document_id: str | None = None):
        con.execute('INSERT INTO audit_events (matter_id,document_id,action,created_at) VALUES (?,?,?,?)',
                    (matter_id, document_id, action, utcnow()))

    def audit(self, action: str, matter_id: str | None = None, document_id: str | None = None):
        with self.connect() as con:
            self._audit(con, action, matter_id, document_id)

    def list_audit(self, matter_id: str) -> list[dict]:
        with self.connect() as con:
            rows = con.execute('''SELECT action,document_id,created_at FROM audit_events
                WHERE matter_id=? ORDER BY id DESC LIMIT 500''', (matter_id,)).fetchall()
        return [dict(row) for row in rows]

    def update_matter(self, matter_id: str, changes: dict, record_history: bool = True):
        if not changes:
            return self.get_matter(matter_id)
        changes = dict(changes)
        if 'keep_groups_intact' in changes:
            changes['keep_groups_intact'] = int(changes['keep_groups_intact'])
        changes['updated_at'] = utcnow()
        with self.connect() as con:
            current = con.execute('SELECT * FROM matters WHERE id=?', (matter_id,)).fetchone()
            if current is None:
                raise ValueError('Matter not found')
            tracked = {key: value for key, value in changes.items() if key != 'updated_at'}
            if all(current[key] == value for key, value in tracked.items()):
                return self._matter(current)
            con.execute('UPDATE matters SET ' + ', '.join(f'{k}=?' for k in changes) + ' WHERE id=?', (*changes.values(), matter_id))
            if record_history:
                self._record_edit(con, matter_id, 'matter',
                                  {key: current[key] for key in tracked}, tracked)
            self._audit(con, 'matter_updated', matter_id)
        return self.get_matter(matter_id)

    def delete_matter(self, matter_id: str):
        with self.connect() as con:
            con.execute('UPDATE matters SET archived_at=?,updated_at=? WHERE id=? AND archived_at IS NULL',
                        (utcnow(), utcnow(), matter_id))
            if con.execute('SELECT changes()').fetchone()[0]:
                self._audit(con, 'matter_archived', matter_id)

    def restore_matter(self, matter_id: str) -> dict | None:
        with self.connect() as con:
            con.execute('UPDATE matters SET archived_at=NULL,updated_at=? WHERE id=? AND archived_at IS NOT NULL',
                        (utcnow(), matter_id))
            if con.execute('SELECT changes()').fetchone()[0]:
                self._audit(con, 'matter_restored', matter_id)
        return self.get_matter(matter_id)

    def get_documents(self, matter_id: str) -> list[dict]:
        with self.connect() as con:
            rows = con.execute('''SELECT * FROM documents WHERE matter_id=? AND archived_at IS NULL
                ORDER BY position,created_at''', (matter_id,)).fetchall()
        return [dict(row) for row in rows]

    def get_bundle_documents(self, matter_id: str) -> list[dict]:
        return [doc for doc in self.get_documents(matter_id) if doc['bundle_included']]

    def bundle_membership(self, matter_id: str) -> dict:
        documents = self.get_documents(matter_id)
        with self.connect() as con:
            row = con.execute('SELECT revision FROM bundle_membership_state WHERE matter_id=?',
                              (matter_id,)).fetchone()
        included = [d for d in documents if d['bundle_included']]
        return dict(bundle_id=f'{matter_id}:primary', title='Hearing bundle',
                    revision=row['revision'] if row else 1,
                    input_snapshot=self.build_input_digest(matter_id),
                    documents=[{k: d[k] for k in ('id', 'title', 'part', 'group_key',
                        'page_count', 'position', 'revision', 'bundle_included')} for d in documents],
                    included_documents=len(included),
                    included_source_pages=sum(d['page_count'] for d in included),
                    reference_documents=len(documents)-len(included),
                    reference_source_pages=sum(d['page_count'] for d in documents if not d['bundle_included']))

    def set_bundle_membership(self, matter_id: str, included_ids: list[str], expected_snapshot: str) -> dict:
        # Caller holds the same process lock as imports, page edits and builds.
        if self.build_input_digest(matter_id) != expected_snapshot:
            raise ValueError('Bundle inputs changed. Reload membership before applying your selection.')
        documents = self.get_documents(matter_id)
        ids = {d['id'] for d in documents}
        if len(included_ids) != len(set(included_ids)) or not set(included_ids) <= ids:
            raise ValueError('Membership must contain unique active documents from this matter.')
        before = {d['id']: d['bundle_included'] for d in documents}
        after = {d['id']: int(d['id'] in included_ids) for d in documents}
        if before != after:
            with self.connect() as con:
                con.execute('BEGIN IMMEDIATE')
                for doc_id, included in after.items():
                    con.execute('UPDATE documents SET bundle_included=? WHERE id=? AND matter_id=?',
                                (included, doc_id, matter_id))
                con.execute('INSERT INTO bundle_membership_state(matter_id,revision) VALUES (?,2) '
                            'ON CONFLICT(matter_id) DO UPDATE SET revision=revision+1', (matter_id,))
                con.execute('UPDATE matters SET updated_at=? WHERE id=?', (utcnow(), matter_id))
                self._record_edit(con, matter_id, 'bundle_membership', before, after)
                self._audit(con, 'bundle_membership_applied', matter_id)
        return self.bundle_membership(matter_id)

    def list_manual_bookmarks(self, matter_id: str) -> list[dict]:
        with self.connect() as con:
            rows = con.execute('''SELECT b.*, d.revision AS current_revision,
                d.page_count AS current_page_count, d.archived_at AS document_archived_at
                FROM manual_bookmarks b LEFT JOIN documents d ON d.id=b.document_id
                WHERE b.matter_id=? ORDER BY b.position''', (matter_id,)).fetchall()
        entries = []
        for row in rows:
            entry = {key: row[key] for key in ('id', 'title', 'document_id', 'page_index',
                                               'target_revision', 'parent_id', 'position')}
            entry['resolved'] = (row['current_revision'] == row['target_revision'] and
                                 row['document_archived_at'] is None and
                                 row['current_page_count'] is not None and
                                 row['page_index'] < row['current_page_count'])
            entries.append(entry)
        return entries

    def set_manual_bookmarks(self, matter_id: str, entries: list[dict]) -> list[dict]:
        if len(entries) > 500:
            raise ValueError('At most 500 manual bookmarks are supported')
        ids = [entry['id'] for entry in entries]
        if len(ids) != len(set(ids)):
            raise ValueError('Manual bookmark IDs must be unique')
        positions = {item_id: index for index, item_id in enumerate(ids)}
        with self.connect() as con:
            current = [dict(row) for row in con.execute('''SELECT id,title,document_id,
                page_index,target_revision,parent_id,position FROM manual_bookmarks
                WHERE matter_id=? ORDER BY position''', (matter_id,))]
            docs = {row['id']: row for row in con.execute(
                'SELECT id,revision,page_count,archived_at FROM documents WHERE matter_id=?',
                (matter_id,))}
            old = {entry['id']: entry for entry in current}
            foreign_ids = set(ids) - set(old)
            if foreign_ids and con.execute(
                'SELECT 1 FROM manual_bookmarks WHERE id IN (' +
                ','.join('?' for _ in foreign_ids) + ') LIMIT 1', tuple(foreign_ids)).fetchone():
                raise ValueError('Bookmark ID already belongs to another matter')
            for index, entry in enumerate(entries):
                doc = docs.get(entry['document_id'])
                previous = old.get(entry['id'])
                unchanged_target = previous and all(previous[key] == entry[key] for key in
                                                    ('document_id', 'page_index', 'target_revision'))
                if not doc or (not unchanged_target and
                               (doc['archived_at'] is not None or
                                entry['page_index'] >= doc['page_count'] or
                                entry['target_revision'] != doc['revision'])):
                    raise ValueError('Choose a current page in an active document')
                parent = entry['parent_id']
                if parent is not None and (parent not in positions or positions[parent] >= index):
                    raise ValueError('Bookmark parent must precede its child')
                if not entry['title'].strip():
                    raise ValueError('Bookmark title must contain visible text')
                entry['position'] = index
            # A PDF outline is a depth-first sequence. Canonicalize scattered
            # children so they cannot silently appear under a different parent.
            children = {}
            for entry in entries:
                children.setdefault(entry['parent_id'], []).append(entry)
            ordered = []
            def visit(parent):
                for entry in children.get(parent, []):
                    entry['position'] = len(ordered)
                    ordered.append(entry)
                    visit(entry['id'])
            visit(None)
            normalized = [{key: item[key] for key in ('id', 'title', 'document_id',
                          'page_index', 'target_revision', 'parent_id', 'position')}
                          for item in ordered]
            if current == normalized:
                return self.list_manual_bookmarks(matter_id)
            con.execute('DELETE FROM manual_bookmarks WHERE matter_id=?', (matter_id,))
            self._write_manual_bookmarks(con, matter_id, normalized)
            self._record_edit(con, matter_id, 'bookmarks', {'entries': current},
                              {'entries': normalized})
            self._audit(con, 'manual_bookmarks_updated', matter_id)
        self.touch(matter_id)
        return self.list_manual_bookmarks(matter_id)

    def list_reviewed_links(self, matter_id: str) -> list[dict]:
        with self.connect() as con:
            rows = con.execute('''SELECT l.*,
                source.revision AS source_current_revision,
                source.page_count AS source_page_count,
                source.archived_at AS source_archived_at,
                target.revision AS target_current_revision,
                target.page_count AS target_page_count,
                target.archived_at AS target_archived_at
                FROM reviewed_links l
                LEFT JOIN documents source ON source.id=l.source_document_id
                LEFT JOIN documents target ON target.id=l.target_document_id
                WHERE l.matter_id=? ORDER BY l.position''', (matter_id,)).fetchall()
        entries = []
        fields = ('id', 'title', 'source_document_id', 'source_page_index',
                  'source_revision', 'rect_x', 'rect_y', 'rect_width', 'rect_height',
                  'kind', 'target_document_id', 'target_page_index',
                  'target_revision', 'uri', 'position')
        for row in rows:
            item = {key: row[key] for key in fields}
            source_ok = (row['source_current_revision'] == row['source_revision'] and
                         row['source_archived_at'] is None and
                         row['source_page_count'] is not None and
                         row['source_page_index'] < row['source_page_count'])
            target_ok = item['kind'] == 'external' or (
                row['target_current_revision'] == row['target_revision'] and
                row['target_archived_at'] is None and
                row['target_page_count'] is not None and
                row['target_page_index'] < row['target_page_count'])
            item['resolved'] = bool(source_ok and target_ok)
            entries.append(item)
        return entries

    def set_reviewed_links(self, matter_id: str, entries: list[dict]) -> list[dict]:
        if len(entries) > 500:
            raise ValueError('At most 500 reviewed links are supported')
        ids = [entry['id'] for entry in entries]
        if len(ids) != len(set(ids)):
            raise ValueError('Reviewed link IDs must be unique')
        fields = ('id', 'title', 'source_document_id', 'source_page_index',
                  'source_revision', 'rect_x', 'rect_y', 'rect_width', 'rect_height',
                  'kind', 'target_document_id', 'target_page_index',
                  'target_revision', 'uri', 'position')
        anchor_fields = ('source_document_id', 'source_page_index', 'source_revision',
                         'rect_x', 'rect_y', 'rect_width', 'rect_height', 'kind',
                         'target_document_id', 'target_page_index', 'target_revision', 'uri')
        with self.connect() as con:
            current = [dict(row) for row in con.execute(
                '''SELECT id,title,source_document_id,source_page_index,source_revision,
                rect_x,rect_y,rect_width,rect_height,kind,target_document_id,
                target_page_index,target_revision,uri,position FROM reviewed_links
                WHERE matter_id=? ORDER BY position''', (matter_id,))]
            old = {entry['id']: entry for entry in current}
            foreign_ids = set(ids) - set(old)
            if foreign_ids and con.execute(
                'SELECT 1 FROM reviewed_links WHERE id IN (' +
                ','.join('?' for _ in foreign_ids) + ') LIMIT 1', tuple(foreign_ids)).fetchone():
                raise ValueError('Reviewed link ID already belongs to another matter')
            docs = {row['id']: row for row in con.execute(
                'SELECT id,revision,page_count,archived_at FROM documents WHERE matter_id=?',
                (matter_id,))}
            normalized = []
            for position, entry in enumerate(entries):
                item = dict(entry, position=position)
                previous = old.get(item['id'])
                unchanged_anchor = previous and all(previous[key] == item[key]
                                                    for key in anchor_fields)
                source = docs.get(item['source_document_id'])
                if not unchanged_anchor and (not source or source['archived_at'] is not None or
                        source['revision'] != item['source_revision'] or
                        item['source_page_index'] >= source['page_count']):
                    raise ValueError('Choose a current source region in an active document')
                if item['kind'] == 'internal':
                    target = docs.get(item['target_document_id'])
                    if not unchanged_anchor and (not target or target['archived_at'] is not None or
                            target['revision'] != item['target_revision'] or
                            item['target_page_index'] >= target['page_count']):
                        raise ValueError('Choose a current target page in an active document')
                normalized.append({key: item[key] for key in fields})
            if current == normalized:
                return self.list_reviewed_links(matter_id)
            con.execute('DELETE FROM reviewed_links WHERE matter_id=?', (matter_id,))
            self._write_reviewed_links(con, matter_id, normalized)
            self._record_edit(con, matter_id, 'reviewed_links', {'entries': current},
                              {'entries': normalized})
            self._audit(con, 'reviewed_links_updated', matter_id)
            con.execute('UPDATE matters SET updated_at=? WHERE id=?', (utcnow(), matter_id))
        return self.list_reviewed_links(matter_id)

    @staticmethod
    def _write_reviewed_links(con, matter_id: str, entries: list[dict]):
        for entry in entries:
            con.execute('''INSERT INTO reviewed_links
                (id,matter_id,title,source_document_id,source_page_index,source_revision,
                rect_x,rect_y,rect_width,rect_height,kind,target_document_id,
                target_page_index,target_revision,uri,position)
                VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)''',
                (entry['id'], matter_id, entry['title'], entry['source_document_id'],
                 entry['source_page_index'], entry['source_revision'], entry['rect_x'],
                 entry['rect_y'], entry['rect_width'], entry['rect_height'], entry['kind'],
                 entry['target_document_id'], entry['target_page_index'],
                 entry['target_revision'], entry['uri'], entry['position']))

    @staticmethod
    def default_output_marks() -> dict:
        return {'index_style': 'compact', 'numbering_style': 'arabic',
                'numbering_restart': 'volume', 'numbering_start': 1, 'numbering_prefix': '',
                'numbering_position': 'bottom_center', 'numbering_front_matter': 'physical',
                'numbering_pdf_labels': True, 'numbering_linked_index': True, 'numbering_back_link': False, 'part_extracts_enabled': False,
                'front_matter': 'index_only', 'index_date': False,
                'cover_heading': '', 'cover_reference': '', 'cover_title': '',
                'watermark_enabled': False, 'watermark_text': 'DRAFT',
                'watermark_position': 'center', 'watermark_font_size': 32.0,
                'watermark_opacity': .16, 'timestamp_enabled': False,
                'timestamp_format': 'datetime', 'timestamp_timezone': 'UTC',
                'timestamp_position': 'bottom_right', 'timestamp_font_size': 8.0,
                'timestamp_prefix': 'Built', 'bates_enabled': False,
                'bates_prefix': '', 'bates_start': 1, 'bates_padding': 6,
                'bates_position': 'bottom_left', 'bates_font_size': 8.0,
                'bates_include_index': False, 'bates_include_separator': False,
                'bates_restart_per_volume': False, 'annexure_mode': 'none',
                'annexure_stamp_scope': 'first',
                'annexure_stamp_position': 'top_right',
                'annexure_stamp_font_size': 10.0, 'annexure_overrides': {}}

    def get_output_marks(self, matter_id: str) -> dict:
        with self.connect() as con:
            row = con.execute('SELECT payload_json FROM output_marks WHERE matter_id=?',
                              (matter_id,)).fetchone()
        return {**self.default_output_marks(), **json.loads(row[0])} if row else self.default_output_marks()

    def list_output_presets(self) -> list[dict]:
        with self.connect() as con:
            rows = con.execute('''SELECT * FROM output_presets
                ORDER BY name COLLATE NOCASE,version DESC''').fetchall()
        return [dict(id=row['id'], name=row['name'], version=row['version'],
                     settings=json.loads(row['payload_json']), created_at=row['created_at'])
                for row in rows]

    def create_output_preset(self, settings: dict) -> dict:
        name = settings['name']
        now = utcnow()
        with self.connect() as con:
            con.execute('BEGIN IMMEDIATE')
            version = con.execute('''SELECT COALESCE(MAX(version),0)+1 FROM output_presets
                WHERE name=? COLLATE NOCASE''', (name,)).fetchone()[0]
            preset_id = new_id()
            con.execute('''INSERT INTO output_presets
                (id,name,version,payload_json,created_at) VALUES (?,?,?,?,?)''',
                (preset_id, name, version, json.dumps(settings, sort_keys=True), now))
        return dict(id=preset_id, name=name, version=version, settings=settings,
                    created_at=now)

    def get_applied_output_preset(self, matter_id: str) -> dict | None:
        with self.connect() as con:
            row = con.execute('''SELECT snapshot_json FROM matter_output_presets
                WHERE matter_id=?''', (matter_id,)).fetchone()
        return json.loads(row[0]) if row else None

    def apply_output_preset(self, matter_id: str, preset_id: str, preserve_overrides: bool = False) -> dict:
        with self.connect() as con:
            con.execute('BEGIN IMMEDIATE')
            row = con.execute('SELECT * FROM output_presets WHERE id=?',
                              (preset_id,)).fetchone()
            if row is None:
                raise ValueError('Output preset not found')
            matter = con.execute('SELECT * FROM matters WHERE id=?', (matter_id,)).fetchone()
            if matter is None:
                raise ValueError('Matter not found')
            settings = json.loads(row['payload_json'])
            old_marks = con.execute('SELECT payload_json FROM output_marks WHERE matter_id=?',
                                    (matter_id,)).fetchone()
            old_preset = con.execute('''SELECT snapshot_json FROM matter_output_presets
                WHERE matter_id=?''', (matter_id,)).fetchone()
            current_marks = ({**self.default_output_marks(), **json.loads(old_marks[0])}
                             if old_marks else self.default_output_marks())
            settings['output_marks'] = {**self.default_output_marks(),
                                        **settings['output_marks'],
                                        'annexure_overrides': {}}
            applied_marks = {**settings['output_marks'],
                             'annexure_overrides': current_marks['annexure_overrides']}
            before = {'max_pages_per_volume': matter['max_pages_per_volume'],
                      'keep_groups_intact': bool(matter['keep_groups_intact']),
                      'output_marks': current_marks,
                      'preset': json.loads(old_preset[0]) if old_preset else None}
            snapshot = {'id': row['id'], 'name': row['name'], 'version': row['version'],
                        'settings': settings, 'created_at': row['created_at'], 'provenance': 'Local user preset'}
            after = {'max_pages_per_volume': settings['max_pages_per_volume'],
                     'keep_groups_intact': settings['keep_groups_intact'],
                     'output_marks': applied_marks, 'preset': snapshot}
            # Cover identity and document exceptions always belong to this matter.
            for key in ('cover_heading', 'cover_reference', 'cover_title'):
                applied_marks[key] = current_marks[key]
            if preserve_overrides and before['preset']:
                baseline = before['preset']['settings']
                for key in ('max_pages_per_volume', 'keep_groups_intact'):
                    if before[key] != baseline[key]:
                        after[key] = before[key]
                old_defaults = {**self.default_output_marks(), **baseline['output_marks']}
                for key, value in current_marks.items():
                    if value != old_defaults.get(key):
                        applied_marks[key] = value
            if before == after:
                return snapshot
            now = utcnow()
            con.execute('''UPDATE matters SET max_pages_per_volume=?,keep_groups_intact=?,
                updated_at=? WHERE id=?''', (after['max_pages_per_volume'],
                                             int(after['keep_groups_intact']), now, matter_id))
            con.execute('''INSERT INTO output_marks (matter_id,payload_json,updated_at)
                VALUES (?,?,?) ON CONFLICT(matter_id) DO UPDATE SET
                payload_json=excluded.payload_json,updated_at=excluded.updated_at''',
                (matter_id, json.dumps(after['output_marks'], sort_keys=True), now))
            con.execute('''INSERT INTO matter_output_presets (matter_id,snapshot_json,updated_at)
                VALUES (?,?,?) ON CONFLICT(matter_id) DO UPDATE SET
                snapshot_json=excluded.snapshot_json,updated_at=excluded.updated_at''',
                (matter_id, json.dumps(snapshot, sort_keys=True), now))
            self._record_edit(con, matter_id, 'output_preset', before, after)
            self._audit(con, 'output_preset_applied', matter_id)
        return snapshot

    def save_bundle_settings(self, matter_id: str, settings: dict) -> dict:
        """One transaction/history entry for the complete accepted layout."""
        matter = self.get_matter(matter_id)
        before = {key: matter[key] for key in ('max_pages_per_volume', 'keep_groups_intact')}
        before.update(output_marks=self.get_output_marks(matter_id),
                      preset=self.get_applied_output_preset(matter_id))
        after = {**settings, 'preset': before['preset']}
        with self.connect() as con:
            valid_ids = {r[0] for r in con.execute('SELECT id FROM documents WHERE matter_id=?', (matter_id,))}
        if set(settings['output_marks']['annexure_overrides']) - valid_ids:
            raise ValueError('Annexure exceptions must target documents in this matter')
        if before == after:
            return after
        now = utcnow()
        with self.connect() as con:
            con.execute('BEGIN IMMEDIATE')
            con.execute('UPDATE matters SET max_pages_per_volume=?,keep_groups_intact=?,updated_at=? WHERE id=?',
                        (after['max_pages_per_volume'], int(after['keep_groups_intact']), now, matter_id))
            con.execute('INSERT INTO output_marks (matter_id,payload_json,updated_at) VALUES (?,?,?) '
                        'ON CONFLICT(matter_id) DO UPDATE SET payload_json=excluded.payload_json,updated_at=excluded.updated_at',
                        (matter_id, json.dumps(after['output_marks'], sort_keys=True), now))
            self._record_edit(con, matter_id, 'output_preset', before, after)
            self._audit(con, 'bundle_settings_updated', matter_id)
        return after

    def set_output_marks(self, matter_id: str, settings: dict) -> dict:
        overrides = settings.get('annexure_overrides', {})
        if overrides:
            with self.connect() as con:
                rows = con.execute('SELECT id FROM documents WHERE matter_id=?',
                                   (matter_id,)).fetchall()
            valid = {row['id'] for row in rows}
            if set(overrides) - valid:
                raise ValueError('Annexure exceptions must target documents in this matter')
        before = self.get_output_marks(matter_id)
        if before == settings:
            return settings
        now = utcnow()
        with self.connect() as con:
            con.execute('''INSERT INTO output_marks (matter_id,payload_json,updated_at)
                VALUES (?,?,?) ON CONFLICT(matter_id) DO UPDATE SET
                payload_json=excluded.payload_json,updated_at=excluded.updated_at''',
                (matter_id, json.dumps(settings, sort_keys=True), now))
            self._record_edit(con, matter_id, 'output_marks', {'settings': before},
                              {'settings': settings})
            self._audit(con, 'output_marks_updated', matter_id)
            con.execute('UPDATE matters SET updated_at=? WHERE id=?', (now, matter_id))
        return settings

    @staticmethod
    def _annotation_fields() -> tuple[str, ...]:
        return ('id', 'document_id', 'page_index', 'target_revision', 'kind',
                'rect_x', 'rect_y', 'rect_width', 'rect_height', 'body', 'color',
                'asset_relpath', 'asset_mime', 'position', 'output_scope')

    @classmethod
    def _raw_page_annotations(cls, con, matter_id: str) -> list[dict]:
        fields = cls._annotation_fields()
        rows = con.execute('''SELECT id,document_id,page_index,target_revision,kind,
            rect_x,rect_y,rect_width,rect_height,body,color,asset_relpath,asset_mime,position,output_scope
            FROM page_annotations WHERE matter_id=? ORDER BY position''', (matter_id,)).fetchall()
        return [{key: row[key] for key in fields} for row in rows]

    def list_page_annotations(self, matter_id: str) -> list[dict]:
        with self.connect() as con:
            rows = con.execute('''SELECT a.*, d.revision AS current_revision,
                d.page_count AS current_page_count,d.archived_at AS document_archived_at
                FROM page_annotations a LEFT JOIN documents d ON d.id=a.document_id
                WHERE a.matter_id=? ORDER BY a.position''', (matter_id,)).fetchall()
        entries = []
        fields = self._annotation_fields()
        for row in rows:
            item = {key: row[key] for key in fields}
            item['resolved'] = bool(row['current_revision'] == row['target_revision'] and
                                    row['document_archived_at'] is None and
                                    row['current_page_count'] is not None and
                                    row['page_index'] < row['current_page_count'])
            entries.append(item)
        return entries

    def get_page_annotation(self, annotation_id: str) -> dict | None:
        with self.connect() as con:
            row = con.execute('SELECT * FROM page_annotations WHERE id=?',
                              (annotation_id,)).fetchone()
        return dict(row) if row else None

    @classmethod
    def _write_page_annotations(cls, con, matter_id: str, entries: list[dict]) -> None:
        fields = cls._annotation_fields()
        for entry in entries:
            con.execute('''INSERT INTO page_annotations
                (id,matter_id,document_id,page_index,target_revision,kind,rect_x,rect_y,
                 rect_width,rect_height,body,color,asset_relpath,asset_mime,position,output_scope)
                VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)''',
                (entry['id'], matter_id, *(entry[key] for key in fields[1:])))

    @staticmethod
    def _validate_annotation_document(con, matter_id: str, entry: dict,
                                      *, allow_stale: bool = False) -> None:
        doc = con.execute('''SELECT revision,page_count,archived_at FROM documents
            WHERE id=? AND matter_id=?''', (entry['document_id'], matter_id)).fetchone()
        if (not doc or doc['archived_at'] is not None or
                entry['page_index'] >= doc['page_count'] or
                (not allow_stale and entry['target_revision'] != doc['revision'])):
            raise ValueError('Choose a current page in an active document')

    def create_page_annotation(self, matter_id: str, entry: dict) -> list[dict]:
        with self.connect() as con:
            before = self._raw_page_annotations(con, matter_id)
            self._validate_annotation_document(con, matter_id, entry)
            if con.execute('SELECT 1 FROM page_annotations WHERE id=?',
                           (entry['id'],)).fetchone():
                raise ValueError('Annotation ID already exists')
            entry = dict(entry, position=len(before))
            self._write_page_annotations(con, matter_id, [entry])
            after = before + [entry]
            self._record_edit(con, matter_id, 'page_annotations', {'entries': before},
                              {'entries': after})
            now = utcnow()
            con.execute('UPDATE matters SET updated_at=? WHERE id=?', (now, matter_id))
            self._audit(con, 'page_annotation_created', matter_id, entry['document_id'])
        return self.list_page_annotations(matter_id)

    def update_page_annotation(self, annotation_id: str, changes: dict) -> list[dict]:
        with self.connect() as con:
            current = con.execute('SELECT * FROM page_annotations WHERE id=?',
                                  (annotation_id,)).fetchone()
            if current is None:
                raise ValueError('Annotation not found')
            matter_id = current['matter_id']
            before = self._raw_page_annotations(con, matter_id)
            merged = dict(current)
            merged.update(changes)
            self._validate_annotation_document(con, matter_id, merged)
            if merged['kind'] == 'comment' and not merged['body'].strip():
                raise ValueError('Comments require visible note text')
            tracked = ('rect_x', 'rect_y', 'rect_width', 'rect_height', 'body', 'color', 'output_scope')
            if all(current[key] == merged[key] for key in tracked):
                return self.list_page_annotations(matter_id)
            con.execute('UPDATE page_annotations SET ' + ','.join(f'{key}=?' for key in tracked) +
                        ' WHERE id=?', (*(merged[key] for key in tracked), annotation_id))
            after = self._raw_page_annotations(con, matter_id)
            self._record_edit(con, matter_id, 'page_annotations', {'entries': before},
                              {'entries': after})
            con.execute('UPDATE matters SET updated_at=? WHERE id=?', (utcnow(), matter_id))
            self._audit(con, 'page_annotation_updated', matter_id, current['document_id'])
        return self.list_page_annotations(matter_id)

    def delete_page_annotation(self, annotation_id: str) -> tuple[str, list[dict]]:
        with self.connect() as con:
            current = con.execute('SELECT * FROM page_annotations WHERE id=?',
                                  (annotation_id,)).fetchone()
            if current is None:
                raise ValueError('Annotation not found')
            matter_id = current['matter_id']
            before = self._raw_page_annotations(con, matter_id)
            con.execute('DELETE FROM page_annotations WHERE id=?', (annotation_id,))
            after = self._raw_page_annotations(con, matter_id)
            for position, entry in enumerate(after):
                if entry['position'] != position:
                    con.execute('UPDATE page_annotations SET position=? WHERE id=?',
                                (position, entry['id']))
                    entry['position'] = position
            self._record_edit(con, matter_id, 'page_annotations', {'entries': before},
                              {'entries': after})
            con.execute('UPDATE matters SET updated_at=? WHERE id=?', (utcnow(), matter_id))
            self._audit(con, 'page_annotation_deleted', matter_id, current['document_id'])
        return matter_id, self.list_page_annotations(matter_id)

    @staticmethod
    def _write_manual_bookmarks(con, matter_id: str, entries: list[dict]):
        for entry in entries:
            con.execute('''INSERT INTO manual_bookmarks
                (id,matter_id,title,document_id,page_index,target_revision,parent_id,position)
                VALUES (?,?,?,?,?,?,?,?)''',
                (entry['id'], matter_id, entry['title'], entry['document_id'],
                 entry['page_index'], entry['target_revision'], entry['parent_id'],
                 entry['position']))

    def list_archived_documents(self, matter_id: str) -> list[dict]:
        with self.connect() as con:
            rows = con.execute('''SELECT * FROM documents WHERE matter_id=? AND archived_at IS NOT NULL
                ORDER BY archived_at DESC''', (matter_id,)).fetchall()
        return [dict(row) for row in rows]

    def storage_usage(self, matter_id: str) -> dict:
        folder = self.matter_dir(matter_id)
        totals = {'originals': 0, 'working': 0, 'exports': 0, 'other': 0}
        if folder.exists():
            for path in folder.rglob('*'):
                if path.is_file() and not path.is_symlink():
                    category = path.relative_to(folder).parts[0]
                    totals[category if category in totals else 'other'] += path.stat().st_size
        return {'bytes': sum(totals.values()), 'by_category': totals}

    def get_memo_parties(self, matter_id: str) -> list[dict]:
        with self.connect() as con:
            row = con.execute('SELECT payload_json FROM memo_parties WHERE matter_id=?',
                              (matter_id,)).fetchone()
        return json.loads(row[0]) if row else []

    def set_memo_parties(self, matter_id: str, parties: list[dict]) -> list[dict]:
        with self.connect() as con:
            previous = con.execute('SELECT payload_json FROM memo_parties WHERE matter_id=?',
                                   (matter_id,)).fetchone()
            before = json.loads(previous[0]) if previous else []
            if before == parties:
                return parties
            con.execute('''INSERT INTO memo_parties (matter_id,payload_json,updated_at)
                VALUES (?,?,?) ON CONFLICT(matter_id) DO UPDATE SET
                payload_json=excluded.payload_json,updated_at=excluded.updated_at''',
                (matter_id, json.dumps(parties, ensure_ascii=False), utcnow()))
            self._record_edit(con, matter_id, 'memo_parties', {'parties': before}, {'parties': parties})
            con.execute('UPDATE matters SET updated_at=? WHERE id=?', (utcnow(), matter_id))
            self._audit(con, 'memo_parties_updated', matter_id)
        return parties

    def get_matter_values(self, matter_id: str) -> dict[str, str]:
        with self.connect() as con:
            row = con.execute('SELECT payload_json FROM matter_values WHERE matter_id=?',
                              (matter_id,)).fetchone()
        return json.loads(row[0]) if row else {}

    def set_matter_values(self, matter_id: str, values: dict[str, str]) -> dict[str, str]:
        with self.connect() as con:
            previous = con.execute('SELECT payload_json FROM matter_values WHERE matter_id=?',
                                   (matter_id,)).fetchone()
            before = json.loads(previous[0]) if previous else {}
            if before == values:
                return values
            con.execute('''INSERT INTO matter_values (matter_id,payload_json,updated_at)
                VALUES (?,?,?) ON CONFLICT(matter_id) DO UPDATE SET
                payload_json=excluded.payload_json,updated_at=excluded.updated_at''',
                (matter_id, json.dumps(values, ensure_ascii=False), utcnow()))
            self._record_edit(con, matter_id, 'matter_values', {'values': before}, {'values': values})
            con.execute('UPDATE matters SET updated_at=? WHERE id=?', (utcnow(), matter_id))
            self._audit(con, 'matter_values_updated', matter_id)
        return values

    def get_filing_review(self, matter_id: str) -> dict:
        from .schemas import FilingReview
        values = self.get_matter_values(matter_id)
        if 'filing_review_meta' not in values and 'filing_review_items' not in values:
            return FilingReview().model_dump()
        try:
            meta = json.loads(values.get('filing_review_meta', '{}'))
            items = json.loads(values.get('filing_review_items', '[]'))
            return FilingReview.model_validate({**meta, 'items': items}).model_dump()
        except (ValueError, TypeError, KeyError) as exc:
            raise ValueError('Stored filing review is invalid; restore or correct it before building') from exc

    def set_filing_review(self, matter_id: str, review: dict) -> dict:
        from .schemas import MatterValuesUpdate
        values = self.get_matter_values(matter_id)
        values['filing_review_meta'] = json.dumps({key: review[key] for key in
            ('court', 'case_type', 'stage', 'source_note')}, ensure_ascii=False)
        values['filing_review_items'] = json.dumps(review['items'], ensure_ascii=False)
        MatterValuesUpdate.model_validate({'values': values})
        self.set_matter_values(matter_id, values)
        return self.get_filing_review(matter_id)

    def get_document(self, doc_id: str) -> dict | None:
        with self.connect() as con:
            row = con.execute('SELECT * FROM documents WHERE id=? AND archived_at IS NULL', (doc_id,)).fetchone()
        return dict(row) if row else None

    @staticmethod
    def _validate_group_parent(con, matter_id: str, doc_id: str,
                               group_key: str, group_role: str) -> None:
        group_key = group_key.strip()
        if group_role != 'parent':
            return
        if not group_key:
            raise ValueError('A group parent must have a group name')
        if con.execute('''SELECT 1 FROM documents
            WHERE matter_id=? AND archived_at IS NULL AND id<>? AND group_key=?
            AND group_role='parent' LIMIT 1''',
            (matter_id, doc_id, group_key)).fetchone():
            raise ValueError(f'Group “{group_key}” already has a designated parent')

    def add_document(self, data: dict) -> dict:
        with self.connect() as con:
            pos = con.execute('SELECT COALESCE(MAX(position),-1)+1 FROM documents WHERE matter_id=?', (data['matter_id'],)).fetchone()[0]
            item = dict(data, position=pos, created_at=utcnow(), updated_at=utcnow())
            con.execute('INSERT INTO documents (' + ','.join(item) + ') VALUES (' + ','.join('?' for _ in item) + ')', tuple(item.values()))
            con.execute('''INSERT INTO document_revisions
                (document_id,revision,working_relpath,page_count,ocr_status,ocr_provenance_json,created_at)
                VALUES (?,?,?,?,?,?,?)''',
                (item['id'], 1, item['working_relpath'], item['page_count'], item['ocr_status'],
                 item.get('ocr_provenance_json', '[]'), item['updated_at']))
            self._record_edit(con, data['matter_id'], 'document_presence',
                              {'id': data['id'], 'archived_at': item['created_at'], 'position': pos},
                              {'id': data['id'], 'archived_at': None, 'position': pos})
            self._audit(con, 'document_imported', data['matter_id'], data['id'])
        self.touch(data['matter_id'])
        return self.get_document(data['id'])

    def update_document(self, doc_id: str, changes: dict, record_history: bool = True,
                        preserve_page_annotations: bool = False,
                        preserve_pdf_marks: bool = False):
        if not changes:
            return self.get_document(doc_id)
        changes = dict(changes)
        changes['updated_at'] = utcnow()
        with self.connect() as con:
            current = con.execute('SELECT * FROM documents WHERE id=?', (doc_id,)).fetchone()
            if current is None:
                raise ValueError('Document not found')
            matter_id = current['matter_id']
            final_group = changes.get('group_key', current['group_key']).strip()
            final_role = changes.get('group_role', current['group_role'])
            self._validate_group_parent(con, matter_id, doc_id, final_group, final_role)
            tracked = {key: value for key, value in changes.items() if key != 'updated_at'}
            if all(current[key] == value for key, value in tracked.items()):
                return dict(current)
            if 'working_relpath' in changes:
                changes['revision'] = current['revision'] + 1
            annotation_ids = []
            if (preserve_page_annotations or preserve_pdf_marks) and 'working_relpath' in changes:
                annotation_ids = [row['id'] for row in con.execute('''
                    SELECT id FROM page_annotations WHERE document_id=? AND target_revision=?
                    AND (?=1 OR output_scope='pdf')''',
                    (doc_id, current['revision'], int(preserve_page_annotations)))]
            con.execute('UPDATE documents SET ' + ','.join(f'{k}=?' for k in changes) + ' WHERE id=?', (*changes.values(), doc_id))
            if annotation_ids:
                con.executemany('UPDATE page_annotations SET target_revision=? WHERE id=?',
                                [(changes['revision'], item_id) for item_id in annotation_ids])
            if 'working_relpath' in changes:
                con.execute('''INSERT INTO document_revisions
                    (document_id,revision,working_relpath,page_count,ocr_status,ocr_provenance_json,created_at)
                    VALUES (?,?,?,?,?,?,?)''', (doc_id, changes['revision'], changes['working_relpath'],
                                            changes.get('page_count', current['page_count']),
                                            changes.get('ocr_status', current['ocr_status']),
                                            changes.get('ocr_provenance_json', current['ocr_provenance_json']),
                                            changes['updated_at']))
            if record_history:
                tracked = {key: value for key, value in changes.items()
                           if key not in ('updated_at', 'revision')}
                self._record_edit(con, matter_id, 'document',
                                  {'id': doc_id, 'fields': {key: current[key] for key in tracked},
                                   'preserved_annotation_ids': annotation_ids},
                                  {'id': doc_id, 'fields': tracked,
                                   'preserved_annotation_ids': annotation_ids})
            self._audit(con, 'document_updated', matter_id, doc_id)
            con.execute('UPDATE matters SET updated_at=? WHERE id=?', (changes['updated_at'], matter_id))
            result = dict(con.execute('SELECT * FROM documents WHERE id=?', (doc_id,)).fetchone())
        return result

    def batch_update_documents(self, matter_id: str, ids: list[str], changes: dict):
        """Apply one metadata edit to an explicit selection as one undoable transaction."""
        if not changes:
            raise ValueError('Choose at least one batch field')
        with self.connect() as con:
            rows = [dict(row) for row in con.execute(
                '''SELECT * FROM documents WHERE matter_id=? AND archived_at IS NULL
                ORDER BY position,created_at''', (matter_id,))]
            by_id = {row['id']: row for row in rows}
            if len(ids) != len(set(ids)) or any(doc_id not in by_id for doc_id in ids):
                raise ValueError('Selection contains a missing, archived or duplicate document')
            affected = [by_id[doc_id] for doc_id in ids]
            if 'group_key' in changes:
                final_groups = {row['id']: (changes['group_key'] if row['id'] in ids
                                             else row['group_key']).strip() for row in rows}
                for row in rows:
                    if row['group_role'] == 'parent' and not final_groups[row['id']]:
                        raise ValueError('Clear or change the designated parent role before removing its group')
                parents: dict[str, str] = {}
                for row in rows:
                    if row['group_role'] != 'parent':
                        continue
                    key = final_groups[row['id']]
                    if key in parents:
                        raise ValueError(f'Group “{key}” would have more than one designated parent')
                    parents[key] = row['id']
            changed = [row for row in affected if any(row[key] != value for key, value in changes.items())]
            if not changed:
                return self.get_documents(matter_id)
            now = utcnow()
            before = [{'id': row['id'], 'fields': {key: row[key] for key in changes}} for row in changed]
            after = [{'id': row['id'], 'fields': dict(changes)} for row in changed]
            assignments = ','.join(f'{key}=?' for key in changes)
            for row in changed:
                con.execute(f'UPDATE documents SET {assignments},updated_at=? WHERE id=?',
                            (*changes.values(), now, row['id']))
            self._record_edit(con, matter_id, 'document_batch', {'items': before}, {'items': after})
            self._audit(con, 'documents_batch_updated', matter_id)
            con.execute('UPDATE matters SET updated_at=? WHERE id=?', (now, matter_id))
        return self.get_documents(matter_id)

    def apply_label_mapping(self, matter_id: str, mappings: list[dict]) -> list[dict]:
        """Apply an explicitly reviewed old-to-new label map as one undoable edit."""
        ids = [item['document_id'] for item in mappings]
        if len(ids) != len(set(ids)):
            raise ValueError('Map each document once')
        with self.connect() as con:
            rows = [dict(row) for row in con.execute(
                '''SELECT * FROM documents WHERE matter_id=? AND archived_at IS NULL
                ORDER BY position,created_at''', (matter_id,))]
            by_id = {row['id']: row for row in rows}
            if any(doc_id not in by_id for doc_id in ids):
                raise ValueError('A mapped document is missing or archived; preview again')

            normalized = []
            for item in mappings:
                row = by_id[item['document_id']]
                if row['label'] != item['old_label']:
                    raise ValueError(f'Label for “{row["title"]}” changed after preview; preview again')
                new_label = item['new_label'].strip()
                if not new_label or len(new_label) > 60:
                    raise ValueError('Every proposed label must contain 1 to 60 visible characters')
                normalized.append((row, new_label))

            final_labels = {row['id']: row['label'].strip() for row in rows}
            for row, new_label in normalized:
                final_labels[row['id']] = new_label
            seen: dict[str, str] = {}
            for row in rows:
                label = final_labels[row['id']]
                if not label:
                    continue
                folded = label.casefold()
                if folded in seen:
                    raise ValueError(f'Label “{label}” would be duplicated')
                seen[folded] = row['id']

            changed = [(row, label) for row, label in normalized
                       if row['label'] != label or row['label_status'] != 'confirmed'
                       or row['label_source'] != 'proposal']
            if not changed:
                return rows
            now = utcnow()
            before = [{'id': row['id'], 'fields': {
                'label': row['label'], 'label_status': row['label_status'],
                'label_source': row['label_source']}} for row, _label in changed]
            after = [{'id': row['id'], 'fields': {
                'label': label, 'label_status': 'confirmed',
                'label_source': 'proposal'}} for row, label in changed]
            for row, label in changed:
                con.execute('''UPDATE documents SET label=?,label_status='confirmed',
                    label_source='proposal',updated_at=? WHERE id=?''',
                    (label, now, row['id']))
            self._record_edit(con, matter_id, 'document_batch',
                              {'items': before}, {'items': after})
            self._audit(con, 'document_labels_applied', matter_id)
            con.execute('UPDATE matters SET updated_at=? WHERE id=?', (now, matter_id))
        return self.get_documents(matter_id)

    def label_export_history(self, matter_id: str, selected_ids: list[str]) -> dict:
        """Report prior generated bundles that recorded the selected documents' labels."""
        from .file_io import read_bytes
        selected = set(selected_ids)
        prior_uses = []
        unknown_exports = []
        root = self.exports_root(matter_id)
        if not root.exists():
            return {'has_prior_exports': False, 'prior_uses': [], 'unknown_exports': []}
        for path in root.glob('*/export_manifest.json'):
            if path.parent.name.startswith('.'):
                continue
            try:
                manifest = json.loads(read_bytes(path))
            except (OSError, ValueError, TypeError):
                continue
            if 'label_inputs' not in manifest:
                unknown_exports.append({'id': manifest.get('id', path.parent.name),
                                        'created_at': manifest.get('created_at')})
                continue
            uses = [item for item in manifest['label_inputs']
                    if item.get('document_id') in selected]
            if uses:
                prior_uses.append({'id': manifest.get('id', path.parent.name),
                                   'created_at': manifest.get('created_at'),
                                   'labels': uses})
        prior_uses.sort(key=lambda item: item.get('created_at') or '', reverse=True)
        unknown_exports.sort(key=lambda item: item.get('created_at') or '', reverse=True)
        return {'has_prior_exports': bool(prior_uses or unknown_exports),
                'prior_uses': prior_uses, 'unknown_exports': unknown_exports}

    def preview_document_sort(self, matter_id: str, mode: str) -> dict:
        docs = self.get_documents(matter_id)
        matter = self.get_matter(matter_id)
        if matter is None:
            raise ValueError('Matter not found')
        if mode not in ('manual', 'upload', 'filename', 'document_date'):
            raise ValueError('Unknown document sort mode')

        def document_key(doc: dict):
            if mode == 'upload':
                return (doc['created_at'], doc['position'], doc['id'])
            if mode == 'filename':
                return (doc['original_name'].casefold(), doc['position'], doc['id'])
            if mode == 'document_date':
                # Exact suggestions may participate in the preview, but their review
                # state is preserved. Missing/ambiguous dates remain stable at the end.
                return (0, doc['document_date'], doc['position'], doc['id']) if doc['document_date'] else \
                       (1, '', doc['position'], doc['id'])
            return (doc['position'], doc['id'])

        if mode == 'manual':
            ordered = list(docs)
        elif matter['keep_groups_intact']:
            units: list[list[dict]] = []
            group_units: dict[str, list[dict]] = {}
            for doc in docs:
                key = doc['group_key'].strip()
                if not key:
                    units.append([doc])
                elif key not in group_units:
                    group_units[key] = [doc]
                    units.append(group_units[key])
                else:
                    group_units[key].append(doc)
            units.sort(key=lambda unit: document_key(unit[0]))
            ordered = [doc for unit in units for doc in unit]
        else:
            ordered = sorted(docs, key=document_key)

        old_positions = {doc['id']: index for index, doc in enumerate(docs)}
        label_counts: dict[str, list[str]] = {}
        label_numbers: dict[str, set[int]] = {}
        for doc in docs:
            label = doc['label'].strip()
            if not label:
                continue
            label_counts.setdefault(label.casefold(), []).append(label)
            match = re.fullmatch(r'(.+?)[- ]?(\d+)', label)
            if match:
                label_numbers.setdefault(match.group(1).upper(), set()).add(int(match.group(2)))
        duplicate_labels = sorted({values[0] for values in label_counts.values() if len(values) > 1},
                                  key=str.casefold)
        label_gaps = []
        for prefix, values in sorted(label_numbers.items()):
            if len(values) > 1:
                label_gaps.extend(f'{prefix}-{number}' for number in
                                  range(min(values), max(values)) if number not in values)
        rows = [{key: doc[key] for key in ('id', 'title', 'original_name', 'document_date',
                                           'date_status', 'date_source', 'date_note', 'group_key', 'label')}
                | {'old_position': old_positions[doc['id']], 'new_position': index}
                for index, doc in enumerate(ordered)]
        return {
            'mode': mode,
            'ids': [doc['id'] for doc in ordered],
            'rows': rows,
            'unknown_dates': 'last',
            'stable_ties': 'current manual order',
            'group_policy': ('keep grouped documents together in their current internal order'
                             if matter['keep_groups_intact'] else 'sort documents individually'),
            'label_review': {'duplicates': duplicate_labels, 'gaps': label_gaps[:100]},
        }

    def delete_document(self, doc_id: str):
        with self.connect() as con:
            row = con.execute('SELECT matter_id,position FROM documents WHERE id=? AND archived_at IS NULL', (doc_id,)).fetchone()
            if row:
                archived_at = utcnow()
                con.execute('UPDATE documents SET archived_at=?,updated_at=? WHERE id=?',
                            (archived_at, archived_at, doc_id))
                self._record_edit(con, row['matter_id'], 'document_presence',
                                  {'id': doc_id, 'archived_at': None, 'position': row['position']},
                                  {'id': doc_id, 'archived_at': archived_at, 'position': row['position']})
                self._audit(con, 'document_archived', row['matter_id'], doc_id)
        if row:
            self.touch(row['matter_id'])

    def restore_document(self, matter_id: str, doc_id: str) -> dict | None:
        with self.connect() as con:
            previous = con.execute('''SELECT archived_at,position,group_key,group_role FROM documents
                WHERE id=? AND matter_id=? AND archived_at IS NOT NULL''', (doc_id, matter_id)).fetchone()
            if previous is None:
                return None
            self._validate_group_parent(con, matter_id, doc_id,
                                        previous['group_key'], previous['group_role'])
            position = con.execute('''SELECT COALESCE(MAX(position),-1)+1 FROM documents
                WHERE matter_id=? AND archived_at IS NULL''', (matter_id,)).fetchone()[0]
            con.execute('''UPDATE documents SET archived_at=NULL,position=?,updated_at=?
                WHERE id=? AND matter_id=? AND archived_at IS NOT NULL''',
                (position, utcnow(), doc_id, matter_id))
            changed = con.execute('SELECT changes()').fetchone()[0]
            if changed:
                self._record_edit(con, matter_id, 'document_presence',
                                  {'id': doc_id, 'archived_at': previous['archived_at'], 'position': previous['position']},
                                  {'id': doc_id, 'archived_at': None, 'position': position})
                self._audit(con, 'document_restored', matter_id, doc_id)
        if changed:
            self.touch(matter_id)
        return self.get_document(doc_id) if changed else None

    def reorder(self, matter_id: str, ids: list[str], record_history: bool = True):
        with self.connect() as con:
            before = [r[0] for r in con.execute(
                '''SELECT id FROM documents WHERE matter_id=? AND archived_at IS NULL
                ORDER BY position,created_at''', (matter_id,))]
            if len(ids) != len(before) or set(ids) != set(before):
                raise ValueError('Order must contain every document exactly once')
            if before == ids:
                return self.get_documents(matter_id)
            for pos, doc_id in enumerate(ids):
                con.execute('UPDATE documents SET position=?, updated_at=? WHERE id=?', (pos, utcnow(), doc_id))
            if record_history:
                self._record_edit(con, matter_id, 'order', {'ids': before}, {'ids': ids})
            self._audit(con, 'documents_reordered', matter_id)
        self.touch(matter_id)
        return self.get_documents(matter_id)

    def history_state(self, matter_id: str) -> dict:
        def label(row):
            if row is None:
                return None
            if row['kind'] == 'document':
                fields = json.loads(row['after_json']).get('fields', {})
                return 'Working PDF edit' if 'working_relpath' in fields else 'Document details'
            if row['kind'] == 'document_presence':
                after = json.loads(row['after_json'])
                return 'Archive document' if after['archived_at'] else 'Add or restore document'
            return {
                'document_batch': 'Batch document details',
                'matter': 'Matter settings',
                'bundle_membership': 'Bundle membership',
                'order': 'Document order',
                'bookmarks': 'Bookmarks',
                'reviewed_links': 'Reviewed links',
                'output_marks': 'Output marks',
                'output_preset': 'Output preset',
                'page_annotations': 'Private review notes or overlays',
                'memo_parties': 'Memo of Parties',
                'matter_values': 'Matter values',
            }.get(row['kind'], 'Matter edit')

        with self.connect() as con:
            undo = con.execute('SELECT COUNT(*) FROM edit_history WHERE matter_id=? AND applied=1',
                               (matter_id,)).fetchone()[0]
            redo = con.execute('SELECT COUNT(*) FROM edit_history WHERE matter_id=? AND applied=0',
                               (matter_id,)).fetchone()[0]
            # Match traverse_history exactly: newest applied edit for Undo,
            # oldest unapplied edit for Redo. Labels do not create another history.
            undo_row = con.execute('''SELECT kind,after_json FROM edit_history
                WHERE matter_id=? AND applied=1 ORDER BY id DESC LIMIT 1''', (matter_id,)).fetchone()
            redo_row = con.execute('''SELECT kind,after_json FROM edit_history
                WHERE matter_id=? AND applied=0 ORDER BY id ASC LIMIT 1''', (matter_id,)).fetchone()
        return {'can_undo': bool(undo), 'can_redo': bool(redo), 'undo_count': undo, 'redo_count': redo,
                'undo_label': label(undo_row), 'redo_label': label(redo_row)}

    def traverse_history(self, matter_id: str, redo: bool = False) -> dict:
        with self.connect() as con:
            row = con.execute('''SELECT * FROM edit_history WHERE matter_id=? AND applied=?
                ORDER BY id ''' + ('ASC' if redo else 'DESC') + ' LIMIT 1',
                (matter_id, 0 if redo else 1)).fetchone()
            if row is None:
                raise ValueError('No edit to ' + ('redo' if redo else 'undo'))
            state = json.loads(row['after_json'] if redo else row['before_json'])
            now = utcnow()
            if row['kind'] == 'document_presence':
                doc = con.execute('SELECT * FROM documents WHERE id=? AND matter_id=?',
                                  (state['id'], matter_id)).fetchone()
                if doc is None:
                    raise ValueError('Document is missing; this edit cannot be restored')
                if state['archived_at'] is None:
                    folder = self.matter_dir(matter_id)
                    if not (folder / doc['source_relpath']).is_file() or not (folder / doc['working_relpath']).is_file():
                        raise ValueError('Document files are missing; this edit cannot be restored')
                    self._validate_group_parent(con, matter_id, state['id'],
                                                doc['group_key'], doc['group_role'])
                con.execute('''UPDATE documents SET archived_at=?,position=?,updated_at=? WHERE id=?''',
                            (state['archived_at'], state['position'], now, state['id']))
            elif row['kind'] == 'bundle_membership':
                for doc_id, included in state.items():
                    con.execute('UPDATE documents SET bundle_included=? WHERE id=? AND matter_id=?',
                                (included, doc_id, matter_id))
                con.execute('INSERT INTO bundle_membership_state(matter_id,revision) VALUES (?,2) '
                            'ON CONFLICT(matter_id) DO UPDATE SET revision=revision+1', (matter_id,))
            elif row['kind'] == 'document':
                doc = con.execute('''SELECT * FROM documents WHERE id=? AND matter_id=?
                    AND archived_at IS NULL''', (state['id'], matter_id)).fetchone()
                if doc is None:
                    raise ValueError('Document was removed; this edit cannot be restored')
                fields = state['fields']
                self._validate_group_parent(
                    con, matter_id, state['id'],
                    fields.get('group_key', doc['group_key']),
                    fields.get('group_role', doc['group_role']))
                if 'working_relpath' in fields:
                    if not (self.matter_dir(matter_id) / fields['working_relpath']).is_file():
                        raise ValueError('Working copy is missing; this edit cannot be restored')
                    source_revision = con.execute('''SELECT MIN(revision) FROM document_revisions
                        WHERE document_id=? AND working_relpath=?''',
                        (state['id'], fields['working_relpath'])).fetchone()[0]
                    revision = doc['revision'] + 1
                    con.execute('''INSERT INTO document_revisions
                        (document_id,revision,working_relpath,page_count,ocr_status,ocr_provenance_json,created_at)
                        VALUES (?,?,?,?,?,?,?)''',
                        (state['id'], revision, fields['working_relpath'],
                         fields.get('page_count', doc['page_count']),
                         fields.get('ocr_status', doc['ocr_status']),
                         fields.get('ocr_provenance_json', doc['ocr_provenance_json']), now))
                    fields = dict(fields, revision=revision)
                    if source_revision is not None:
                        con.execute('''UPDATE redaction_records SET active=CASE
                            WHEN before_revision=? THEN 0
                            WHEN after_revision=? THEN 1
                            WHEN removed_revision-1=? THEN 1
                            WHEN removed_revision=? THEN 0
                            ELSE active END WHERE document_id=?''',
                            (source_revision, source_revision, source_revision,
                             source_revision, state['id']))
                changes = dict(fields, updated_at=now)
                con.execute('UPDATE documents SET ' + ','.join(f'{key}=?' for key in changes) +
                            ' WHERE id=?', (*changes.values(), state['id']))
                for annotation_id in state.get('preserved_annotation_ids', []):
                    con.execute('UPDATE page_annotations SET target_revision=? WHERE id=? AND document_id=?',
                                (revision, annotation_id, state['id']))
            elif row['kind'] == 'document_batch':
                for item in state['items']:
                    doc = con.execute('''SELECT id FROM documents WHERE id=? AND matter_id=?
                        AND archived_at IS NULL''', (item['id'], matter_id)).fetchone()
                    if doc is None:
                        raise ValueError('A selected document was removed; this batch edit cannot be restored')
                    fields = item['fields']
                    changes = dict(fields, updated_at=now)
                    con.execute('UPDATE documents SET ' + ','.join(f'{key}=?' for key in changes) +
                                ' WHERE id=?', (*changes.values(), item['id']))
            elif row['kind'] == 'matter':
                changes = dict(state, updated_at=now)
                con.execute('UPDATE matters SET ' + ','.join(f'{key}=?' for key in changes) +
                            ' WHERE id=?', (*changes.values(), matter_id))
            elif row['kind'] == 'order':
                actual = [r[0] for r in con.execute('''SELECT id FROM documents WHERE matter_id=?
                    AND archived_at IS NULL ORDER BY position,created_at''', (matter_id,))]
                if len(actual) != len(state['ids']) or set(actual) != set(state['ids']):
                    raise ValueError('Document set changed; order cannot be restored')
                for position, doc_id in enumerate(state['ids']):
                    con.execute('UPDATE documents SET position=?,updated_at=? WHERE id=?',
                                (position, now, doc_id))
            elif row['kind'] == 'bookmarks':
                con.execute('DELETE FROM manual_bookmarks WHERE matter_id=?', (matter_id,))
                self._write_manual_bookmarks(con, matter_id, state['entries'])
            elif row['kind'] == 'reviewed_links':
                con.execute('DELETE FROM reviewed_links WHERE matter_id=?', (matter_id,))
                self._write_reviewed_links(con, matter_id, state['entries'])
            elif row['kind'] == 'output_marks':
                con.execute('''INSERT INTO output_marks (matter_id,payload_json,updated_at)
                    VALUES (?,?,?) ON CONFLICT(matter_id) DO UPDATE SET
                    payload_json=excluded.payload_json,updated_at=excluded.updated_at''',
                    (matter_id, json.dumps(state['settings'], sort_keys=True), now))
            elif row['kind'] == 'output_preset':
                con.execute('''UPDATE matters SET max_pages_per_volume=?,keep_groups_intact=?,
                    updated_at=? WHERE id=?''', (state['max_pages_per_volume'],
                                                 int(state['keep_groups_intact']), now, matter_id))
                con.execute('''INSERT INTO output_marks (matter_id,payload_json,updated_at)
                    VALUES (?,?,?) ON CONFLICT(matter_id) DO UPDATE SET
                    payload_json=excluded.payload_json,updated_at=excluded.updated_at''',
                    (matter_id, json.dumps(state['output_marks'], sort_keys=True), now))
                if state['preset'] is None:
                    con.execute('DELETE FROM matter_output_presets WHERE matter_id=?', (matter_id,))
                else:
                    con.execute('''INSERT INTO matter_output_presets
                        (matter_id,snapshot_json,updated_at) VALUES (?,?,?)
                        ON CONFLICT(matter_id) DO UPDATE SET
                        snapshot_json=excluded.snapshot_json,updated_at=excluded.updated_at''',
                        (matter_id, json.dumps(state['preset'], sort_keys=True), now))
            elif row['kind'] == 'page_annotations':
                for entry in state['entries']:
                    self._validate_annotation_document(con, matter_id, entry, allow_stale=True)
                    if entry['kind'] == 'image':
                        asset = self.matter_dir(matter_id) / entry['asset_relpath']
                        if not asset.is_file() or asset.is_symlink():
                            raise ValueError('Image overlay asset is missing; this edit cannot be restored')
                con.execute('DELETE FROM page_annotations WHERE matter_id=?', (matter_id,))
                self._write_page_annotations(con, matter_id, state['entries'])
            elif row['kind'] in ('memo_parties', 'matter_values'):
                table = row['kind']
                field = 'parties' if table == 'memo_parties' else 'values'
                con.execute(f'''INSERT INTO {table} (matter_id,payload_json,updated_at)
                    VALUES (?,?,?) ON CONFLICT(matter_id) DO UPDATE SET
                    payload_json=excluded.payload_json,updated_at=excluded.updated_at''',
                    (matter_id, json.dumps(state[field], ensure_ascii=False), now))
            else:
                raise ValueError('Unsupported edit history entry')
            con.execute('UPDATE matters SET updated_at=? WHERE id=?', (now, matter_id))
            con.execute('UPDATE edit_history SET applied=? WHERE id=?', (int(redo), row['id']))
            self._audit(con, 'edit_redone' if redo else 'edit_undone', matter_id)
        return self.history_state(matter_id)

    def touch(self, matter_id: str):
        with self.connect() as con:
            con.execute('UPDATE matters SET updated_at=? WHERE id=?', (utcnow(), matter_id))

    @staticmethod
    def sha256(path: Path) -> str:
        from .file_io import sha256
        return sha256(path)

    def original_path(self, doc: dict) -> Path:
        return self.matter_dir(doc['matter_id']) / doc['source_relpath']

    def working_path(self, doc: dict) -> Path:
        return self.matter_dir(doc['matter_id']) / doc['working_relpath']

    def next_working_path(self, doc: dict) -> tuple[str, Path]:
        rel = f'working/{doc["id"]}-r{doc["revision"] + 1}.pdf'
        return rel, self.matter_dir(doc['matter_id']) / rel

    def discard_uncommitted_working(self, doc_id: str, rel: str, candidate: Path):
        """Never delete a committed revision after an ambiguous commit failure.

        If SQLite itself is unavailable, leave the file for startup reconciliation.
        Keeping an orphan is safer than deleting a potentially committed revision.
        """
        try:
            with self.connect() as con:
                referenced = con.execute('''SELECT 1 FROM document_revisions
                    WHERE document_id=? AND working_relpath=? LIMIT 1''', (doc_id, rel)).fetchone()
        except (sqlite3.Error, OSError):
            return
        if not referenced:
            candidate.unlink(missing_ok=True)

    def exports_root(self, matter_id: str) -> Path:
        return self.matter_dir(matter_id) / 'exports'

    def build_input_digest(self, matter_id: str, *, effective: bool = False) -> str:
        """Fingerprint inputs that determine a generated bundle, including profile rules."""
        matter = self.get_matter(matter_id)
        if matter is None:
            raise ValueError('Matter not found')
        fields = ('name', 'court_profile', 'case_type', 'petitioner', 'respondent',
                  'advocate', 'case_number', 'max_pages_per_volume', 'keep_groups_intact')
        documents = []
        included_ids = {d['id'] for d in self.get_bundle_documents(matter_id)}
        for doc in self.get_documents(matter_id):
            if effective and doc['id'] not in included_ids:
                continue
            documents.append({
                key: doc[key] for key in ('id', 'position', 'title', 'kind', 'part',
                                           'label', 'label_status', 'label_source', 'group_key',
                                           'group_role',
                                           'document_date', 'date_status',
                                           'date_source', 'date_note', 'page_count',
                                           'revision', 'source_sha256', 'bundle_included',
                                           'certificate_signature_status',
                                           'certificate_signature_count',
                                           'visible_signature_status', 'signature_status')
            } | {'working_sha256': self.sha256(self.working_path(doc)),
                 'original_sha256': self.sha256(self.original_path(doc))}
                | ({'input_updated_at': doc['updated_at']} if effective else {}))
        profiles_path = Path(__file__).resolve().parent / 'rules' / 'profiles.json'
        profiles = json.loads(profiles_path.read_text(encoding='utf-8'))
        profile = next((p for p in profiles if p['id'] == matter['court_profile']), None)
        if profile is None:
            raise ValueError('Court profile is missing')
        payload = {'schema': 2, 'matter': {key: matter[key] for key in fields},
                   'documents': documents, 'profile': profile,
                   'manual_bookmarks': [{key: entry[key] for key in ('id', 'title',
                       'document_id', 'page_index', 'target_revision', 'parent_id', 'position')}
                       for entry in self.list_manual_bookmarks(matter_id)],
                   'reviewed_links': [{key: entry[key] for key in ('id', 'title',
                       'source_document_id', 'source_page_index', 'source_revision',
                       'rect_x', 'rect_y', 'rect_width', 'rect_height', 'kind',
                       'target_document_id', 'target_page_index', 'target_revision',
                       'uri', 'position')} for entry in self.list_reviewed_links(matter_id)],
                   'output_marks': self.get_output_marks(matter_id),
                   'filing_review': self.get_filing_review(matter_id),
                   'output_preset': self.get_applied_output_preset(matter_id)}
        annotations = []
        for entry in self.list_page_annotations(matter_id):
            item = {key: entry[key] for key in self._annotation_fields()}
            if item['kind'] == 'image':
                item['asset_sha256'] = self.sha256(
                    self.matter_dir(matter_id) / item['asset_relpath'])
            annotations.append(item)
        payload['page_annotations'] = annotations
        if effective:
            # Relative included order matters; reference-only insertions do not.
            for position, doc in enumerate(documents):
                doc['position'] = position
            payload['manual_bookmarks'] = [b for b in payload['manual_bookmarks']
                                           if b['document_id'] in included_ids]
            payload['reviewed_links'] = [link for link in payload['reviewed_links']
                                        if link['source_document_id'] in included_ids]
            payload['page_annotations'] = [a for a in annotations
                                          if a['document_id'] in included_ids]
        encoded = json.dumps(payload, sort_keys=True, separators=(',', ':'),
                             ensure_ascii=False).encode('utf-8')
        return hashlib.sha256(encoded).hexdigest()

    def list_exports(self, matter_id: str) -> list[dict]:
        from .file_io import read_bytes
        root = self.exports_root(matter_id)
        if not root.exists():
            return []
        matter = self.get_matter(matter_id)
        exports = []
        try:
            current_digest = self.build_input_digest(matter_id)
            effective_digest = self.build_input_digest(matter_id, effective=True)
        except (OSError, ValueError):
            current_digest = effective_digest = None
        for manifest in root.glob('*/export_manifest.json'):
            if manifest.parent.name.startswith('.'):
                continue
            try:
                info = json.loads(read_bytes(manifest))
                recorded_digest = info.get('build_input_digest')
                if info.get('effective_input_digest'):
                    info['stale'] = info['effective_input_digest'] != effective_digest
                else:
                    info['stale'] = (not recorded_digest or not matter or
                                     info.get('matter_updated_at') != matter['updated_at'] or
                                     recorded_digest != current_digest)
                exports.append(info)
            except (ValueError, OSError):
                continue
        return sorted(exports, key=lambda x: x.get('created_at', ''), reverse=True)

    def get_export_file(self, matter_id: str, export_id: str, filename: str) -> Path | None:
        from .file_io import read_bytes, size
        # Strict export folder identity and path containment.
        if len(export_id) != 32 or not all(c in '0123456789abcdef' for c in export_id):
            return None
        if Path(filename).name != filename or filename.startswith('.'):
            return None
        path = self.exports_root(matter_id) / export_id / filename
        if (path.is_file() and not path.is_symlink() and not path.parent.is_symlink()
                and path.parent.resolve() == (self.exports_root(matter_id) / export_id).resolve()):
            try:
                manifest = json.loads(read_bytes(path.parent / 'export_manifest.json'))
                artifact = next((item for item in manifest['files'] if item['name'] == filename), None)
                if artifact and artifact['bytes'] == size(path):
                    if artifact.get('sha256') and self.sha256(path) != artifact['sha256']:
                        return None
                    return path
            except (OSError, ValueError, KeyError, TypeError):
                return None
        return None

    def save_job(self, job: dict):
        now = utcnow()
        with self.connect() as con:
            con.execute('''INSERT INTO jobs
                (id,matter_id,kind,target_id,status,progress,message,error,result_json,
                 retry_job_id,created_at,updated_at)
                VALUES (?,?,?,?,?,?,?,?,?,?,?,?)
                ON CONFLICT(id) DO UPDATE SET
                status=excluded.status,progress=excluded.progress,message=excluded.message,
                error=excluded.error,result_json=excluded.result_json,
                retry_job_id=excluded.retry_job_id,updated_at=excluded.updated_at''',
                (job['id'], job['matter_id'], job['kind'], job.get('target_id'),
                 job['status'], job['progress'], job['message'], job.get('error'),
                 json.dumps(job.get('result')) if job.get('result') is not None else None,
                 job.get('retry_job_id'), job.get('created_at', now), now))

    @staticmethod
    def _job(row) -> dict:
        item = dict(row)
        item['result'] = json.loads(item.pop('result_json')) if item['result_json'] else None
        return item

    def get_job(self, job_id: str) -> dict | None:
        with self.connect() as con:
            row = con.execute('SELECT * FROM jobs WHERE id=?', (job_id,)).fetchone()
        return self._job(row) if row else None

    def list_jobs(self, matter_id: str) -> list[dict]:
        with self.connect() as con:
            rows = con.execute('SELECT * FROM jobs WHERE matter_id=? ORDER BY created_at DESC',
                               (matter_id,)).fetchall()
        return [self._job(row) for row in rows]

    def interrupt_unfinished_jobs(self):
        with self.connect() as con:
            con.execute('''UPDATE jobs SET status='interrupted',
                message='Interrupted by engine restart; retry starts a new job',
                error='The prior operation did not complete', updated_at=?
                WHERE status IN ('queued','running')''', (utcnow(),))

    def recover_interrupted_work(self):
        from .file_io import read_bytes
        """Remove only unpublished build staging directories after acquiring the data lock."""
        self.interrupt_unfinished_jobs()
        with self.connect() as con:
            matter_ids = [row[0] for row in con.execute('SELECT id FROM matters')]
            cutoff = (datetime.now(timezone.utc) - timedelta(days=90)).isoformat(timespec='microseconds')
            con.execute('''DELETE FROM jobs WHERE status IN ('completed','failed','cancelled')
                AND updated_at < ?''', (cutoff,))
        for matter_id in matter_ids:
            root = self.exports_root(matter_id)
            if root.is_dir():
                for staging in root.glob('.build-*'):
                    if staging.is_dir() and not staging.is_symlink():
                        shutil.rmtree(staging)
            # An engine can stop after the atomic export rename but before its
            # completed job status reaches SQLite. Recover only fully hashed
            # packages; legacy/partial manifests remain interrupted for review.
            for job in self.list_jobs(matter_id):
                if job['status'] != 'interrupted' or job['kind'] != 'build':
                    continue
                if not re.fullmatch(r'[0-9a-f]{32}', job['id']):
                    continue
                try:
                    manifest = json.loads(read_bytes(root / job['id'] / 'export_manifest.json'))
                    if (manifest['id'] != job['id'] or manifest['matter_id'] != matter_id
                            or not manifest['files']):
                        continue
                    if not all(item.get('sha256') and self.get_export_file(matter_id, job['id'], item['name'])
                               for item in manifest['files']):
                        continue
                    job.update(status='completed', progress=100, error=None,
                               message='Recovered verified completed export after restart', result=manifest)
                    self.save_job(job)
                except (OSError, ValueError, KeyError, TypeError):
                    continue
