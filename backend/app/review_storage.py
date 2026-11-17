"""Read-only revision inspection and page comparison; no UI or network dependencies."""
from difflib import SequenceMatcher
from pathlib import Path
import hashlib
from .file_io import open_pdf, size


def revision_record(store, doc, revision):
    with store.connect() as con:
        row = con.execute('SELECT * FROM document_revisions WHERE document_id=? AND revision=?',
                          (doc['id'], revision)).fetchone()
    if row is None:
        raise ValueError('Retained revision not found')
    row = dict(row)
    root = store.matter_dir(doc['matter_id']).resolve()
    path = root / row['working_relpath']
    if not path.is_file() or path.is_symlink() or not path.resolve().is_relative_to(root):
        raise ValueError('Retained revision file is unavailable')
    return row, path


def revisions(store, doc):
    with store.connect() as con:
        rows = con.execute('SELECT revision,created_at,page_count FROM document_revisions WHERE document_id=? ORDER BY revision DESC', (doc['id'],)).fetchall()
    result = []
    for row in rows:
        item = dict(row, available=False, sha256=None, bytes=None, current=row['revision'] == doc['revision'])
        try:
            _, path = revision_record(store, doc, row['revision'])
            item.update(available=True, sha256=store.sha256(path), bytes=size(path))
        except (ValueError, OSError):
            pass
        result.append(item)
    return result


def compare_pages(store, matter_id, build_id, doc, prior_source_page, current_source_page, expected_revision, ignore_generated_numbering=False):
    if doc['matter_id'] != matter_id or doc['revision'] != expected_revision:
        raise ValueError('Working revision changed or belongs to another matter; reopen comparison')
    prior = next((item for item in store.list_exports(matter_id) if item['id'] == build_id), None)
    if not prior:
        raise ValueError('Saved build is unavailable')
    rows = [row for row in prior.get('page_map_snapshot', []) if row['id'] == doc['id']]
    row = next((row for row in rows if row.get('source_start', 0) <= prior_source_page - 1 < row.get('source_end', row.get('source_start', 0) + row['end_page'] - row['start_page'] + 1)), None)
    if not row:
        raise ValueError('This source page is not mapped in the selected build; choose a retained source page')
    if not 1 <= current_source_page <= doc['page_count']:
        raise ValueError('Current source page is outside this revision')
    path = store.get_export_file(matter_id, build_id, 'combined.pdf')
    if not path:
        raise ValueError('Saved PDF is unavailable or failed its integrity check')
    physical = row['combined_start_page'] + prior_source_page - 1 - row.get('source_start', 0)
    with open_pdf(path) as old, open_pdf(store.working_path(doc)) as current:
        a, b = old[physical - 1], current[current_source_page - 1]
        # Bounded extraction and rendering; text differences are candidates, not legal interpretation.
        before, after = a.get_text()[:50000], b.get_text()[:50000]
        numbering_removed = False
        if ignore_generated_numbering:
            import pymupdf
            contract = prior.get('validation_contract', {}).get('combined.pdf', {})
            labels, regions = contract.get('printed_labels', []), contract.get('label_regions', [])
            if len(labels) >= physical and len(regions) >= physical and labels[physical - 1]:
                label, region = str(labels[physical - 1]).strip(), pymupdf.Rect(regions[physical - 1])
                lines = []
                for block in a.get_text('dict')['blocks']:
                    for line in block.get('lines', []):
                        text = ''.join(span['text'] for span in line['spans'])
                        if text.strip() == label and pymupdf.Rect(line['bbox']).intersects(region):
                            numbering_removed = True
                        else:
                            lines.append(text)
                before = '\n'.join(lines)[:50000]

        diffs = []
        left, right = before.splitlines(), after.splitlines()
        for kind, i, j, k, l in SequenceMatcher(None, left, right, autojunk=True).get_opcodes():
            if kind != 'equal':
                diffs.append({'kind': kind, 'before': '\n'.join(left[i:j])[:4000], 'after': '\n'.join(right[k:l])[:4000]})
        def fingerprint(page):
            import pymupdf
            scale = min(1, 1200 / max(page.rect.width, page.rect.height))
            pix = page.get_pixmap(matrix=pymupdf.Matrix(scale, scale), alpha=False)
            return (pix.width, pix.height, hashlib.sha256(pix.samples).hexdigest())
        visually_equal = fingerprint(a) == fingerprint(b)
    return {'build_id': build_id, 'document_id': doc['id'], 'revision': doc['revision'],
            'prior_physical_page': physical, 'prior_source_page': prior_source_page,
            'current_source_page': current_source_page, 'current_page_count': doc['page_count'],
            'prior_source_ranges': [{'first': r.get('source_start', 0) + 1, 'last': r.get('source_end', r.get('source_start', 0) + r['end_page'] - r['start_page'] + 1)} for r in rows],
            'differences': diffs[:100], 'truncated': len(diffs) > 100 or len(before) == 50000 or len(after) == 50000,
            'numbering_removed': numbering_removed, 'rendered_equal': visually_equal, 'has_text': bool(before.strip() or after.strip()),
            'note': ('Recorded generated numbering was excluded from text differences only. ' if numbering_removed else 'Generated numbering and output marks are included. ') + ' Pages are selected independently; text differences may include OCR or layout differences. This is not automatic insertion alignment or legal verification.'}
