"""Versioned audience compositions over shared, immutable source revisions."""
from __future__ import annotations

import hashlib
import json

from .storage import new_id, utcnow


def plans(store, matter_id: str) -> list[dict]:
    with store.connect() as con:
        rows = con.execute('SELECT payload_json FROM audience_variants WHERE matter_id=? ORDER BY updated_at DESC',
                           (matter_id,)).fetchall()
    return [json.loads(row[0]) for row in rows]


def get_plan(store, matter_id: str, plan_id: str | None) -> dict | None:
    return next((plan for plan in plans(store, matter_id) if plan['id'] == plan_id), None)


def source_snapshot(store, matter_id: str) -> str:
    # All matter inputs are watched conservatively. Reference-only edits cannot
    # silently enter an audience plan after the reviewer has chosen its sources.
    return store.build_input_digest(matter_id)


def primary_baseline(store, matter_id: str) -> dict:
    digest = source_snapshot(store, matter_id)
    prior = next((item for item in store.list_exports(matter_id)
                  if item.get('artifact_kind', 'primary') == 'primary' and not item.get('stale')), None)
    return dict(kind='primary', id='primary:' + digest, version=None,
                input_snapshot=digest, build_id=prior['id'] if prior else None,
                included_ids=[doc['id'] for doc in store.get_bundle_documents(matter_id)],
                sources=source_contract(store, matter_id, store.get_bundle_documents(matter_id)),
                settings=settings(store, matter_id),
                build_sha256=next((file['sha256'] for file in prior.get('files', [])
                                   if file['name'] == 'combined.pdf'), None) if prior else None)


def settings(store, matter_id: str) -> dict:
    matter = store.get_matter(matter_id)
    return dict(max_pages_per_volume=matter['max_pages_per_volume'],
                keep_groups_intact=bool(matter['keep_groups_intact']),
                output_marks=store.get_output_marks(matter_id))


def ordered_documents(store, matter_id: str, included_ids: list[str]) -> list[dict]:
    docs = store.get_documents(matter_id)
    by_id = {doc['id']: doc for doc in docs}
    if len(included_ids) != len(set(included_ids)) or any(doc_id not in by_id for doc_id in included_ids):
        raise ValueError('Variant membership contains a duplicate or missing document')
    if any(by_id[doc_id]['archived_at'] is not None for doc_id in included_ids):
        raise ValueError('Archived documents cannot enter a variant')
    return [by_id[doc_id] for doc_id in included_ids]


def source_contract(store, matter_id: str, docs: list[dict]) -> list[dict]:
    from .supplements import catalog
    pages = {item['id']: item for item in catalog(store, matter_id, {doc['id'] for doc in docs})}
    return [dict(document_id=doc['id'], working_revision=doc['revision'],
                 working_sha256=store.sha256(store.working_path(doc)),
                 source_sha256=doc['source_sha256'], source_page_count=doc['page_count'],
                 page_ids=[page['id'] for page in pages[doc['id']]['pages']]) for doc in docs]


def plan_contract(plan: dict) -> str:
    payload = {key: plan[key] for key in ('id', 'matter_id', 'version', 'name', 'baseline',
                                           'included_ids', 'sources', 'settings', 'input_snapshot')}
    return hashlib.sha256(json.dumps(payload, ensure_ascii=False, sort_keys=True,
                                     separators=(',', ':')).encode()).hexdigest()


def is_current(store, matter_id: str, plan: dict) -> tuple[bool, str]:
    if plan['input_snapshot'] != source_snapshot(store, matter_id):
        return False, 'Matter inputs changed. Review this variant and save a new version before building.'
    try:
        docs = ordered_documents(store, matter_id, plan['included_ids'])
        if source_contract(store, matter_id, docs) != plan['sources']:
            return False, 'Working source revisions changed. Review membership before building.'
    except (ValueError, OSError) as exc:
        return False, str(exc)
    return True, ''


def save(store, matter_id: str, payload, plan_id: str | None = None) -> dict:
    old = get_plan(store, matter_id, plan_id) if plan_id else None
    if plan_id and not old:
        raise ValueError('Variant plan not found')
    if old and old['version'] != payload.expected_version:
        raise ValueError('Variant changed in another view. Reload before saving.')
    if payload.expected_snapshot != source_snapshot(store, matter_id):
        raise ValueError('Matter inputs changed. Reload and review the variant.')
    if old:
        baseline = old['baseline']
        if payload.baseline_kind != baseline['kind'] or payload.baseline_id != baseline['id']:
            raise ValueError('The baseline is immutable. Create another variant for a new baseline.')
    elif payload.baseline_kind == 'primary':
        baseline = primary_baseline(store, matter_id)
        if payload.baseline_id != baseline['id']:
            raise ValueError('Primary baseline changed. Reload before creating the variant.')
    else:
        source_id, _, version = payload.baseline_id.partition(':')
        base = get_plan(store, matter_id, source_id)
        if not base or str(base['version']) != version:
            raise ValueError('Chosen variant baseline changed or is unavailable.')
        baseline = dict(kind='variant', id=payload.baseline_id, version=base['version'],
                        input_snapshot=base['input_snapshot'], contract_digest=plan_contract(base),
                        included_ids=base['included_ids'], sources=base['sources'], settings=base['settings'])
    docs = ordered_documents(store, matter_id, payload.included_ids)
    marks = payload.output_marks.model_dump()
    if any(doc_id not in payload.included_ids for doc_id in marks['volume_splits']):
        raise ValueError('A volume split references a document outside this variant')
    if any(doc_id not in payload.included_ids for doc_id in marks['annexure_overrides']):
        raise ValueError('An annexure exception references a document outside this variant')
    now = utcnow()
    plan = dict(id=plan_id or new_id(), kind='variant', matter_id=matter_id,
                version=old['version'] + 1 if old else 1, name=payload.name,
                baseline=baseline, included_ids=payload.included_ids,
                sources=source_contract(store, matter_id, docs),
                settings=dict(max_pages_per_volume=payload.max_pages_per_volume,
                              keep_groups_intact=payload.keep_groups_intact,
                              output_marks=marks), input_snapshot=payload.expected_snapshot,
                created_at=old['created_at'] if old else now, updated_at=now)
    with store.connect() as con:
        con.execute('''INSERT INTO audience_variants(id,matter_id,payload_json,updated_at) VALUES(?,?,?,?)
                       ON CONFLICT(id) DO UPDATE SET payload_json=excluded.payload_json,
                                                     updated_at=excluded.updated_at''',
                    (plan['id'], matter_id, json.dumps(plan), now))
        store._audit(con, 'audience_variant_saved', matter_id)
    return plan


def review(store, matter_id: str, plan: dict) -> dict:
    from .engine import bundle_preview, preflight
    from .engine import load_profiles
    docs = ordered_documents(store, matter_id, plan['included_ids'])
    options = plan['settings']
    preview = bundle_preview(store, matter_id, options['max_pages_per_volume'],
                             options['keep_groups_intact'], options['output_marks'],
                             documents_override=docs)
    profile = load_profiles()[store.get_matter(matter_id)['court_profile']]
    checks = preflight(store, matter_id, profile, documents_override=docs,
                       settings_override=options['output_marks'])
    current, reason = is_current(store, matter_id, plan)
    baseline_ids = set(plan['baseline'].get('included_ids', []))
    included = set(plan['included_ids'])
    by_id = {doc['id']: doc for doc in store.get_documents(matter_id)}
    baseline_sources = {source['document_id']: source for source in plan['baseline'].get('sources', [])}
    changes = [dict(document_id=doc['id'], title=doc['title'],
                    state=('added' if doc['id'] in included else 'excluded') if
                    (doc['id'] in included) != (doc['id'] in baseline_ids) else
                    'changed' if doc['id'] in included and baseline_sources.get(doc['id']) and
                    (baseline_sources[doc['id']]['working_revision'] != doc['revision'] or
                     baseline_sources[doc['id']]['working_sha256'] != store.sha256(store.working_path(doc))) else
                    'unchanged',
                    pages=doc['page_count'], revision=doc['revision'])
               for doc in by_id.values()]
    baseline_settings = plan['baseline'].get('settings', {})
    settings_differences = [key for key in ('max_pages_per_volume', 'keep_groups_intact')
                            if baseline_settings.get(key) != options.get(key)]
    settings_differences += [f'output_marks.{key}' for key in options['output_marks']
                             if baseline_settings.get('output_marks', {}).get(key) != options['output_marks'][key]]
    return dict(plan=plan, contract_digest=plan_contract(plan), current=current,
                stale_reason=reason, preview=preview, checks=checks,
                differences=changes, settings_differences=settings_differences,
                included_documents=docs,
                baseline=plan['baseline'], output_history=[item for item in store.list_exports(matter_id)
                    if item.get('variant_plan_id') == plan['id']])
