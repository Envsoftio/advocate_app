"""Versioned, local-only matter backup and restore. No export is a legal certification."""
from __future__ import annotations

import json
import io
import os
import struct
import shutil
import sqlite3
import zipfile
from pathlib import Path, PurePosixPath

from .storage import Store, new_id, utcnow
from .file_io import protected, read_bytes, size as file_size, write_bytes
from .vault import encrypted_sink
from .schemas import (MatterCreate, DocumentPatch, MemoPartiesUpdate, MatterValuesUpdate,
                      ManualBookmark, ReviewedLink, OutputMarks)
from .engine import load_profiles
from .pdf_ops import ProcessingError, validate_overlay_image
from .signatures import combined_signature_status

PACKAGE_VERSION = 10
PROTECTED_PACKAGE_MAGIC = b'CBMAT4\n'
MAX_FILE_BYTES = 1024 * 1024 * 1024
MAX_PACKAGE_BYTES = 4 * MAX_FILE_BYTES


class PortableMatterError(ValueError):
    pass


def protected_package_header(envelope: dict) -> bytes:
    metadata = json.dumps(envelope, separators=(',', ':')).encode('utf-8')
    if len(metadata) > 16 * 1024:
        raise PortableMatterError('Invalid protected package')
    return PROTECTED_PACKAGE_MAGIC + struct.pack('>I', len(metadata)) + metadata


def protected_package_bytes(envelope: dict, encrypted_zip: bytes) -> bytes:
    if not encrypted_zip.startswith(b'CBENC1\n'):
        raise PortableMatterError('Invalid protected package')
    return protected_package_header(envelope) + encrypted_zip


def parse_protected_package(raw: bytes) -> tuple[dict, bytes]:
    if not raw.startswith(PROTECTED_PACKAGE_MAGIC) or len(raw) < len(PROTECTED_PACKAGE_MAGIC) + 4:
        raise PortableMatterError('Invalid protected package')
    offset = len(PROTECTED_PACKAGE_MAGIC)
    length = struct.unpack('>I', raw[offset:offset + 4])[0]
    if not 0 < length <= 16 * 1024 or len(raw) < offset + 4 + length:
        raise PortableMatterError('Invalid protected package header')
    try:
        envelope = json.loads(raw[offset + 4:offset + 4 + length])
    except (ValueError, TypeError) as exc:
        raise PortableMatterError('Invalid protected package header') from exc
    return envelope, raw[offset + 4 + length:]


def _safe_relpath(value: str) -> str:
    path = PurePosixPath(value)
    if (not value or path.is_absolute() or len(path.parts) < 2 or
            path.parts[0] not in ('originals', 'working', 'assets') or
            any(part in ('.', '..') or part.startswith('.') for part in path.parts)):
        raise PortableMatterError('Unsafe matter file path')
    return value


def create_package(store: Store, matter_id: str, destination: Path) -> None:
    matter = store.get_matter(matter_id)
    if matter is None:
        raise PortableMatterError('Matter not found')
    documents = store.get_documents(matter_id) + store.list_archived_documents(matter_id)
    with store.connect() as con:
        revisions = [dict(row) for row in con.execute('''SELECT r.* FROM document_revisions r
            JOIN documents d ON d.id=r.document_id WHERE d.matter_id=?
            ORDER BY r.document_id,r.revision''', (matter_id,))]
    paths = {doc['source_relpath'] for doc in documents}
    paths.update(rev['working_relpath'] for rev in revisions)
    paths.update(doc['working_relpath'] for doc in documents)
    annotations = store.list_page_annotations(matter_id)
    paths.update(item['asset_relpath'] for item in annotations
                 if item['kind'] == 'image' and item['asset_relpath'])
    files = []
    for rel in sorted(paths):
        _safe_relpath(rel)
        path = store.matter_dir(matter_id) / rel
        if not path.is_file() or path.is_symlink():
            raise PortableMatterError(f'Matter file missing or unsafe: {rel}')
        size = file_size(path)
        if size > MAX_FILE_BYTES:
            raise PortableMatterError(f'Matter file exceeds portable package limit: {rel}')
        files.append({'path': rel, 'bytes': size, 'sha256': store.sha256(path)})
    by_path = {item['path']: item for item in files}
    for doc in documents:
        if by_path[doc['source_relpath']]['sha256'] != doc['source_sha256']:
            raise PortableMatterError(f'Original hash mismatch for document {doc["id"]}')
    manifest = {'format': 'courtbundle-matter', 'version': PACKAGE_VERSION,
                'created_at': utcnow(), 'matter': matter, 'documents': documents,
                'revisions': revisions, 'memo_parties': store.get_memo_parties(matter_id),
                'matter_values': store.get_matter_values(matter_id),
                'manual_bookmarks': [{key: item[key] for key in ('id', 'title',
                    'document_id', 'page_index', 'target_revision', 'parent_id', 'position')}
                    for item in store.list_manual_bookmarks(matter_id)],
                'reviewed_links': [{key: item[key] for key in ('id', 'title',
                    'source_document_id', 'source_page_index', 'source_revision',
                    'rect_x', 'rect_y', 'rect_width', 'rect_height', 'kind',
                    'target_document_id', 'target_page_index', 'target_revision',
                    'uri', 'position')} for item in store.list_reviewed_links(matter_id)],
                'output_marks': store.get_output_marks(matter_id),
                'page_annotations': [{key: item[key] for key in store._annotation_fields()}
                                     for item in annotations],
                'files': files}
    destination.parent.mkdir(parents=True, exist_ok=True)
    candidate = destination.with_name(destination.name + '.partial-' + new_id())
    try:
        def fill_archive(output):
            with zipfile.ZipFile(output, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
                archive.writestr('manifest.json', json.dumps(manifest, ensure_ascii=False))
                for item in files:
                    archive.writestr('files/' + item['path'],
                                     read_bytes(store.matter_dir(matter_id) / item['path']))
        if protected(destination):
            with encrypted_sink(destination, store._data_key) as output:
                fill_archive(output)
        else:
            fill_archive(candidate)
            os.replace(candidate, destination)
        store.audit('matter_backed_up', matter_id)
    finally:
        candidate.unlink(missing_ok=True)


def restore_package(store: Store, package: Path | bytes) -> dict:
    matter_id = new_id()
    final = store.matter_dir(matter_id)
    stage = final.with_name('.restore-' + matter_id)
    if stage.exists() or final.exists():
        raise PortableMatterError('Restore path collision')
    try:
        package_source = (io.BytesIO(package) if isinstance(package, bytes) else
                          io.BytesIO(read_bytes(package)) if protected(package) else package)
        with zipfile.ZipFile(package_source) as archive:
            entries = archive.infolist()
            names = [entry.filename for entry in entries]
            if len(names) != len(set(names)) or 'manifest.json' not in names:
                raise PortableMatterError('Duplicate or missing package entries')
            if archive.getinfo('manifest.json').file_size > 10 * 1024 * 1024:
                raise PortableMatterError('Manifest is too large')
            manifest = json.loads(archive.read('manifest.json'))
            if manifest.get('format') != 'courtbundle-matter' or manifest.get('version') not in (1, 2, 3, 4, 5, 6, 7, 8, 9, PACKAGE_VERSION):
                raise PortableMatterError('Unsupported portable matter package')
            files = manifest.get('files')
            if not isinstance(files, list):
                raise PortableMatterError('Invalid package file list')
            expected = {'manifest.json'}
            total = 0
            for item in files:
                rel = _safe_relpath(item['path'])
                entry_name = 'files/' + rel
                expected.add(entry_name)
                entry = archive.getinfo(entry_name)
                total += entry.file_size
                if (entry.is_dir() or entry.file_size != item['bytes'] or
                        entry.file_size > MAX_FILE_BYTES or total > MAX_PACKAGE_BYTES or
                        (entry.external_attr >> 16) & 0o170000 == 0o120000):
                    raise PortableMatterError('Invalid package member size or type')
            if set(names) != expected:
                raise PortableMatterError('Unexpected or missing package entries')
            stage.mkdir(parents=True)
            for item in files:
                rel = item['path']
                target = stage / rel
                target.parent.mkdir(parents=True, exist_ok=True)
                write_bytes(target, archive.read('files/' + rel))
                if store.sha256(target) != item['sha256']:
                    raise PortableMatterError(f'Hash mismatch in package: {rel}')

        matter = manifest['matter']
        documents = manifest['documents']
        revisions = manifest['revisions']
        validated_matter = MatterCreate.model_validate(matter)
        if matter['court_profile'] not in load_profiles():
            raise PortableMatterError('Unknown court profile in package')
        if manifest['memo_parties']:
            MemoPartiesUpdate.model_validate({'parties': manifest['memo_parties']})
        if manifest['version'] >= 3:
            if 'matter_values' not in manifest:
                raise PortableMatterError('Missing matter values in v3 package')
            MatterValuesUpdate.model_validate({'values': manifest['matter_values']})
        by_path = {item['path']: item for item in files}
        active_group_parents: set[str] = set()
        for doc in documents:
            if manifest['version'] < 4:
                doc.setdefault('document_date', '')
                doc.setdefault('date_status', 'unknown')
                doc.setdefault('date_source', 'none')
                doc.setdefault('source_file_mtime', '')
            if manifest['version'] < 5:
                doc.setdefault('date_note', '')
            if manifest['version'] < 6:
                doc.setdefault('label_status', 'unreviewed')
                doc.setdefault('label_source', 'legacy' if doc.get('label') else 'none')
            if manifest['version'] < 7:
                doc.setdefault('group_role', 'member')
            if manifest['version'] < 8:
                doc.setdefault('certificate_signature_status', 'unknown')
                doc.setdefault('certificate_signature_count', 0)
                doc.setdefault('signature_inspection_note',
                               'Legacy package was not inspected for PDF certificate-signature indicators.')
                doc.setdefault('visible_signature_status', 'unreviewed')
                doc.setdefault('signature_status', 'inspection_inconclusive')
            DocumentPatch.model_validate({key: doc[key] for key in
                ('title', 'kind', 'part', 'label', 'label_status', 'label_source',
                 'group_key', 'group_role', 'document_date',
                 'date_status', 'date_source', 'date_note', 'visible_signature_status')})
            if (doc['certificate_signature_status'] not in ('detected', 'not_detected', 'unknown') or
                    type(doc['certificate_signature_count']) is not int or
                    doc['certificate_signature_count'] < 0 or
                    not isinstance(doc['signature_inspection_note'], str) or
                    len(doc['signature_inspection_note']) > 1000 or
                    doc['signature_status'] != combined_signature_status(
                        doc['certificate_signature_status'], doc['visible_signature_status'])):
                raise PortableMatterError('Invalid signature inspection metadata in package')
            if doc['group_role'] == 'parent':
                group_key = doc['group_key'].strip()
                if not group_key:
                    raise PortableMatterError('A group parent must have a group name')
                if doc.get('archived_at') is None:
                    if group_key in active_group_parents:
                        raise PortableMatterError(f'Group “{group_key}” has more than one designated parent')
                    active_group_parents.add(group_key)
            if any(type(doc[key]) is not int for key in ('page_count', 'revision', 'position')) or \
                    doc['page_count'] < 1 or doc['revision'] < 1 or doc['position'] < 0:
                raise PortableMatterError('Invalid document counts in package')
            if not isinstance(doc['source_file_mtime'], str) or len(doc['source_file_mtime']) > 40:
                raise PortableMatterError('Invalid source file timestamp in package')
            if (_safe_relpath(doc['source_relpath']) not in by_path or
                    _safe_relpath(doc['working_relpath']) not in by_path or
                    by_path[doc['source_relpath']]['sha256'] != doc['source_sha256']):
                raise PortableMatterError('Document source or working copy is invalid')
        for rev in revisions:
            if any(type(rev[key]) is not int for key in ('revision', 'page_count')) or \
                    rev['revision'] < 1 or rev['page_count'] < 1:
                raise PortableMatterError('Invalid revision counts in package')
            if _safe_relpath(rev['working_relpath']) not in by_path:
                raise PortableMatterError('Document revision file is missing')
        old_ids = {doc['id'] for doc in documents}
        if len(old_ids) != len(documents) or any(rev['document_id'] not in old_ids for rev in revisions):
            raise PortableMatterError('Invalid document revision references')
        id_map = {old: new_id() for old in old_ids}
        bookmarks = manifest.get('manual_bookmarks', [])
        if manifest['version'] >= 2 and 'manual_bookmarks' not in manifest:
            raise PortableMatterError('Missing manual bookmarks in v2 package')
        if not isinstance(bookmarks, list) or len(bookmarks) > 500:
            raise PortableMatterError('Invalid manual bookmarks')
        bookmark_ids = [item['id'] for item in bookmarks]
        if len(set(bookmark_ids)) != len(bookmark_ids):
            raise PortableMatterError('Duplicate manual bookmark IDs')
        revision_pages = {(rev['document_id'], rev['revision']): rev['page_count']
                          for rev in revisions}
        for index, item in enumerate(bookmarks):
            ManualBookmark.model_validate({key: item[key] for key in ('id', 'title',
                'document_id', 'page_index', 'target_revision', 'parent_id')})
            if (item['document_id'] not in old_ids or item.get('position') != index or
                    item['page_index'] >= revision_pages.get(
                        (item['document_id'], item['target_revision']), 0) or
                    (item['parent_id'] is not None and
                     item['parent_id'] not in bookmark_ids[:index])):
                raise PortableMatterError('Invalid manual bookmark target or hierarchy')
        bookmark_id_map = {old: new_id() for old in bookmark_ids}
        links = manifest.get('reviewed_links', [])
        marks = manifest.get('output_marks', Store.default_output_marks())
        if manifest['version'] >= 9 and ('reviewed_links' not in manifest or
                                         'output_marks' not in manifest):
            raise PortableMatterError('Missing PDF-04 settings in v9 package')
        if not isinstance(links, list) or len(links) > 500:
            raise PortableMatterError('Invalid reviewed links')
        link_ids = [item.get('id') for item in links]
        if len(link_ids) != len(set(link_ids)):
            raise PortableMatterError('Duplicate reviewed link IDs')
        for index, item in enumerate(links):
            ReviewedLink.model_validate({key: item.get(key) for key in (
                'id', 'title', 'source_document_id', 'source_page_index', 'source_revision',
                'rect_x', 'rect_y', 'rect_width', 'rect_height', 'kind',
                'target_document_id', 'target_page_index', 'target_revision', 'uri')})
            if (item.get('position') != index or item['source_document_id'] not in old_ids or
                    item['source_page_index'] >= revision_pages.get(
                        (item['source_document_id'], item['source_revision']), 0)):
                raise PortableMatterError('Invalid reviewed link source')
            if item['kind'] == 'internal' and (
                    item['target_document_id'] not in old_ids or
                    item['target_page_index'] >= revision_pages.get(
                        (item['target_document_id'], item['target_revision']), 0)):
                raise PortableMatterError('Invalid reviewed link target')
        validated_marks = OutputMarks.model_validate(marks).model_dump()
        link_id_map = {old: new_id() for old in link_ids}
        annotations = manifest.get('page_annotations', [])
        if manifest['version'] >= 10 and 'page_annotations' not in manifest:
            raise PortableMatterError('Missing PDF-03 annotations in v10 package')
        if not isinstance(annotations, list) or len(annotations) > 2000:
            raise PortableMatterError('Invalid page annotations')
        annotation_ids = [item.get('id') for item in annotations]
        if len(annotation_ids) != len(set(annotation_ids)):
            raise PortableMatterError('Duplicate page annotation IDs')
        annotation_id_map = {old: new_id() for old in annotation_ids}
        allowed_colors = {'yellow', 'green', 'blue', 'pink'}
        for index, item in enumerate(annotations):
            required = set(store._annotation_fields())
            if set(item) != required or item['kind'] not in ('image', 'highlight', 'comment'):
                raise PortableMatterError('Invalid page annotation fields')
            if (not isinstance(item['id'], str) or len(item['id']) != 32 or
                    item['document_id'] not in old_ids or item['position'] != index or
                    type(item['page_index']) is not int or item['page_index'] < 0 or
                    type(item['target_revision']) is not int or item['target_revision'] < 1 or
                    item['page_index'] >= revision_pages.get(
                        (item['document_id'], item['target_revision']), 0) or
                    item['color'] not in allowed_colors or not isinstance(item['body'], str) or
                    len(item['body']) > 4000):
                raise PortableMatterError('Invalid page annotation target or content')
            for key in ('rect_x', 'rect_y', 'rect_width', 'rect_height'):
                if not isinstance(item[key], (int, float)):
                    raise PortableMatterError('Invalid page annotation region')
            if (item['rect_x'] < 0 or item['rect_y'] < 0 or item['rect_width'] <= 0 or
                    item['rect_height'] <= 0 or item['rect_x'] + item['rect_width'] > 1 or
                    item['rect_y'] + item['rect_height'] > 1):
                raise PortableMatterError('Invalid page annotation region')
            if item['kind'] == 'comment' and not item['body'].strip():
                raise PortableMatterError('Invalid empty comment')
            if item['kind'] == 'image':
                if (item['asset_mime'] not in ('image/png', 'image/jpeg') or
                        not item['asset_relpath'] or
                        _safe_relpath(item['asset_relpath']) not in by_path or
                        by_path[item['asset_relpath']]['bytes'] > 10 * 1024 * 1024):
                    raise PortableMatterError('Invalid image overlay asset')
                try:
                    validate_overlay_image(
                        read_bytes(stage / item['asset_relpath']),
                        'png' if item['asset_mime'] == 'image/png' else 'jpeg')
                except ProcessingError as exc:
                    raise PortableMatterError(f'Invalid image overlay asset: {exc}') from exc
            elif item['asset_relpath'] is not None or item['asset_mime'] is not None:
                raise PortableMatterError('Review annotations cannot contain image assets')
        os.replace(stage, final)
        with store.connect() as con:
            con.execute('BEGIN IMMEDIATE')
            matter_fields = validated_matter.model_dump()
            matter_fields['created_at'] = matter['created_at']
            matter_fields.update(id=matter_id, updated_at=utcnow())
            con.execute('INSERT INTO matters (' + ','.join(matter_fields) + ') VALUES (' +
                        ','.join('?' for _ in matter_fields) + ')', tuple(matter_fields.values()))
            for doc in documents:
                fields = {key: doc[key] for key in (
                    'title', 'kind', 'part', 'label', 'label_status', 'label_source',
                    'group_key', 'group_role', 'position', 'page_count',
                    'document_date', 'date_status', 'date_source', 'date_note', 'source_file_mtime',
                    'ocr_status', 'original_name', 'source_sha256', 'source_relpath',
                    'working_relpath', 'certificate_signature_status',
                    'certificate_signature_count', 'signature_inspection_note',
                    'visible_signature_status', 'signature_status',
                    'created_at', 'updated_at', 'revision', 'archived_at')}
                fields.update(id=id_map[doc['id']], matter_id=matter_id)
                con.execute('INSERT INTO documents (' + ','.join(fields) + ') VALUES (' +
                            ','.join('?' for _ in fields) + ')', tuple(fields.values()))
            for rev in revisions:
                con.execute('''INSERT INTO document_revisions
                    (document_id,revision,working_relpath,page_count,ocr_status,created_at)
                    VALUES (?,?,?,?,?,?)''', (id_map[rev['document_id']], rev['revision'],
                    rev['working_relpath'], rev['page_count'], rev['ocr_status'], rev['created_at']))
            if manifest['memo_parties']:
                con.execute('INSERT INTO memo_parties (matter_id,payload_json,updated_at) VALUES (?,?,?)',
                            (matter_id, json.dumps(manifest['memo_parties'], ensure_ascii=False), utcnow()))
            if manifest['version'] >= 3 and manifest['matter_values']:
                con.execute('INSERT INTO matter_values (matter_id,payload_json,updated_at) VALUES (?,?,?)',
                            (matter_id, json.dumps(manifest['matter_values'], ensure_ascii=False), utcnow()))
            for item in bookmarks:
                con.execute('''INSERT INTO manual_bookmarks
                    (id,matter_id,title,document_id,page_index,target_revision,parent_id,position)
                    VALUES (?,?,?,?,?,?,?,?)''',
                    (bookmark_id_map[item['id']], matter_id, item['title'],
                     id_map[item['document_id']], item['page_index'], item['target_revision'],
                     bookmark_id_map.get(item['parent_id']), item['position']))
            for item in links:
                con.execute('''INSERT INTO reviewed_links
                    (id,matter_id,title,source_document_id,source_page_index,source_revision,
                    rect_x,rect_y,rect_width,rect_height,kind,target_document_id,
                    target_page_index,target_revision,uri,position)
                    VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)''',
                    (link_id_map[item['id']], matter_id, item['title'],
                     id_map[item['source_document_id']], item['source_page_index'],
                     item['source_revision'], item['rect_x'], item['rect_y'],
                     item['rect_width'], item['rect_height'], item['kind'],
                     id_map.get(item['target_document_id']), item['target_page_index'],
                     item['target_revision'], item['uri'], item['position']))
            if manifest['version'] >= 9:
                con.execute('''INSERT INTO output_marks
                    (matter_id,payload_json,updated_at) VALUES (?,?,?)''',
                    (matter_id, json.dumps(validated_marks, sort_keys=True), utcnow()))
            for item in annotations:
                con.execute('''INSERT INTO page_annotations
                    (id,matter_id,document_id,page_index,target_revision,kind,rect_x,rect_y,
                     rect_width,rect_height,body,color,asset_relpath,asset_mime,position)
                    VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)''',
                    (annotation_id_map[item['id']], matter_id,
                     id_map[item['document_id']], item['page_index'], item['target_revision'],
                     item['kind'], item['rect_x'], item['rect_y'], item['rect_width'],
                     item['rect_height'], item['body'], item['color'], item['asset_relpath'],
                     item['asset_mime'], item['position']))
            store._audit(con, 'matter_restored_from_package', matter_id)
    except (KeyError, TypeError, ValueError, zipfile.BadZipFile, sqlite3.Error, OSError) as exc:
        shutil.rmtree(stage, ignore_errors=True)
        shutil.rmtree(final, ignore_errors=True)
        raise PortableMatterError(f'Cannot restore matter: {exc}') from exc
    return store.get_matter(matter_id)
