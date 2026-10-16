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
from datetime import datetime, timezone
from pathlib import Path
import shutil
import tempfile
import zipfile

import pymupdf

from .pdf_ops import (ProcessingError, validate_pdf, apply_output_marks,
                      validate_overlay_image)
from .file_io import open_pdf, read_bytes, save_pdf, size as file_size, write_bytes
from .storage import Store, utcnow
from .signatures import (CERTIFICATE_SIGNATURE_DETECTED,
                         SIGNATURE_INSPECTION_INCONCLUSIVE,
                         VISIBLE_SIGNATURE_ONLY, risk_summary)

INDEX_ROWS_PER_PAGE = 23
A4_W, A4_H = 595, 842
EXPORT_INTERACTIVE_CONTENT_POLICY = {
    'source_annotations': 'omitted',
    'source_links': 'omitted',
    'courtbundle_image_overlays': 'flattened into source-document pages when revision anchors resolve',
    'courtbundle_highlights_comments': 'private review annotations omitted from filing-preparation PDFs',
    'reviewed_user_links': 'retained when both revision-anchored endpoints resolve',
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
        signature_status = doc.get('signature_status', SIGNATURE_INSPECTION_INCONCLUSIVE)
        if signature_status == CERTIFICATE_SIGNATURE_DETECTED:
            issues.append(dict(severity='warning', code='CERTIFICATE_SIGNATURE_INDICATORS',
                               message=(f'{doc["title"]} contains PDF certificate-signature indicators. '
                                        'Validity and current byte coverage were not checked; edits, OCR and '
                                        'bundle generation may invalidate or omit signature protections. '
                                        'The imported original is retained.'), **common))
        elif signature_status == SIGNATURE_INSPECTION_INCONCLUSIVE:
            issues.append(dict(severity='warning', code='SIGNATURE_INSPECTION_INCONCLUSIVE',
                               message=(f'Signature inspection for {doc["title"]} was inconclusive. '
                                        'No cryptographic validation was attempted; acknowledge the risk '
                                        'before transforming it.'), **common))
        elif signature_status == VISIBLE_SIGNATURE_ONLY:
            issues.append(dict(severity='warning', code='VISIBLE_SIGNATURE_ONLY',
                               message=(f'{doc["title"]} was marked by a reviewer as containing a visible '
                                        'or scanned signature, but no PDF certificate-signature indicators '
                                        'were detected. The signature and signer are not verified.'), **common))
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
            with open_pdf(working) as pdf:
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
    annotations = store.list_page_annotations(matter_id)
    private_count = sum(item['kind'] in ('highlight', 'comment') for item in annotations)
    unresolved_images = sum(item['kind'] == 'image' and not item['resolved'] for item in annotations)
    resolved_images = sum(item['kind'] == 'image' and item['resolved'] for item in annotations)
    for item in annotations:
        if item['kind'] != 'image':
            continue
        path = store.matter_dir(matter_id) / item['asset_relpath']
        try:
            if not path.is_file() or path.is_symlink():
                raise ProcessingError('asset file is missing or unsafe')
            validate_overlay_image(
                read_bytes(path), 'png' if item['asset_mime'] == 'image/png' else 'jpeg')
        except (OSError, ProcessingError) as exc:
            issues.append(dict(severity='error', code='BAD_IMAGE_OVERLAY_ASSET',
                               message=f'Image overlay {item["id"]} cannot be used: {exc}',
                               document_id=item['document_id']))
    if resolved_images:
        issues.append(dict(severity='info', code='IMAGE_OVERLAYS_FLATTENED',
                           message=(f'{resolved_images} resolved image overlay(s) will be flattened '
                                    'into newly generated source-document pages.'), document_id=None))
    if private_count:
        issues.append(dict(severity='info', code='PRIVATE_REVIEW_ANNOTATIONS_OMITTED',
                           message=(f'{private_count} CourtBundle highlight/comment review annotation(s) '
                                    'will be omitted from filing-preparation PDFs. Image overlays use a '
                                    'separate explicit flattened-content policy.'), document_id=None))
    if unresolved_images:
        issues.append(dict(severity='warning', code='UNRESOLVED_IMAGE_OVERLAYS',
                           message=(f'{unresolved_images} image overlay(s) target an older revision, '
                                    'removed document or missing page and will be omitted until reviewed.'),
                           document_id=None))
    if not any(i['severity'] == 'error' for i in issues):
        issues.append(dict(severity='info', code='TECHNICAL_PREFLIGHT', message='Basic technical preflight completed. Legal completeness, authenticity, signatures, limitation and registry acceptance are not verified.', document_id=None))
    return issues


def index_pages_for(doc_count: int) -> int:
    return max(1, math.ceil(doc_count / INDEX_ROWS_PER_PAGE))


def volume_rows(documents: list[dict], volume_no: int) -> tuple[list[dict], int]:
    """The single page map used by preview, PDF rendering, links and CSV."""
    index_count = index_pages_for(len(documents))
    position = index_count
    rows = []
    for doc in documents:
        start = position + 1
        end = position + doc['page_count']
        rows.append(dict(id=doc['id'], part=doc['part'], kind=doc['kind'],
                         label=doc['label'], title=doc['title'], start_page=start,
                         end_page=end, volume=volume_no, total_pages=doc['page_count']))
        position = end
    return rows, index_count


def bundle_preview(store: Store, matter_id: str, max_pages: int,
                   keep_groups: bool) -> dict:
    """Read-only view of the exact current volume/index/page-number plan."""
    documents = store.get_documents(matter_id)
    bookmarks = store.list_manual_bookmarks(matter_id)
    document_titles = {document['id']: document['title'] for document in documents}
    plans = volume_plan(documents, max_pages, keep_groups) if documents else []
    volumes = []
    destinations = {}
    combined_offset = 0
    for number, group in enumerate(plans, start=1):
        rows, index_count = volume_rows(group, number)
        page_count = index_count + sum(doc['page_count'] for doc in group)
        for row in rows:
            destinations[row['id']] = (number, row['start_page'], combined_offset)
            row['combined_start_page'] = combined_offset + row['start_page']
            row['combined_end_page'] = combined_offset + row['end_page']
        volumes.append(dict(number=number, index_pages=index_count,
                            page_count=page_count, combined_start_page=combined_offset + 1,
                            combined_end_page=combined_offset + page_count,
                            rows=rows))
        combined_offset += page_count
    manual = []
    for item in bookmarks:
        destination = destinations.get(item['document_id'])
        resolved = bool(item['resolved'] and destination)
        manual.append(dict(id=item['id'], title=item['title'], resolved=resolved,
                           document_title=document_titles.get(item['document_id'], 'Archived document'),
                           document_page=item['page_index'] + 1,
                           volume=destination[0] if resolved else None,
                           volume_page=destination[1] + item['page_index'] if resolved else None,
                           combined_page=destination[2] + destination[1] + item['page_index'] if resolved else None))
    return dict(volumes=volumes, combined_pages=combined_offset, manual_bookmarks=manual,
                numbering='Physical page numbers restart at 1 in each volume; the combined PDF retains those printed numbers.')


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
    sizes = [(name, file_size(staging / name)) for name in filenames]
    oversized = [(name, size) for name, size in sizes if size > limit_bytes]
    if oversized:
        details = ', '.join(f'{name} ({size:,} bytes)' for name, size in oversized)
        raise ProcessingError(
            f'Export exceeds the selected profile {max_file_mb:g} MB per-file technical limit: '
            f'{details}. No new export was published. Reduce the output or choose a reviewed workflow.'
        )


def manual_toc(bookmarks: list[dict], destinations: dict[str, int]) -> list[list]:
    """Resolve manual targets against the same document page map as index rows."""
    resolved_levels: dict[str, int] = {}
    entries = []
    for bookmark in bookmarks:
        start = destinations.get(bookmark['document_id'])
        if not bookmark['resolved'] or start is None:
            continue
        parent_level = resolved_levels.get(bookmark['parent_id'], 1)
        level = parent_level + 1
        resolved_levels[bookmark['id']] = level
        entries.append([level, bookmark['title'],
                        start + bookmark['page_index']])
    return [[1, 'Manual bookmarks', entries[0][2]], *entries] if entries else []


def validate_navigation(path: Path, pages: int, toc: list, links: dict[int, list[int]]) -> None:
    """Check actual serialized destinations, not merely outline/link counts."""
    validate_pdf(path, pages)
    with open_pdf(path) as pdf:
        if pdf.get_toc() != toc:
            raise ProcessingError('Written PDF outline differs from the deterministic page map')
        for index, page in enumerate(pdf):
            actual = [link.get('page') for link in page.get_links()
                      if link['kind'] == pymupdf.LINK_GOTO]
            if actual != links.get(index, []):
                raise ProcessingError('Written PDF index destinations differ from the deterministic page map')


def _reviewed_link_rect(page: pymupdf.Page, link: dict) -> pymupdf.Rect:
    displayed = pymupdf.Rect(
        page.rect.x0 + page.rect.width * link['rect_x'],
        page.rect.y0 + page.rect.height * link['rect_y'],
        page.rect.x0 + page.rect.width * (link['rect_x'] + link['rect_width']),
        page.rect.y0 + page.rect.height * (link['rect_y'] + link['rect_height']))
    return displayed * page.derotation_matrix


def insert_reviewed_links(output: pymupdf.Document, reviewed_links: list[dict],
                          document_starts: dict[str, int]) -> dict:
    """Resolve only app-reviewed actions; imported PDF actions remain omitted."""
    expected_goto: dict[int, list[int]] = {}
    expected_uri: dict[int, list[str]] = {}
    retained = omitted_cross_output = unresolved = 0
    for link in reviewed_links:
        source_start = document_starts.get(link['source_document_id'])
        if not link.get('resolved'):
            unresolved += 1
            continue
        if source_start is None:
            continue
        source_page_index = source_start + link['source_page_index']
        if not 0 <= source_page_index < output.page_count:
            raise ProcessingError('Reviewed link source differs from the deterministic page map')
        page = output[source_page_index]
        action = {'from': _reviewed_link_rect(page, link)}
        if link['kind'] == 'external':
            action.update(kind=pymupdf.LINK_URI, uri=link['uri'])
            expected_uri.setdefault(source_page_index, []).append(link['uri'])
        else:
            target_start = document_starts.get(link['target_document_id'])
            if target_start is None:
                omitted_cross_output += 1
                continue
            destination = target_start + link['target_page_index']
            action.update(kind=pymupdf.LINK_GOTO, page=destination,
                          to=pymupdf.Point(0, 0))
            expected_goto.setdefault(source_page_index, []).append(destination)
        page.insert_link(action)
        retained += 1
    return {'retained': retained, 'omitted_cross_output': omitted_cross_output,
            'unresolved': unresolved, 'expected_goto': expected_goto,
            'expected_uri': expected_uri}


def validate_reviewed_links(path: Path, expected_uri: dict[int, list[str]]) -> None:
    with open_pdf(path) as pdf:
        for index, page in enumerate(pdf):
            actual = [link.get('uri') for link in page.get_links()
                      if link['kind'] == pymupdf.LINK_URI]
            if actual != expected_uri.get(index, []):
                raise ProcessingError('Written PDF external links differ from reviewed link inputs')


def apply_page_annotations(output: pymupdf.Document, annotations: list[dict],
                           document_starts: dict[str, int], cancelled=None) -> dict:
    """Flatten resolved image overlays; private review annotations stay out."""
    inserted = unresolved_images = private_omitted = 0
    for item in annotations:
        if cancelled and cancelled():
            raise ProcessingError('Operation cancelled')
        start = document_starts.get(item['document_id'])
        if start is None:
            continue
        if item['kind'] in ('highlight', 'comment'):
            private_omitted += 1
            continue
        if item['kind'] != 'image':
            raise ProcessingError('Unknown CourtBundle page annotation kind')
        if not item.get('resolved'):
            unresolved_images += 1
            continue
        page_index = start + item['page_index']
        if not 0 <= page_index < output.page_count:
            raise ProcessingError('Image overlay target differs from the deterministic page map')
        page = output[page_index]
        displayed = pymupdf.Rect(
            page.rect.x0 + page.rect.width * item['rect_x'],
            page.rect.y0 + page.rect.height * item['rect_y'],
            page.rect.x0 + page.rect.width * (item['rect_x'] + item['rect_width']),
            page.rect.y0 + page.rect.height * (item['rect_y'] + item['rect_height']))
        page.insert_image(displayed * page.derotation_matrix,
                          stream=read_bytes(Path(item['_asset_path'])),
                          keep_proportion=False, overlay=True)
        inserted += 1
    return {'image_overlays_inserted': inserted,
            'unresolved_image_overlays': unresolved_images,
            'private_review_annotations_omitted': private_omitted}


def render_volume(documents: list[dict], matter: dict, path: Path, volume_no: int,
                  cancelled=None, bookmarks: list[dict] | None = None,
                  reviewed_links: list[dict] | None = None,
                  page_annotations: list[dict] | None = None,
                  output_marks: dict | None = None,
                  mark_at: datetime | str | None = None):
    """Create an indexed, paginated, bookmarked volume and return page-map records."""
    rows, index_count = volume_rows(documents, volume_no)

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
            with open_pdf(d['_working_path']) as source:
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
            toc.append([2, title, row['start_page']])

        document_starts = {row['id']: row['start_page'] - 1 for row in rows}
        annotation_summary = apply_page_annotations(
            output, page_annotations or [], document_starts, cancelled)

        # Stamp consistent 1-based physical page number at the foot of each volume page.
        for i in range(output.page_count):
            if cancelled and cancelled():
                raise ProcessingError('Operation cancelled')
            page = output[i]
            y = page.rect.height - 24
            if y > 40:
                page.insert_textbox(pymupdf.Rect(24, y - 11, page.rect.width - 24, y + 5),
                                    str(i + 1), fontsize=9, align=pymupdf.TEXT_ALIGN_CENTER,
                                    color=(.24,.28,.33))

        mark_count = mark_overlap_count = 0
        if output_marks and (output_marks.get('watermark_enabled') or
                             output_marks.get('timestamp_enabled')):
            for row, document in zip(rows, documents):
                for source_page in range(document['page_count']):
                    if cancelled and cancelled():
                        raise ProcessingError('Operation cancelled')
                    results = apply_output_marks(
                        output[row['start_page'] - 1 + source_page], output_marks, mark_at)
                    mark_count += len(results)
                    mark_overlap_count += sum(bool(item['overlaps_content']) for item in results)

        # Link each index row to its true 0-based destination.
        for i in range(index_count):
            page = output[i]
            subset = rows[i * INDEX_ROWS_PER_PAGE:(i + 1) * INDEX_ROWS_PER_PAGE]
            for offset, row in enumerate(subset):
                ypos = 187 + offset * 25
                page.insert_link({'kind': pymupdf.LINK_GOTO,
                                  'from': pymupdf.Rect(90, ypos - 14, 545, ypos + 8),
                                  'page': row['start_page'] - 1, 'to': pymupdf.Point(0, 0)})
        reviewed_summary = insert_reviewed_links(
            output, reviewed_links or [], document_starts)
        toc.extend(manual_toc(bookmarks or [], {row['id']: row['start_page'] for row in rows}))
        output.set_toc(toc)
        output.set_metadata({'title': f'{matter["name"]} - Volume {volume_no}',
                             'subject': 'DRAFT CourtBundle POC - not court certified',
                             'creator': 'CourtBundle POC'})
        save_pdf(output, path, garbage=4, deflate=True)

    expected_links = {}
    for index, row in enumerate(rows):
        expected_links.setdefault(index // INDEX_ROWS_PER_PAGE, []).append(row['start_page'] - 1)
    for page_index, destinations in reviewed_summary['expected_goto'].items():
        expected_links.setdefault(page_index, []).extend(destinations)
    validate_navigation(path, index_count + sum(d['page_count'] for d in documents), toc, expected_links)
    validate_reviewed_links(path, reviewed_summary['expected_uri'])

    with open_pdf(path) as check:
        if check.page_count > matter['max_pages_per_volume']:
            raise ProcessingError('Internal error: resulting volume exceeded page cap')
        if len(check.get_toc()) < 2 or sum(len(check[i].get_links()) for i in range(index_count)) != len(rows):
            raise ProcessingError('Internal index/bookmark validation failed')
    return rows, index_count, {
        'reviewed_links': {key: reviewed_summary[key] for key in
                           ('retained', 'omitted_cross_output', 'unresolved')},
        'page_annotations': annotation_summary,
        'marks': {'inserted': mark_count, 'overlaps_content': mark_overlap_count},
    }


def build_bundle(store: Store, matter_id: str, export_id: str, progress, cancelled,
                 signature_acknowledged: bool = False) -> dict:
    matter = store.get_matter(matter_id)
    if not matter:
        raise ProcessingError('Matter not found')
    profiles = load_profiles()
    profile = profiles[matter['court_profile']]
    documents = store.get_documents(matter_id)
    signature_risks = risk_summary(documents)
    if signature_risks and not signature_acknowledged:
        raise ProcessingError('Signature-risk acknowledgment is required before building transformed bundle PDFs')
    bookmarks = store.list_manual_bookmarks(matter_id)
    reviewed_links = store.list_reviewed_links(matter_id)
    page_annotations = store.list_page_annotations(matter_id)
    for item in page_annotations:
        if item['kind'] == 'image' and item['asset_relpath']:
            item['_asset_path'] = str(store.matter_dir(matter_id) / item['asset_relpath'])
    output_marks = store.get_output_marks(matter_id)
    mark_at = datetime.now(timezone.utc)
    mark_at_iso = mark_at.isoformat(timespec='seconds')
    progress(5, 'Checking source documents')
    problems = preflight(store, matter_id, profile)
    if any(x['severity'] == 'error' for x in problems):
        raise ProcessingError('Preflight failed: ' + '; '.join(x['message'] for x in problems if x['severity']=='error'))
    build_input_digest = store.build_input_digest(matter_id)
    build_settings = {'max_pages_per_volume': matter['max_pages_per_volume'],
                      'keep_groups_intact': matter['keep_groups_intact'],
                      'court_profile': matter['court_profile']}
    bookmark_inputs = [{key: item[key] for key in ('id', 'title', 'document_id',
                        'page_index', 'target_revision', 'parent_id', 'position')}
                       for item in bookmarks]
    reviewed_link_inputs = [{key: item[key] for key in ('id', 'title',
                            'source_document_id', 'source_page_index', 'source_revision',
                            'rect_x', 'rect_y', 'rect_width', 'rect_height', 'kind',
                            'target_document_id', 'target_page_index', 'target_revision',
                            'uri', 'position')} | {'resolved': item['resolved']}
                            for item in reviewed_links]
    # Export reports must prove which annotations influenced a build without
    # copying private review text or internal asset paths into filing artifacts.
    annotation_inputs = [{key: item[key] for key in (
        'id', 'document_id', 'page_index', 'target_revision', 'kind',
        'rect_x', 'rect_y', 'rect_width', 'rect_height', 'color', 'position')}
        | {'resolved': item['resolved'],
           'body_present': bool(item['body']),
           'asset_sha256': (store.sha256(Path(item['_asset_path']))
                            if item['kind'] == 'image' and item.get('_asset_path')
                            and Path(item['_asset_path']).is_file() else None)}
        for item in page_annotations]
    input_revisions = [{'document_id': d['id'], 'revision': d['revision'],
                        'working_sha256': store.sha256(store.working_path(d))}
                       for d in documents]
    label_inputs = [{'document_id': d['id'], 'position': d['position'],
                     'label': d['label'],
                     'label_status': d.get('label_status', 'unreviewed'),
                     'label_source': d.get('label_source', 'legacy' if d['label'] else 'none'),
                     'group_key': d['group_key'],
                     'group_role': d.get('group_role', 'member')}
                    for d in documents]
    signature_inputs = [{'document_id': d['id'], 'signature_status': d['signature_status'],
                         'certificate_signature_status': d['certificate_signature_status'],
                         'certificate_signature_count': d['certificate_signature_count'],
                         'visible_signature_status': d['visible_signature_status']}
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
            rows, index_count, content_summary = render_volume(
                group, matter, staging / filename, vno, cancelled, bookmarks,
                reviewed_links, page_annotations, output_marks, mark_at)
            volume_rows.append(dict(filename=filename, rows=rows, index_count=index_count,
                                    content_summary=content_summary))
            all_rows.extend(rows)

        progress(70, 'Creating combined bookmarked PDF')
        combined_file = staging / 'combined.pdf'
        with pymupdf.open() as combined:
            global_toc = []
            global_links = {}
            combined_destinations = {}
            combined_document_starts = {}
            offset = 0
            for vno, volume in enumerate(volume_rows, start=1):
                with open_pdf(staging / volume['filename']) as part:
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
                    combined_destinations[row['id']] = destination + 1
                    combined_document_starts[row['id']] = destination
                    if current_part != row['part']:
                        current_part = row['part']
                        global_toc.append([2, f'Part {current_part}', destination + 1])
                    global_toc.append([3, (row['label'] + ' - ' if row['label'] else '') + row['title'], destination + 1])
                    idx_page = offset + rno // INDEX_ROWS_PER_PAGE
                    global_links.setdefault(idx_page, []).append(destination)
                    ypos = 187 + (rno % INDEX_ROWS_PER_PAGE) * 25
                    combined[idx_page].insert_link({'kind': pymupdf.LINK_GOTO,
                        'from': pymupdf.Rect(90, ypos-14, 545, ypos+8),
                        'page': destination, 'to': pymupdf.Point(0, 0)})
                offset += count
            combined_link_summary = insert_reviewed_links(
                combined, reviewed_links, combined_document_starts)
            for page_index, destinations in combined_link_summary['expected_goto'].items():
                global_links.setdefault(page_index, []).extend(destinations)
            global_toc.extend(manual_toc(bookmarks, combined_destinations))
            combined.set_toc(global_toc)
            combined.set_metadata({'title': f'{matter["name"]} - Combined Filing Bundle',
                                   'subject': 'DRAFT CourtBundle POC - not court certified',
                                   'creator': 'CourtBundle POC'})
            save_pdf(combined, combined_file, garbage=4, deflate=True)

        validate_navigation(combined_file, offset, global_toc, global_links)
        validate_reviewed_links(combined_file, combined_link_summary['expected_uri'])

        progress(83, 'Checking bookmarks and generating index CSV')
        with open_pdf(combined_file) as final:
            if not final.get_toc() or len(final.get_toc()) < len(all_rows):
                raise ProcessingError('Combined bookmarks validation failed')
            if sum(len(final[i].get_links()) for i in range(final.page_count)) < len(all_rows):
                raise ProcessingError('Combined index links validation failed')
            combined_pages = final.page_count

        csvpath = staging / 'index.csv'
        csv_stream = io.StringIO(newline='')
        writer = csv.DictWriter(csv_stream, fieldnames=['volume','part','kind','label','title','start_page','end_page','total_pages'])
        writer.writeheader()
        writer.writerows({key:r[key] for key in writer.fieldnames} for r in all_rows)
        write_bytes(csvpath, csv_stream.getvalue().encode('utf-8-sig'))
        file_bytes = file_size(combined_file)
        if any(any(ord(c)>126 for c in row['title']+row['label']) for row in all_rows):
            problems.append(dict(severity='warning', code='NON_LATIN_INDEX', document_id=None,
                                 message='Printed index uses built-in Latin PDF fonts; non-Latin title glyphs may be replaced by ?. PDF outlines retain Unicode titles.'))
        unresolved = [bookmark for bookmark in bookmarks if not bookmark['resolved']]
        if unresolved:
            problems.append(dict(severity='warning', code='UNRESOLVED_MANUAL_BOOKMARKS',
                                 document_id=None,
                                 message=f'{len(unresolved)} manual bookmark(s) target an older revision, removed document, or missing page and were omitted from this export. Resolve them before use.'))
        unresolved_links = [link for link in reviewed_links if not link['resolved']]
        if unresolved_links:
            problems.append(dict(severity='warning', code='UNRESOLVED_REVIEWED_LINKS',
                                 document_id=None,
                                 message=f'{len(unresolved_links)} reviewed link(s) target an older revision, removed document, or missing page and were omitted from this export. Resolve them before use.'))
        cross_volume_omissions = sum(
            volume['content_summary']['reviewed_links']['omitted_cross_output']
            for volume in volume_rows)
        if cross_volume_omissions:
            problems.append(dict(severity='warning', code='CROSS_VOLUME_LINKS_COMBINED_ONLY',
                                 document_id=None,
                                 message=f'{cross_volume_omissions} reviewed internal link occurrence(s) crossed a volume boundary. They are active in combined.pdf and omitted from standalone volumes to avoid broken destinations.'))
        unresolved_images = [item for item in page_annotations
                             if item['kind'] == 'image' and not item['resolved']]
        private_annotations = [item for item in page_annotations
                               if item['kind'] in ('highlight', 'comment')]
        mark_overlap_count = sum(
            volume['content_summary']['marks']['overlaps_content'] for volume in volume_rows)
        if mark_overlap_count:
            problems.append(dict(severity='warning', code='OUTPUT_MARK_OVERLAP',
                                 document_id=None,
                                 message=f'{mark_overlap_count} output mark placement(s) intersect detectable text, image, drawing, link or generated page-number content. Review every marked page; signatures and seals are not reliably detectable.'))
        problems.append(dict(severity='info', code='NOT_A_FILING_APPROVAL', document_id=None,
                             message='Technical checks passed or reported. Filing acceptance, legal validity and registry objections need human review.'))
        report = {'matter_id': matter_id, 'export_id': export_id, 'profile': profile['id'],
                  'profile_status': profile['status'], 'created_at': utcnow(),
                  'matter_updated_at': matter['updated_at'], 'input_revisions': input_revisions,
                  'label_inputs': label_inputs,
                  'signature_inputs': signature_inputs,
                  'signature_risk_acknowledged': bool(signature_risks and signature_acknowledged),
                  'build_input_digest': build_input_digest,
                  'build_settings': build_settings, 'manual_bookmark_inputs': bookmark_inputs,
                  'reviewed_link_inputs': reviewed_link_inputs,
                  'page_annotation_inputs': annotation_inputs,
                  'output_marks': output_marks, 'output_mark_timestamp': mark_at_iso,
                  'reviewed_link_summary': {
                      'combined_retained': combined_link_summary['retained'],
                      'standalone_cross_volume_omissions': cross_volume_omissions,
                      'unresolved': len(unresolved_links)},
                  'output_mark_summary': {'overlaps_content': mark_overlap_count,
                      'inserted': sum(volume['content_summary']['marks']['inserted']
                                      for volume in volume_rows)},
                  'page_annotation_summary': {
                      'image_overlays_inserted': sum(volume['content_summary']['page_annotations']['image_overlays_inserted'] for volume in volume_rows),
                      'unresolved_image_overlays': len(unresolved_images),
                      'private_review_annotations_omitted': len(private_annotations)},
                  'interactive_content_policy': EXPORT_INTERACTIVE_CONTENT_POLICY,
                  'total_input_documents': len(documents), 'volume_count': len(plans),
                  'combined_pages': combined_pages, 'combined_bytes': file_bytes,
                  'checks': problems, 'volumes': [{'name':v['filename'],'items':len(v['rows'])} for v in volume_rows]}
        write_bytes(staging / 'validation-report.json', json.dumps(report, indent=2).encode('utf-8'))
        readme = ('CourtBundle POC - DRAFT EXPORT\n\n'
                  'This package is NOT court-certified and is NOT proof of filing.\n'
                  'The selected rules are illustrative technical checks only.\n'
                  'Original evidence and signed files must be reviewed independently.\n'
                  'Working-copy PDF annotation objects and link actions are omitted from volume and combined PDFs.\n'
                  'Flattened marks already in page content remain visible.\n'
                  'Resolved CourtBundle image overlays are flattened into new source-document pages. Private highlights and comments are omitted by default.\n'
                  'Generated index links, bookmarks and explicitly reviewed CourtBundle links are retained when their revision-anchored targets resolve.\n'
                  'Cross-volume reviewed links are available only in combined.pdf; configured output marks apply to source document pages, not index pages.\n'
                  'See validation-report.json for omissions, mark settings and detectable overlap warnings.\n'
                  'Every PDF page number in an index is the 1-based page position WITHIN that volume.\n'
                  'combined.pdf concatenates all generated volumes, including their separate indexes.\n')
        write_bytes(staging / 'READ_ME_FIRST.txt', readme.encode('utf-8'))
        progress(90, 'Packaging export ZIP')
        zip_file = staging / 'filing-package.zip'
        zip_stream = io.BytesIO()
        with zipfile.ZipFile(zip_stream, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=6) as z:
            for filename in ['combined.pdf', *[v['filename'] for v in volume_rows], 'index.csv', 'validation-report.json', 'READ_ME_FIRST.txt']:
                if cancelled():
                    raise ProcessingError('Operation cancelled')
                z.writestr(filename, read_bytes(staging / filename))
        write_bytes(zip_file, zip_stream.getvalue())
        with zipfile.ZipFile(io.BytesIO(read_bytes(zip_file))) as archive:
            if archive.testzip() is not None:
                raise ProcessingError('Export ZIP integrity verification failed')
        filenames = ['filing-package.zip', 'combined.pdf', *[v['filename'] for v in volume_rows], 'index.csv', 'validation-report.json']
        # The ZIP may exceed the cap even when each PDF fits. Check everything offered
        # for download before the staging directory can be published as an export.
        check_export_sizes(staging, filenames, profile.get('max_file_mb'))
        export_manifest = dict(id=export_id, created_at=report['created_at'], matter_id=matter_id,
                               pages=combined_pages, volumes=len(plans),
                               matter_updated_at=matter['updated_at'], input_revisions=input_revisions,
                               label_inputs=label_inputs,
                               signature_inputs=signature_inputs,
                               signature_risk_acknowledged=bool(signature_risks and signature_acknowledged),
                               build_input_digest=build_input_digest,
                               build_settings=build_settings, manual_bookmark_inputs=bookmark_inputs,
                               reviewed_link_inputs=reviewed_link_inputs,
                               page_annotation_inputs=annotation_inputs,
                               output_marks=output_marks, output_mark_timestamp=mark_at_iso,
                               interactive_content_policy=EXPORT_INTERACTIVE_CONTENT_POLICY,
                               files=[{'name':name,'bytes':file_size(staging/name),
                                       'sha256':store.sha256(staging/name)} for name in filenames],
                               checks=problems)
        write_bytes(staging / 'export_manifest.json', json.dumps(export_manifest, indent=2).encode('utf-8'))
        if cancelled():
            raise ProcessingError('Operation cancelled')
        source_errors = original_integrity_issues(store, documents)
        if source_errors:
            raise ProcessingError('Source integrity check failed before export: ' +
                                  '; '.join(issue['message'] for issue in source_errors))
        if store.build_input_digest(matter_id) != build_input_digest:
            raise ProcessingError('Build inputs or profile changed during assembly; no export was published')
        final_dir = export_root / export_id
        if final_dir.exists():
            raise ProcessingError('Export id collision')
        for artifact in staging.iterdir():
            with artifact.open('rb') as stream:
                os.fsync(stream.fileno())
        # Atomic publish on the same filesystem; staged files vanish on failures.
        os.replace(staging, final_dir)
    progress(100, 'Bundle ready to download')
    return export_manifest
