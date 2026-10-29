"""Revision-bound supplemental plans. No generated artifact or insertion policy in B04A."""
import json
from .storage import new_id, utcnow
from .schemas import OutputMarks


def catalog(store, matter_id, document_ids=None):
    """Opaque persistent IDs identify a page in an immutable working revision.

    No positional/content heuristics carry identity to a different revision.
    Undo to the same stored file recovers its IDs; changed bytes fail closed.
    """
    result = []
    for doc in store.get_documents(matter_id):
        if document_ids is not None and doc['id'] not in document_ids:
            continue
        digest = store.sha256(store.working_path(doc))
        with store.connect() as con:
            rows = con.execute('SELECT * FROM source_page_identities WHERE document_id=? AND working_relpath=? ORDER BY page_index',
                               (doc['id'], doc['working_relpath'])).fetchall()
            if not rows:
                con.executemany('INSERT INTO source_page_identities VALUES (?,?,?,?,?,?,?)',
                    [(new_id(), matter_id, doc['id'], doc['working_relpath'], doc['revision'], i, digest)
                     for i in range(doc['page_count'])])
                rows = con.execute('SELECT * FROM source_page_identities WHERE document_id=? AND working_relpath=? ORDER BY page_index',
                                   (doc['id'], doc['working_relpath'])).fetchall()
        if len(rows) != doc['page_count'] or any(r['working_sha256'] != digest for r in rows):
            raise ValueError('Stored working bytes changed unexpectedly. Restore the working revision before selecting pages.')
        result.append({**{k: doc[k] for k in ('id', 'title', 'revision', 'page_count', 'bundle_included')},
            'pages': [{'id': r['id'], 'page_index': r['page_index'], 'revision': doc['revision']} for r in rows]})
    return result


def plans(store, matter_id):
    with store.connect() as con:
        return [json.loads(r[0]) for r in con.execute(
            'SELECT payload_json FROM supplemental_plans WHERE matter_id=? ORDER BY updated_at DESC', (matter_id,))]


def get_plan(store, matter_id, plan_id):
    return next((p for p in plans(store, matter_id) if p['id'] == plan_id), None)


def baseline(store, matter_id, build_id):
    prior = next((b for b in store.list_exports(matter_id) if b['id'] == build_id), None)
    if not prior:
        raise ValueError('Baseline build is unavailable. The plan cannot be rebound to another build.')
    return prior


def snapshot(store, matter_id):
    # Includes all documents, even reference-only sources available to a supplement.
    return store.build_input_digest(matter_id)


def selection(store, matter_id, ids):
    current = {p['id']: (d, p) for d in catalog(store, matter_id) for p in d['pages']}
    with store.connect() as con:
        known = {r['id']: dict(r) for r in con.execute('SELECT * FROM source_page_identities WHERE matter_id=?', (matter_id,))}
    resolved, issues = [], []
    for page_id in ids:
        if page_id not in known:
            issues.append({'page_id': page_id, 'reason': 'Unknown page identity in this matter'})
        elif page_id not in current:
            r = known[page_id]
            issues.append({'page_id': page_id, 'document_id': r['document_id'], 'page_index': r['page_index'],
                           'reason': 'Missing/deleted page or stale working revision. Explicitly remove or reselect; no automatic rebinding.'})
        else:
            d, p = current[page_id]
            resolved.append({'id': page_id, 'document_id': d['id'], 'page_index': p['page_index'], 'revision': d['revision']})
    return resolved, issues


def save(store, matter_id, payload, plan_id=None):
    if payload.expected_snapshot != snapshot(store, matter_id):
        raise ValueError('Matter inputs changed. Reload material and review the exact selected pages before saving.')
    prior = baseline(store, matter_id, payload.baseline_build_id)
    old = get_plan(store, matter_id, plan_id) if plan_id else None
    if plan_id and (old is None or old['version'] != payload.expected_version):
        raise ValueError('Supplement changed in another view. Reopen the saved plan before applying changes.')
    if old and old['baseline_build_id'] != prior['id']:
        raise ValueError('A saved supplement baseline is immutable. Create a new plan to use another build.')
    if len(set(payload.selected_page_ids)) != len(payload.selected_page_ids):
        raise ValueError('Selected page identities must be unique')
    _, issues = selection(store, matter_id, payload.selected_page_ids)
    if issues:
        raise ValueError(issues[0]['reason'])
    if payload.output_marks.volume_splits:
        raise ValueError('Primary-bundle internal split policies cannot be applied to a supplemental plan.')
    now = utcnow()
    data = dict(id=plan_id or new_id(), kind='supplement', matter_id=matter_id,
        version=old['version']+1 if old else 1, baseline_build_id=prior['id'],
        baseline_created_at=prior['created_at'], baseline_pages=prior['pages'],
        baseline_files=prior.get('file_metadata', prior.get('files', [])),
        title=payload.title.strip(), selected_page_ids=payload.selected_page_ids,
        settings={'max_pages_per_volume': payload.max_pages_per_volume,
                  'keep_groups_intact': payload.keep_groups_intact, 'output_marks': payload.output_marks.model_dump()},
        created_at=old['created_at'] if old else now, updated_at=now)
    if not data['title']:
        raise ValueError('Enter a supplement title')
    with store.connect() as con:
        con.execute('INSERT INTO supplemental_plans VALUES (?,?,?,?) ON CONFLICT(id) DO UPDATE SET payload_json=excluded.payload_json,updated_at=excluded.updated_at',
                    (data['id'], matter_id, json.dumps(data), now))
        store._audit(con, 'supplement_plan_saved', matter_id)
    return data


def draft_defaults(store, matter_id):
    marks = OutputMarks.model_validate(store.get_output_marks(matter_id)).model_dump()
    marks.update(front_matter='cover_index', numbering_prefix='S1 · ', numbering_start=1,
                 volume_splits={}, annexure_overrides={})
    matter = store.get_matter(matter_id)
    return dict(max_pages_per_volume=matter['max_pages_per_volume'], keep_groups_intact=False, output_marks=marks)
