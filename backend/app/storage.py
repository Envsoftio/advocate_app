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


def utcnow() -> str:
    return datetime.now(timezone.utc).isoformat(timespec='microseconds')


def new_id() -> str:
    return uuid.uuid4().hex


class Store:
    HISTORY_LIMIT = 100
    def __init__(self, root: Path):
        self.root = Path(root).expanduser().resolve()
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
        self._matter_locks_guard = threading.Lock()
        self._matter_locks: dict[str, threading.Lock] = {}
        try:
            self._initialize()
            self.recover_temporary_packages()
            self.recover_orphan_working_revisions()
        except BaseException:
            self.close()
            raise

    def close(self):
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
        """Serialize edits and jobs for one matter within the local sidecar."""
        with self._matter_locks_guard:
            return self._matter_locks.setdefault(matter_id, threading.Lock())

    @contextmanager
    def connect(self):
        con = sqlite3.connect(self.dbpath, timeout=30)
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
            if version > 4:
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
                    label TEXT NOT NULL, group_key TEXT NOT NULL, position INTEGER NOT NULL,
                    page_count INTEGER NOT NULL, ocr_status TEXT NOT NULL,
                    original_name TEXT NOT NULL, source_sha256 TEXT NOT NULL,
                    source_relpath TEXT NOT NULL, working_relpath TEXT NOT NULL,
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
                ocr_status TEXT NOT NULL, created_at TEXT NOT NULL,
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
            if version < 3:
                matter_columns = {row['name'] for row in con.execute('PRAGMA table_info(matters)')}
                document_columns = {row['name'] for row in con.execute('PRAGMA table_info(documents)')}
                if 'archived_at' not in matter_columns:
                    con.execute('ALTER TABLE matters ADD COLUMN archived_at TEXT')
                if 'archived_at' not in document_columns:
                    con.execute('ALTER TABLE documents ADD COLUMN archived_at TEXT')
            if version < 4:
                con.execute('PRAGMA user_version=4')

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
            con.execute('''INSERT INTO memo_parties (matter_id,payload_json,updated_at)
                VALUES (?,?,?) ON CONFLICT(matter_id) DO UPDATE SET
                payload_json=excluded.payload_json,updated_at=excluded.updated_at''',
                (matter_id, json.dumps(parties, ensure_ascii=False), utcnow()))
        return parties

    def get_document(self, doc_id: str) -> dict | None:
        with self.connect() as con:
            row = con.execute('SELECT * FROM documents WHERE id=? AND archived_at IS NULL', (doc_id,)).fetchone()
        return dict(row) if row else None

    def add_document(self, data: dict) -> dict:
        with self.connect() as con:
            pos = con.execute('SELECT COALESCE(MAX(position),-1)+1 FROM documents WHERE matter_id=?', (data['matter_id'],)).fetchone()[0]
            item = dict(data, position=pos, created_at=utcnow(), updated_at=utcnow())
            con.execute('INSERT INTO documents (' + ','.join(item) + ') VALUES (' + ','.join('?' for _ in item) + ')', tuple(item.values()))
            con.execute('''INSERT INTO document_revisions
                (document_id,revision,working_relpath,page_count,ocr_status,created_at)
                VALUES (?,?,?,?,?,?)''',
                (item['id'], 1, item['working_relpath'], item['page_count'], item['ocr_status'], item['updated_at']))
            self._record_edit(con, data['matter_id'], 'document_presence',
                              {'id': data['id'], 'archived_at': item['created_at'], 'position': pos},
                              {'id': data['id'], 'archived_at': None, 'position': pos})
            self._audit(con, 'document_imported', data['matter_id'], data['id'])
        self.touch(data['matter_id'])
        return self.get_document(data['id'])

    def update_document(self, doc_id: str, changes: dict, record_history: bool = True):
        if not changes:
            return self.get_document(doc_id)
        changes = dict(changes)
        changes['updated_at'] = utcnow()
        with self.connect() as con:
            current = con.execute('SELECT * FROM documents WHERE id=?', (doc_id,)).fetchone()
            if current is None:
                raise ValueError('Document not found')
            matter_id = current['matter_id']
            tracked = {key: value for key, value in changes.items() if key != 'updated_at'}
            if all(current[key] == value for key, value in tracked.items()):
                return dict(current)
            if 'working_relpath' in changes:
                changes['revision'] = current['revision'] + 1
            con.execute('UPDATE documents SET ' + ','.join(f'{k}=?' for k in changes) + ' WHERE id=?', (*changes.values(), doc_id))
            if 'working_relpath' in changes:
                con.execute('''INSERT INTO document_revisions
                    (document_id,revision,working_relpath,page_count,ocr_status,created_at)
                    VALUES (?,?,?,?,?,?)''', (doc_id, changes['revision'], changes['working_relpath'],
                                            changes.get('page_count', current['page_count']),
                                            changes.get('ocr_status', current['ocr_status']), changes['updated_at']))
            if record_history:
                tracked = {key: value for key, value in changes.items()
                           if key not in ('updated_at', 'revision')}
                self._record_edit(con, matter_id, 'document',
                                  {'id': doc_id, 'fields': {key: current[key] for key in tracked}},
                                  {'id': doc_id, 'fields': tracked})
            self._audit(con, 'document_updated', matter_id, doc_id)
        self.touch(matter_id)
        return self.get_document(doc_id)

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
            previous = con.execute('''SELECT archived_at,position FROM documents
                WHERE id=? AND matter_id=? AND archived_at IS NOT NULL''', (doc_id, matter_id)).fetchone()
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
        with self.connect() as con:
            undo = con.execute('SELECT COUNT(*) FROM edit_history WHERE matter_id=? AND applied=1',
                               (matter_id,)).fetchone()[0]
            redo = con.execute('SELECT COUNT(*) FROM edit_history WHERE matter_id=? AND applied=0',
                               (matter_id,)).fetchone()[0]
        return {'can_undo': bool(undo), 'can_redo': bool(redo), 'undo_count': undo, 'redo_count': redo}

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
                con.execute('''UPDATE documents SET archived_at=?,position=?,updated_at=? WHERE id=?''',
                            (state['archived_at'], state['position'], now, state['id']))
            elif row['kind'] == 'document':
                doc = con.execute('''SELECT * FROM documents WHERE id=? AND matter_id=?
                    AND archived_at IS NULL''', (state['id'], matter_id)).fetchone()
                if doc is None:
                    raise ValueError('Document was removed; this edit cannot be restored')
                fields = state['fields']
                if 'working_relpath' in fields:
                    if not (self.matter_dir(matter_id) / fields['working_relpath']).is_file():
                        raise ValueError('Working copy is missing; this edit cannot be restored')
                    revision = doc['revision'] + 1
                    con.execute('''INSERT INTO document_revisions
                        (document_id,revision,working_relpath,page_count,ocr_status,created_at)
                        VALUES (?,?,?,?,?,?)''',
                        (state['id'], revision, fields['working_relpath'],
                         fields.get('page_count', doc['page_count']),
                         fields.get('ocr_status', doc['ocr_status']), now))
                    fields = dict(fields, revision=revision)
                changes = dict(fields, updated_at=now)
                con.execute('UPDATE documents SET ' + ','.join(f'{key}=?' for key in changes) +
                            ' WHERE id=?', (*changes.values(), state['id']))
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
        h = hashlib.sha256()
        with path.open('rb') as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                h.update(chunk)
        return h.hexdigest()

    def original_path(self, doc: dict) -> Path:
        return self.matter_dir(doc['matter_id']) / doc['source_relpath']

    def working_path(self, doc: dict) -> Path:
        return self.matter_dir(doc['matter_id']) / doc['working_relpath']

    def next_working_path(self, doc: dict) -> tuple[str, Path]:
        rel = f'working/{doc["id"]}-r{doc["revision"] + 1}.pdf'
        return rel, self.matter_dir(doc['matter_id']) / rel

    def exports_root(self, matter_id: str) -> Path:
        return self.matter_dir(matter_id) / 'exports'

    def list_exports(self, matter_id: str) -> list[dict]:
        root = self.exports_root(matter_id)
        if not root.exists():
            return []
        matter = self.get_matter(matter_id)
        exports = []
        for manifest in root.glob('*/export_manifest.json'):
            try:
                info = json.loads(manifest.read_text(encoding='utf-8'))
                info['stale'] = not matter or info.get('matter_updated_at') != matter['updated_at']
                exports.append(info)
            except (ValueError, OSError):
                continue
        return sorted(exports, key=lambda x: x.get('created_at', ''), reverse=True)

    def get_export_file(self, matter_id: str, export_id: str, filename: str) -> Path | None:
        # Strict export folder identity and path containment.
        if len(export_id) != 32 or not all(c in '0123456789abcdef' for c in export_id):
            return None
        if Path(filename).name != filename or filename.startswith('.'):
            return None
        path = self.exports_root(matter_id) / export_id / filename
        if path.is_file() and path.parent.resolve() == (self.exports_root(matter_id) / export_id).resolve():
            return path
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
