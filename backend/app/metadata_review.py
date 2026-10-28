"""Local, evidence-bound proposals. No proposal changes authoritative metadata."""
from __future__ import annotations
import base64
import hashlib
import json
import re
from datetime import date
from pathlib import Path

import pymupdf
from pydantic import BaseModel, ConfigDict, Field, field_validator
from typing import Literal
from .file_io import open_pdf
from .schemas import DocumentPatch
from .storage import utcnow


class ReviewScope(BaseModel):
    model_config = ConfigDict(extra='forbid')
    ids: list[str] = Field(min_length=1, max_length=500)
    pages: dict[str, int] = Field(default_factory=dict)


class AcceptedField(BaseModel):
    model_config = ConfigDict(extra='forbid')
    document_id: str
    field: Literal['title', 'document_date']
    value: str
    evidence_id: str

    @field_validator('value')
    @classmethod
    def visible_value(cls, value):
        value = value.strip()
        if not value or len(value) > 250 or any(ord(c) < 32 for c in value):
            raise ValueError('Enter 1–250 visible characters')
        return value


class AcceptReview(ReviewScope):
    expected_snapshot: str
    fields: list[AcceptedField] = Field(min_length=1, max_length=1000)
    sort_group: str | None = None


class ReviewRow(BaseModel):
    model_config = ConfigDict(extra='forbid')
    document_id: str
    changes: DocumentPatch


class SaveReview(BaseModel):
    model_config = ConfigDict(extra='forbid')
    expected_snapshot: str
    rows: list[ReviewRow] = Field(min_length=1, max_length=500)


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, ensure_ascii=False).encode()).hexdigest()


def snapshot(store, matter_id):
    # Includes reference-only documents and timestamps: reject change/undo ABA too.
    return digest([store.get_matter(matter_id), store.get_documents(matter_id)])


MONTHS = {name.lower(): i for i, names in enumerate([
    ('January', 'Jan'), ('February', 'Feb'), ('March', 'Mar'), ('April', 'Apr'),
    ('May', 'May'), ('June', 'Jun'), ('July', 'Jul'), ('August', 'Aug'),
    ('September', 'Sep'), ('October', 'Oct'), ('November', 'Nov'), ('December', 'Dec')], 1)
    for name in names}
DATE_RE = re.compile(r'\b(?:\d{4}-\d{2}-\d{2}|\d{1,2}[ /.-]\d{1,2}[ /.-]\d{4}|\d{1,2}[ _-]+(?:' + '|'.join(MONTHS) + r')[ ,_-]+\d{4})\b', re.I)


def date_candidates(text):
    values, evidence, uncertain = set(), [], False
    for match in DATE_RE.finditer(text.replace('_', ' ')):
        raw = match.group(); evidence.append(raw)
        try:
            if re.fullmatch(r'\d{4}-\d{2}-\d{2}', raw):
                value = date.fromisoformat(raw)
            else:
                parts = re.split(r'[ ,/._-]+', raw)
                if parts[1].lower() not in MONTHS:
                    # Numeric day/month order is never inferred.
                    uncertain = True
                    continue
                value = date(int(parts[2]), MONTHS[parts[1].lower()], int(parts[0]))
            values.add(value.isoformat())
        except ValueError:
            uncertain = True
    return (next(iter(values)) if len(values) == 1 and not uncertain else ''), evidence, uncertain or len(values) > 1


def proposals(store, matter_id, scope):
    documents = store.get_documents(matter_id)
    by_id = {d['id']: d for d in documents}
    if len(set(scope.ids)) != len(scope.ids) or any(i not in by_id for i in scope.ids):
        raise ValueError('Choose existing documents from this matter once each')
    if any(i not in scope.ids or p < 1 for i, p in scope.pages.items()):
        raise ValueError('Source pages must belong to the review selection')
    rows = []
    for doc_id in scope.ids:
        doc = by_id[doc_id]; page = scope.pages.get(doc_id, 1)
        if page > doc['page_count']:
            raise ValueError('Source page is outside the current document')
        text = ''; available = True; working_sha256 = ''
        try:
            working_sha256 = store.sha256(store.working_path(doc))
            with open_pdf(store.working_path(doc)) as pdf:
                text = pdf[page - 1].get_text()[:40000]
        except (OSError, RuntimeError, ValueError, IndexError):
            available = False
        common = dict(document_id=doc_id, original_name=doc['original_name'], revision=doc['revision'],
                      source_sha256=doc['source_sha256'], working_sha256=working_sha256, page=page, group_key=doc['group_key'])
        title = re.sub(r'[_\s]+', ' ', Path(doc['original_name']).stem).strip()[:250]
        title_row = dict(common, field='title', current=doc['title'], proposed=title,
                         status='proposed' if title else 'unavailable', source='filename',
                         evidence=doc['original_name'], note='Filename wording only; review the title before accepting.')
        value, matches, ambiguous = date_candidates(doc['original_name'] + '\n' + text)
        _, page_matches, _ = date_candidates(text)
        status = 'proposed' if value else 'uncertain' if ambiguous else 'unavailable'
        note = ('Review required: numeric, conflicting or invalid dates.' if ambiguous else
                'No unambiguous date found. Enter a date only after checking the source.' if available else
                'Working source is unavailable. Restore it before accepting a date.')
        date_row = dict(common, field='document_date', current=doc['document_date'], proposed=value,
                        status=status, source='page' if page_matches else 'filename',
                        evidence=' · '.join(dict.fromkeys(matches)) or ('No date in selected page or filename' if available else 'Source unavailable'),
                        note='Source text is not legally verified.' if value else note,
                        available=available)
        for row in (title_row, date_row):
            # Actual selected page text is hashed even when no date was extracted.
            row['evidence_id'] = digest([row, text, available])
            rows.append(row)
    return dict(input_snapshot=snapshot(store, matter_id), rows=rows,
                document_versions={d['id']: d['updated_at'] for d in documents})


def commit(store, matter_id, changes, provenance=None, sort_group=None):
    """Caller holds matter_lock. Metadata + optional group positions + history: one commit."""
    with store.connect() as con:
        con.execute('BEGIN IMMEDIATE')
        docs = [dict(r) for r in con.execute('SELECT * FROM documents WHERE matter_id=? AND archived_at IS NULL ORDER BY position,created_at', (matter_id,))]
        by_id = {d['id']: d for d in docs}
        if any(i not in by_id for i in changes):
            raise ValueError('A reviewed document is missing or archived')
        for doc_id, fields in changes.items():
            if 'title' in fields and not fields['title'].strip():
                raise ValueError('Title must contain visible characters')
            # Validate against the entire final group assignment below.
            by_id[doc_id] = {**by_id[doc_id], **fields}
        parents = set()
        for doc in by_id.values():
            if doc['group_role'] == 'parent':
                if not doc['group_key'] or doc['group_key'] in parents:
                    raise ValueError('Each named group can have only one designated parent')
                parents.add(doc['group_key'])
        if sort_group is not None:
            if not sort_group.strip():
                raise ValueError('Select a named group before sorting')
            group = [by_id[d['id']] for d in docs if d['group_key'] == sort_group]
            if not group or not any(i in changes for i in [d['id'] for d in group]):
                raise ValueError('Sort only a group containing accepted fields')
            ordered = sorted(group, key=lambda d: (d['date_status'] != 'confirmed' or not d['document_date'],
                                                   d['document_date'] if d['date_status'] == 'confirmed' else ''))
            for slot, doc in zip(group, ordered):
                changes.setdefault(doc['id'], {})['position'] = slot['position']
        before, after = [], []
        now = utcnow()
        for doc in docs:
            fields = {k: v for k, v in changes.get(doc['id'], {}).items() if doc[k] != v}
            if not fields:
                continue
            before.append(dict(id=doc['id'], fields={k: doc[k] for k in fields}))
            after.append(dict(id=doc['id'], fields=fields))
            con.execute('UPDATE documents SET ' + ','.join(f'{k}=?' for k in fields) + ',updated_at=? WHERE id=?', (*fields.values(), now, doc['id']))
        if after:
            store._record_edit(con, matter_id, 'document_batch', {'items': before}, {'items': after, 'metadata_evidence': provenance or []})
            store._audit(con, 'metadata_review_accepted', matter_id)
            con.execute('UPDATE matters SET updated_at=? WHERE id=?', (now, matter_id))
    return store.get_documents(matter_id)


def accept(store, matter_id, payload):
    result = proposals(store, matter_id, payload)
    rows = {(r['document_id'], r['field']): r for r in result['rows']}
    changes, evidence, seen = {}, [], set()
    for field in payload.fields:
        key = (field.document_id, field.field)
        if key in seen or key not in rows:
            raise ValueError('Accept each selected field once within the review selection')
        seen.add(key); row = rows[key]
        if field.evidence_id != row['evidence_id']:
            raise RuntimeError('Source evidence changed; reload suggestions before accepting')
        if field.field == 'document_date':
            if not row.get('available'):
                raise ValueError('Source unavailable; date cannot be accepted')
            if not re.fullmatch(r'\d{4}-\d{2}-\d{2}', field.value):
                raise ValueError('Enter an unambiguous ISO date (YYYY-MM-DD)')
            date.fromisoformat(field.value)
            fields = dict(document_date=field.value, date_status='confirmed', date_source='user', date_note='')
        else:
            fields = dict(title=field.value)
        changes.setdefault(field.document_id, {}).update(fields)
        evidence.append(dict(row, accepted_value=field.value))
    return commit(store, matter_id, changes, evidence, payload.sort_group)


def source_preview(store, doc, page):
    if page < 1 or page > doc['page_count']:
        raise ValueError('Source page is outside the document')
    with open_pdf(store.working_path(doc)) as pdf:
        source = pdf[page-1]
        pix = source.get_pixmap(matrix=pymupdf.Matrix(1.4, 1.4), alpha=False)
        return dict(image='data:image/png;base64,' + base64.b64encode(pix.tobytes('png')).decode(),
                    document_id=doc['id'], revision=doc['revision'], page=page, pages=doc['page_count'])
