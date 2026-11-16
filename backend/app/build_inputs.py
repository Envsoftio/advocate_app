"""Durable, matter-scoped build inputs. Never restore a snapshot over live work.

The frozen SQLCipher/SQLite store reuses the engine and its deterministic map.
Only selected matter metadata and referenced source/working/asset bytes are copied;
no other matters, edit history, passwords or prior output files are duplicated.
"""
from __future__ import annotations
import json
import hashlib
import os
import re
import shutil
from pathlib import Path
from .storage import Store, new_id, utcnow
from .file_io import copy_file, read_bytes, write_bytes, write_chunks, iter_plain_chunks
from .pdf_ops import ProcessingError

MATTER_TABLES = ('documents', 'memo_parties', 'matter_values', 'manual_bookmarks',
    'reviewed_links', 'output_marks', 'matter_output_presets', 'page_annotations',
    'source_page_identities', 'supplemental_plans', 'bundle_membership_state', 'audience_variants')
DOCUMENT_TABLES = ('document_revisions', 'redaction_records', 'ocr_page_reviews')


def root_for(store, matter_id, snapshot_id):
    if not re.fullmatch(r'[0-9a-f]{32}', snapshot_id):
        raise ProcessingError('Invalid saved build input identity')
    root = store.matter_dir(matter_id) / 'build-inputs' / snapshot_id
    if root.is_symlink() or not root.resolve().is_relative_to(store.root):
        raise ProcessingError('Unsafe saved build input location')
    return root


class FrozenStore(Store):
    def __init__(self, live, root):
        self.live = live
        super().__init__(root, live._data_key)

    def exports_root(self, matter_id):
        # Baselines and newly generated immutable outputs belong to the live store.
        return self.live.exports_root(matter_id)


def table_digest(store):
    with store.connect() as con:
        content = {table: sorted([dict(row) for row in con.execute(f'SELECT * FROM {table}')],
                    key=lambda row: json.dumps(row, sort_keys=True))
                   for table in ('matters', *MATTER_TABLES, *DOCUMENT_TABLES)}
    return hashlib.sha256(json.dumps(content, sort_keys=True).encode()).hexdigest()


def capture(store, matter_id, *, kind='build', plan=None):
    from .engine import bundle_preview, supplemental_preview
    from .supplements import catalog
    from . import variants
    snapshot_id = new_id()
    destination = root_for(store, matter_id, snapshot_id)
    temporary = destination.with_name('.capture-' + snapshot_id)
    frozen = None
    try:
        # Preserve the live page identities so retry never manufactures new ones.
        documents = store.get_documents(matter_id)
        try:
            catalog(store, matter_id, {d['id'] for d in documents if d['bundle_included']} if kind == 'build' else None)
        except (OSError, ValueError):
            pass  # The worker reports the actual unavailable/corrupt input.
        with store.connect() as source:
            tables = {'matters': [dict(source.execute('SELECT * FROM matters WHERE id=?', (matter_id,)).fetchone())]}
            for table in MATTER_TABLES:
                tables[table] = [dict(row) for row in source.execute(f'SELECT * FROM {table} WHERE matter_id=?', (matter_id,))]
            for table in DOCUMENT_TABLES:
                tables[table] = [dict(row) for row in source.execute(f'SELECT r.* FROM {table} r JOIN documents d ON d.id=r.document_id WHERE d.matter_id=?', (matter_id,))]
        frozen = FrozenStore(store, temporary)
        with frozen.connect() as target:
            for table, rows in tables.items():
                for row in rows:
                    columns = ','.join('"'+key+'"' for key in row)
                    target.execute(f'INSERT INTO {table} ({columns}) VALUES ({",".join("?" for _ in row)})', tuple(row.values()))
        paths = {d[key] for d in tables['documents'] for key in ('source_relpath', 'working_relpath')}
        if kind == 'supplement-build':
            paths.update(r['working_relpath'] for r in tables['document_revisions'])
        paths.update(a['asset_relpath'] for a in tables['page_annotations'] if a['asset_relpath'])
        unavailable = []
        file_hashes = {}
        for rel in sorted(paths):
            source = store.matter_dir(matter_id) / rel
            if Path(rel).is_absolute() or '..' in Path(rel).parts or source.is_symlink() or not source.resolve().is_relative_to(store.matter_dir(matter_id).resolve()):
                raise ProcessingError('Unsafe referenced build input path')
            if not source.is_file():
                unavailable.append(rel)
                continue
            write_chunks(frozen.matter_dir(matter_id) / rel, iter_plain_chunks(source))
            file_hashes[rel] = frozen.sha256(frozen.matter_dir(matter_id) / rel)
        selected = [d for d in documents if d['bundle_included']]
        source_pages = sum(d['page_count'] for d in selected)
        if kind == 'variant-build':
            selected = variants.ordered_documents(store, matter_id, plan['included_ids'])
            source_pages = sum(d['page_count'] for d in selected)
        elif kind == 'supplement-build':
            from .supplements import selection
            resolved, _ = selection(store, matter_id, plan['selected_page_ids'])
            ids = {p['document_id'] for p in resolved}
            selected = [d for d in documents if d['id'] in ids]
            source_pages = len(resolved)
        summary = dict(snapshot_id=snapshot_id, captured_at=utcnow(), kind=kind,
            plan_id=plan['id'] if plan else None, included_documents=len(selected),
            source_pages=source_pages, generated_pages=None, physical_pages=None, volumes=None,
            documents=[{key: d[key] for key in ('id', 'title', 'revision', 'page_count', 'signature_status', 'certificate_signature_status', 'visible_signature_status')} for d in selected])
        try:
            summary['input_digest'] = frozen.build_input_digest(matter_id)
        except (OSError, ValueError):
            summary['input_digest'] = None
        try:
            matter = frozen.get_matter(matter_id)
            settings = plan['settings'] if plan else dict(max_pages_per_volume=matter['max_pages_per_volume'], keep_groups_intact=matter['keep_groups_intact'], output_marks=frozen.get_output_marks(matter_id))
            preview = (supplemental_preview(frozen, matter_id, plan['selected_page_ids'], settings)
                if kind == 'supplement-build' else bundle_preview(frozen, matter_id,
                    settings['max_pages_per_volume'], settings['keep_groups_intact'], settings['output_marks'],
                    documents_override=selected))
            summary.update(generated_pages=preview['combined_pages']-source_pages,
                physical_pages=preview['combined_pages'], volumes=len(preview['volumes']))
        except (OSError, ValueError, ProcessingError) as exc:
            summary['plan_error'] = str(exc)
        from .engine import load_profiles
        metadata = dict(summary=summary, plan=plan, profile=load_profiles()[matter['court_profile']], unavailable=unavailable, file_hashes=file_hashes, tables_digest=table_digest(frozen))
        write_bytes(temporary / 'snapshot.json', json.dumps(metadata).encode())
        frozen.close()
        frozen = None
        os.replace(temporary, destination)
        return summary
    except BaseException:
        if frozen:
            frozen.close()
        shutil.rmtree(temporary, ignore_errors=True)
        raise


def load(store, matter_id, context):
    root = root_for(store, matter_id, context['snapshot_id'])
    if any(not (root / name).is_file() or (root / name).is_symlink() for name in ('snapshot.json', 'court_bundle.sqlite3')):
        raise ProcessingError('Saved build inputs are unavailable. Build current inputs instead.')
    metadata = json.loads(read_bytes(root / 'snapshot.json'))
    if metadata['summary'] != context:
        raise ProcessingError('Saved build input contract changed; retry is blocked.')
    return root, metadata


def run(store, matter_id, context, job_id, progress, cancelled, acknowledged):
    from .engine import build_bundle, load_profiles
    root, metadata = load(store, matter_id, context)
    frozen = FrozenStore(store, root)
    try:
        matter = frozen.get_matter(matter_id)
        if table_digest(frozen) != metadata['tables_digest']:
            raise ProcessingError('Saved build metadata failed integrity verification')
        for rel, digest in metadata['file_hashes'].items():
            path = frozen.matter_dir(matter_id) / rel
            if not path.resolve().is_relative_to(root) or path.is_symlink() or frozen.sha256(path) != digest:
                raise ProcessingError('Saved build file failed integrity verification')
        if metadata['profile'] != load_profiles().get(matter['court_profile']):
            raise ProcessingError('The technical profile changed since submission. Review and build current inputs.')
        if context.get('input_digest') and frozen.build_input_digest(matter_id) != context['input_digest']:
            raise ProcessingError('Saved build inputs failed integrity verification; retry is blocked.')
        if cancelled():
            raise ProcessingError('Operation cancelled')
        if not context.get('input_digest'):
            from .engine import preflight
            problems = preflight(frozen, matter_id, metadata['profile'])
            errors = [item for item in problems if item['severity'] == 'error']
            if errors:
                raise ProcessingError('Preflight failed: ' + '; '.join(item['message'] for item in errors),
                                      recovery={'failures': errors})
        result = build_bundle(frozen, matter_id, job_id, progress, cancelled,
            signature_acknowledged=acknowledged, input_snapshot_id=context['snapshot_id'],
            supplement_plan=metadata['plan'] if context['kind'] == 'supplement-build' else None,
            variant_plan=metadata['plan'] if context['kind'] == 'variant-build' else None)
        # Copy the immutable measured checks, then evaluate freshness against live inputs.
        source = frozen.matter_dir(matter_id) / 'validation' / job_id
        destination = store.matter_dir(matter_id) / 'validation' / job_id
        if source.exists():
            for path in source.rglob('*'):
                if path.is_file():
                    copy_file(path, destination / path.relative_to(source))
        return result
    finally:
        frozen.close()


def cleanup(store, matter_id):
    """Snapshots follow durable job retention; interrupted attempts remain retryable."""
    root = store.matter_dir(matter_id) / 'build-inputs'
    if not root.is_dir() or root.is_symlink():
        return
    retained = {(job.get('input_context') or {}).get('snapshot_id') for job in store.list_jobs(matter_id)}
    for child in root.iterdir():
        if child.is_dir() and not child.is_symlink() and (
                re.fullmatch(r'\.capture-[0-9a-f]{32}', child.name) or
                (re.fullmatch(r'[0-9a-f]{32}', child.name) and child.name not in retained)):
            shutil.rmtree(child)
