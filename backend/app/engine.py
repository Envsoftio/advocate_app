"""Deterministic PDF bundle engine.

This is filing-preparation software, not a court-certified rules engine.
Index positions, bookmark destinations and the emitted CSV share one page map.
"""
from __future__ import annotations
import csv
import io
import json
import math
import os
from pathlib import Path
import shutil
import tempfile
import zipfile

import pymupdf

from .pdf_ops import ProcessingError
from .storage import Store, utcnow

INDEX_ROWS_PER_PAGE = 23
A4_W, A4_H = 595, 842
EXPORT_INTERACTIVE_CONTENT_POLICY = {
    'source_annotations': 'omitted',
    'source_links': 'omitted',
    'generated_index_links': 'retained',
}


def load_profiles() -> dict:
    path = Path(__file__).resolve().parent / 'rules' / 'profiles.json'
    return {p['id']: p for p in json.loads(path.read_text(encoding='utf-8'))}


def original_integrity_issues(store: Store, docs: list[dict]) -> list[dict]:
    """Compare imported originals with their recorded hashes without changing either."""
    issues = []
    for doc in docs:
        original = store.original_path(doc)
        common = dict(severity='error', document_id=doc['id'])
        if not original.is_file():
            issues.append(dict(code='MISSING_ORIGINAL',
                               message=f'Original missing for {doc["title"]} (document {doc["id"]}).', **common))
            continue
        try:
            actual_hash = store.sha256(original)
        except OSError as exc:
            issues.append(dict(code='ORIGINAL_UNREADABLE',
                               message=f'Cannot read original for {doc["title"]} (document {doc["id"]}): {exc}.', **common))
            continue
        if actual_hash != doc['source_sha256']:
            issues.append(dict(code='ORIGINAL_HASH_MISMATCH',
                               message=f'Original bytes changed for {doc["title"]} (document {doc["id"]}); import hash does not match.', **common))
    return issues


def preflight(store: Store, matter_id: str, profile: dict) -> list[dict]:
    docs = store.get_documents(matter_id)
    issues = []
    if not docs:
        issues.append(dict(severity='error', code='NO_DOCUMENTS', message='Import at least one document before building.', document_id=None))
        return issues
    if profile['status'] != 'APPROVED':
        issues.append(dict(severity='info', code='PROFILE_NOT_APPROVED', message='This is a demo/illustrative profile, not a verified court-compliance determination.', document_id=None))
    issues.extend(original_integrity_issues(store, docs))
    seen_hashes = set()
    seen_labels = set()
    for doc in docs:
        common = dict(document_id=doc['id'])
        if doc['source_sha256'] in seen_hashes:
            issues.append(dict(severity='warning', code='DUPLICATE_INPUT', message=f'Possible duplicate original: {doc["title"]}', **common))
        seen_hashes.add(doc['source_sha256'])
        if doc['kind'] == 'annexure' and not doc['label']:
            issues.append(dict(severity='warning', code='MISSING_ANNEXURE_LABEL', message=f'Annexure {doc["title"]} has no identifier.', **common))
        if doc['label']:
            key = doc['label'].casefold().strip()
            if key in seen_labels:
                issues.append(dict(severity='warning', code='DUPLICATE_LABEL', message=f'Duplicate annexure/document label: {doc["label"]}', **common))
            seen_labels.add(key)
        working = store.working_path(doc)
        if not working.is_file():
            issues.append(dict(severity='error', code='MISSING_WORKING_COPY', message=f'Working file missing: {doc["title"]}', **common))
            continue
        try:
            with pymupdf.open(working) as pdf:
                if pdf.needs_pass:
                    issues.append(dict(severity='error', code='ENCRYPTED_INPUT', message=f'Encrypted input: {doc["title"]}', **common))
                annotation_count = 0
                link_count = 0
                for index, page in enumerate(pdf):
                    annotation_count += sum(1 for _ in (page.annots() or ()))
                    link_count += len(page.get_links())
                    if not page.get_text('text').strip() and profile['require_searchable_text']:
                        issues.append(dict(severity='warning', code='UNSEARCHABLE_PAGE', message=f'{doc["title"]}, page {index + 1} has no extractable text. Consider OCR/review.', **common))
                if annotation_count:
                    issues.append(dict(severity='warning', code='SOURCE_ANNOTATIONS_OMITTED',
                                       message=f'{doc["title"]} has {annotation_count} working-copy annotation(s) that will be omitted from volume and combined PDFs; the imported original is retained.', **common))
                if link_count:
                    issues.append(dict(severity='warning', code='SOURCE_LINKS_OMITTED',
                                       message=f'{doc["title"]} has {link_count} working-copy link(s) that will be omitted from volume and combined PDFs; generated index links remain.', **common))
        except Exception as exc:
            issues.append(dict(severity='error', code='BAD_PDF', message=f'Cannot process {doc["title"]}: {exc}', **common))
    if not any(i['severity'] == 'error' for i in issues):
        issues.append(dict(severity='info', code='TECHNICAL_PREFLIGHT', message='Basic technical preflight completed. Legal completeness, authenticity, signatures, limitation and registry acceptance are not verified.', document_id=None))
    return issues


def index_pages_for(doc_count: int) -> int:
    return max(1, math.ceil(doc_count / INDEX_ROWS_PER_PAGE))


def make_units(docs: list[dict], keep_groups: bool) -> list[list[dict]]:
    units = []
    # A group must occupy a contiguous run. Reject accidental non-contiguous repeats.
    used_group_keys: set[str] = set()
    current = []
    current_key = ''
    for doc in docs:
        key = doc['group_key'].strip() if keep_groups else ''
        if key:
            if key != current_key:
                if current:
                    units.append(current)
                if key in used_group_keys:
                    raise ProcessingError(f'Group "{key}" is not contiguous. Reorder documents so grouped records are adjacent.')
                used_group_keys.add(key)
                current = []
            current.append(doc)
            current_key = key
        else:
            if current:
                units.append(current)
                current = []
            units.append([doc])
            current_key = ''
    if current:
        units.append(current)
    return units


def volume_plan(docs: list[dict], max_pages: int, keep_groups: bool) -> list[list[dict]]:
    volumes = []
    current = []
    page_count = 0
    for unit in make_units(docs, keep_groups):
        unit_pages = sum(d['page_count'] for d in unit)
        next_index_pages = index_pages_for(len(current) + len(unit))
        if page_count + unit_pages + next_index_pages <= max_pages:
            current.extend(unit)
            page_count += unit_pages
            continue
        if current:
            volumes.append(current)
            current = []
            page_count = 0
        if unit_pages + index_pages_for(len(unit)) > max_pages:
            joined = ', '.join(d['title'] for d in unit)
            raise ProcessingError(f'Document/group ({joined}) exceeds {max_pages}-page volume cap including index. No document was split. Increase the cap or revise the grouping policy.')
        current.extend(unit)
        page_count += unit_pages
    if current:
        volumes.append(current)
    return volumes


def clean_title(text: str, limit=64) -> str:
    """Base-14 fonts only support a limited character repertoire in this POC."""
    value = ''.join(c if 32 <= ord(c) <= 126 else '?' for c in str(text or ''))
    return value[:limit - 3] + '...' if len(value) > limit else value


def check_export_sizes(staging: Path, filenames: list[str], max_file_mb: float | None) -> None:
    """Reject any downloadable artifact above the selected technical file cap."""
    if max_file_mb is None:
        return
    limit_bytes = max_file_mb * 1024 * 1024
    sizes = [(name, (staging / name).stat().st_size) for name in filenames]
    oversized = [(name, size) for name, size in sizes if size > limit_bytes]
    if oversized:
        details = ', '.join(f'{name} ({size:,} bytes)' for name, size in oversized)
        raise ProcessingError(
            f'Export exceeds the selected profile {max_file_mb:g} MB per-file technical limit: '
            f'{details}. No new export was published. Reduce the output or choose a reviewed workflow.'
        )


def render_volume(documents: list[dict], matter: dict, path: Path, volume_no: int, cancelled=None):
    """Create an indexed, paginated, bookmarked volume and return page-map records."""
    index_count = index_pages_for(len(documents))
    rows = []
    position = index_count
    for d in documents:
        start = position + 1  # 1-based PDF page including index
        end = position + d['page_count']
        rows.append(dict(id=d['id'], part=d['part'], kind=d['kind'], label=d['label'],
                         title=d['title'], start_page=start, end_page=end, volume=volume_no,
                         total_pages=d['page_count']))
        position = end

    with pymupdf.open() as output:
        for i in range(index_count):
            page = output.new_page(width=A4_W, height=A4_H)
            page.draw_rect(pymupdf.Rect(0, 0, A4_W, 10), color=(0.10, .25, .46), fill=(0.10, .25, .46))
            page.insert_text((50, 50), 'COURTBUNDLE / INDEX', fontsize=10, fontname='hebo', color=(.12,.32,.58))
            page.insert_text((50, 82), f'Volume {volume_no} - Document Index', fontsize=19, fontname='hebo', color=(.08,.16,.29))
            page.insert_textbox(pymupdf.Rect(50, 94, 555, 135), clean_title(matter['name'], 95), fontsize=9)
            page.insert_text((50, 156), 'PART', fontsize=9, fontname='hebo')
            page.insert_text((100, 156), 'DOCUMENT / PARTICULARS', fontsize=9, fontname='hebo')
            page.insert_text((490, 156), 'PAGES', fontsize=9, fontname='hebo')
            page.draw_line((50, 164), (545, 164), color=(.7,.75,.8), width=.8)
            subset = rows[i * INDEX_ROWS_PER_PAGE:(i + 1) * INDEX_ROWS_PER_PAGE]
            for offset, row in enumerate(subset):
                ypos = 187 + offset * 25
                page.insert_text((50, ypos), str(row['part']), fontsize=9)
                display_title = (row['label'] + '  ' if row['label'] else '') + row['title']
                page.insert_text((100, ypos), clean_title(display_title, 66), fontsize=9)
                start_end = str(row['start_page']) if row['start_page'] == row['end_page'] else f'{row["start_page"]}-{row["end_page"]}'
                page.insert_text((488, ypos), start_end, fontsize=9)
                # The target pages do not exist yet; insert links after adding all documents.
                if offset % 2 == 0:
                    page.draw_line((50, ypos+8), (545, ypos+8), color=(.92,.93,.95), width=.5)
            page.insert_text((50, 774), 'Index entries open their source pages. Verify legal completeness separately.', fontsize=8, color=(.45,.48,.52))
            page.insert_text((50, 796), 'DRAFT / POC - NOT COURT CERTIFIED', fontsize=8, color=(.55,.32,.24))

        toc = [[1, f'Volume {volume_no}: Index', 1]]
        current_part = None
        for row, d in zip(rows, documents):
            if cancelled and cancelled():
                raise ProcessingError('Operation cancelled')
            with pymupdf.open(d['_working_path']) as source:
                if source.page_count != d['page_count']:
                    raise ProcessingError(f'Page count changed for {d["title"]} while building. Retry.')
                # Source annotations/links are omitted by policy; generated index links
                # are inserted from the deterministic page map after document insertion.
                for first in range(0, source.page_count, 10):
                    if cancelled and cancelled():
                        raise ProcessingError('Operation cancelled')
                    output.insert_pdf(source, from_page=first,
                                      to_page=min(first + 9, source.page_count - 1),
                                      annots=False, links=False)
            if row['part'] != current_part:
                current_part = row['part']
                toc.append([1, f'Part {current_part}', row['start_page']])
            title = (row['label'] + ' - ' if row['label'] else '') + row['title']
            toc.append([2, clean_title(title, 130), row['start_page']])

        # Stamp consistent 1-based physical page number at the foot of each volume page.
        for i in range(output.page_count):
            if cancelled and cancelled():
                raise ProcessingError('Operation cancelled')
            page = output[i]
            x = page.rect.width / 2 - 16
            y = page.rect.height - 24
            if y > 40:
                page.insert_text((x, y), str(i + 1), fontsize=9, color=(.24,.28,.33))

        # Link each index row to its true 0-based destination.
        for i in range(index_count):
            page = output[i]
            subset = rows[i * INDEX_ROWS_PER_PAGE:(i + 1) * INDEX_ROWS_PER_PAGE]
            for offset, row in enumerate(subset):
                ypos = 187 + offset * 25
                page.insert_link({'kind': pymupdf.LINK_GOTO,
                                  'from': pymupdf.Rect(90, ypos - 14, 545, ypos + 8),
                                  'page': row['start_page'] - 1, 'to': pymupdf.Point(0, 0)})
        output.set_toc(toc)
        output.set_metadata({'title': f'{matter["name"]} - Volume {volume_no}',
                             'subject': 'DRAFT CourtBundle POC - not court certified',
                             'creator': 'CourtBundle POC'})
        output.save(path, garbage=4, deflate=True)

    with pymupdf.open(path) as check:
        if check.page_count > matter['max_pages_per_volume']:
            raise ProcessingError('Internal error: resulting volume exceeded page cap')
        if len(check.get_toc()) < 2 or sum(len(check[i].get_links()) for i in range(index_count)) != len(rows):
            raise ProcessingError('Internal index/bookmark validation failed')
    return rows, index_count


def build_bundle(store: Store, matter_id: str, export_id: str, progress, cancelled) -> dict:
    matter = store.get_matter(matter_id)
    if not matter:
        raise ProcessingError('Matter not found')
    profiles = load_profiles()
    profile = profiles[matter['court_profile']]
    documents = store.get_documents(matter_id)
    progress(5, 'Checking source documents')
    problems = preflight(store, matter_id, profile)
    if any(x['severity'] == 'error' for x in problems):
        raise ProcessingError('Preflight failed: ' + '; '.join(x['message'] for x in problems if x['severity']=='error'))
    input_revisions = [{'document_id': d['id'], 'revision': d['revision'],
                        'working_sha256': store.sha256(store.working_path(d))}
                       for d in documents]
    docs = [dict(d, _working_path=str(store.working_path(d))) for d in documents]
    plans = volume_plan(docs, matter['max_pages_per_volume'], matter['keep_groups_intact'])
    if cancelled():
        raise ProcessingError('Operation cancelled')
    export_root = store.exports_root(matter_id)
    export_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.build-', dir=export_root) as temp:
        staging = Path(temp)
        volume_rows = []
        all_rows = []
        for vno, group in enumerate(plans, start=1):
            if cancelled():
                raise ProcessingError('Operation cancelled')
            progress(10 + int((vno - 1) / len(plans) * 55), f'Building volume {vno} of {len(plans)}')
            filename = f'volume-{vno:02}.pdf'
            rows, index_count = render_volume(group, matter, staging / filename, vno, cancelled)
            volume_rows.append(dict(filename=filename, rows=rows, index_count=index_count))
            all_rows.extend(rows)

        progress(70, 'Creating combined bookmarked PDF')
        combined_file = staging / 'combined.pdf'
        with pymupdf.open() as combined:
            global_toc = []
            offset = 0
            for vno, volume in enumerate(volume_rows, start=1):
                with pymupdf.open(staging / volume['filename']) as part:
                    count = part.page_count
                    # Re-create internal index hyperlinks after concatenation for reliable targets.
                    for first in range(0, count, 10):
                        if cancelled():
                            raise ProcessingError('Operation cancelled')
                        combined.insert_pdf(part, from_page=first,
                                            to_page=min(first + 9, count - 1),
                                            links=False, annots=False)
                global_toc.append([1, f'Volume {vno}', offset + 1])
                current_part = None
                for rno, row in enumerate(volume['rows']):
                    destination = offset + row['start_page'] - 1
                    if current_part != row['part']:
                        current_part = row['part']
                        global_toc.append([2, f'Part {current_part}', destination + 1])
                    global_toc.append([3, clean_title((row['label'] + ' - ' if row['label'] else '') + row['title'], 130), destination + 1])
                    idx_page = offset + rno // INDEX_ROWS_PER_PAGE
                    ypos = 187 + (rno % INDEX_ROWS_PER_PAGE) * 25
                    combined[idx_page].insert_link({'kind': pymupdf.LINK_GOTO,
                        'from': pymupdf.Rect(90, ypos-14, 545, ypos+8),
                        'page': destination, 'to': pymupdf.Point(0, 0)})
                offset += count
            combined.set_toc(global_toc)
            combined.set_metadata({'title': f'{matter["name"]} - Combined Filing Bundle',
                                   'subject': 'DRAFT CourtBundle POC - not court certified',
                                   'creator': 'CourtBundle POC'})
            combined.save(combined_file, garbage=4, deflate=True)

        progress(83, 'Checking bookmarks and generating index CSV')
        with pymupdf.open(combined_file) as final:
            if not final.get_toc() or len(final.get_toc()) < len(all_rows):
                raise ProcessingError('Combined bookmarks validation failed')
            if sum(len(final[i].get_links()) for i in range(final.page_count)) < len(all_rows):
                raise ProcessingError('Combined index links validation failed')
            combined_pages = final.page_count

        csvpath = staging / 'index.csv'
        with csvpath.open('w', newline='', encoding='utf-8-sig') as stream:
            writer = csv.DictWriter(stream, fieldnames=['volume','part','kind','label','title','start_page','end_page','total_pages'])
            writer.writeheader()
            writer.writerows({key:r[key] for key in writer.fieldnames} for r in all_rows)
        file_bytes = combined_file.stat().st_size
        if any(any(ord(c)>126 for c in row['title']+row['label']) for row in all_rows):
            problems.append(dict(severity='warning', code='NON_LATIN_INDEX', document_id=None,
                                 message='Index/outline currently use built-in Latin PDF fonts; non-Latin title glyphs may be replaced by ?.'))
        problems.append(dict(severity='info', code='NOT_A_FILING_APPROVAL', document_id=None,
                             message='Technical checks passed or reported. Filing acceptance, legal validity and registry objections need human review.'))
        report = {'matter_id': matter_id, 'export_id': export_id, 'profile': profile['id'],
                  'profile_status': profile['status'], 'created_at': utcnow(),
                  'matter_updated_at': matter['updated_at'], 'input_revisions': input_revisions,
                  'interactive_content_policy': EXPORT_INTERACTIVE_CONTENT_POLICY,
                  'total_input_documents': len(documents), 'volume_count': len(plans),
                  'combined_pages': combined_pages, 'combined_bytes': file_bytes,
                  'checks': problems, 'volumes': [{'name':v['filename'],'items':len(v['rows'])} for v in volume_rows]}
        (staging / 'validation-report.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
        readme = ('CourtBundle POC - DRAFT EXPORT\n\n'
                  'This package is NOT court-certified and is NOT proof of filing.\n'
                  'The selected rules are illustrative technical checks only.\n'
                  'Original evidence and signed files must be reviewed independently.\n'
                  'Working-copy PDF annotation objects and link actions are omitted from volume and combined PDFs.\n'
                  'Flattened marks already in page content remain visible.\n'
                  'Generated index links and bookmarks are retained. See validation-report.json for affected documents.\n'
                  'Every PDF page number in an index is the 1-based page position WITHIN that volume.\n'
                  'combined.pdf concatenates all generated volumes, including their separate indexes.\n')
        (staging / 'READ_ME_FIRST.txt').write_text(readme, encoding='utf-8')
        progress(90, 'Packaging export ZIP')
        zip_file = staging / 'filing-package.zip'
        with zipfile.ZipFile(zip_file, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=6) as z:
            for filename in ['combined.pdf', *[v['filename'] for v in volume_rows], 'index.csv', 'validation-report.json', 'READ_ME_FIRST.txt']:
                z.write(staging / filename, arcname=filename)
        filenames = ['filing-package.zip', 'combined.pdf', *[v['filename'] for v in volume_rows], 'index.csv', 'validation-report.json']
        # The ZIP may exceed the cap even when each PDF fits. Check everything offered
        # for download before the staging directory can be published as an export.
        check_export_sizes(staging, filenames, profile.get('max_file_mb'))
        export_manifest = dict(id=export_id, created_at=report['created_at'], matter_id=matter_id,
                               pages=combined_pages, volumes=len(plans),
                               matter_updated_at=matter['updated_at'], input_revisions=input_revisions,
                               interactive_content_policy=EXPORT_INTERACTIVE_CONTENT_POLICY,
                               files=[{'name':name,'bytes':(staging/name).stat().st_size} for name in filenames],
                               checks=problems)
        (staging / 'export_manifest.json').write_text(json.dumps(export_manifest, indent=2), encoding='utf-8')
        if cancelled():
            raise ProcessingError('Operation cancelled')
        source_errors = original_integrity_issues(store, documents)
        if source_errors:
            raise ProcessingError('Source integrity check failed before export: ' +
                                  '; '.join(issue['message'] for issue in source_errors))
        final_dir = export_root / export_id
        if final_dir.exists():
            raise ProcessingError('Export id collision')
        # Atomic publish on the same filesystem; staged files vanish on failures.
        os.replace(staging, final_dir)
    progress(100, 'Bundle ready to download')
    return export_manifest
