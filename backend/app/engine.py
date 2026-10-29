"""Deterministic PDF bundle engine.

This is filing-preparation software, not a court-certified rules engine.
Index positions, bookmark destinations and the emitted CSV share one page map.
"""
from __future__ import annotations
import csv
from html import escape as html_escape
import io
import hashlib
import json
import math
import os
import re
from datetime import datetime, timezone
from pathlib import Path
import shutil
import tempfile
import zipfile

import pymupdf

from .pdf_ops import (ProcessingError, validate_pdf, apply_output_marks,
                      validate_overlay_image)
from .editable_marks import apply_pdf_marks
from .output_processing import process_generated_pdf
from .office_export import word_draft, excel_index, powerpoint_draft
from .file_io import (iter_plain_chunks, open_pdf, protected, read_bytes,
                      save_pdf, size as file_size, write_bytes)
from .storage import Store, utcnow
from .vault import EncryptedReader, encrypted_sink
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
    'courtbundle_editable_pdf_marks': 'PDF-scope text, shapes and stamps flattened; staged redactions applied to generated PDFs when revision anchors resolve',
    'reviewed_user_links': 'retained when both revision-anchored endpoints resolve',
    'generated_index_links': 'retained',
}


def load_profiles() -> dict:
    path = Path(__file__).resolve().parent / 'rules' / 'profiles.json'
    return {p['id']: p for p in json.loads(path.read_text(encoding='utf-8'))}


def supported_index_script(value: str) -> bool:
    """Fail closed outside the Latin/Devanagari visual pilot and common punctuation."""
    return all(32 <= ord(char) <= 0x024f or
               0x0900 <= ord(char) <= 0x097f or
               0x2000 <= ord(char) <= 0x206f or
               0x20a0 <= ord(char) <= 0x20cf or
               0xa8e0 <= ord(char) <= 0xa8ff
               for char in value)


def original_integrity_issues(store: Store, docs: list[dict]) -> list[dict]:
    """Compare imported originals with their recorded hashes without changing either."""
    issues = []
    for doc in docs:
        original = store.original_path(doc)
        common = dict(severity='error', document_id=doc['id'], artifact='original')
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


def preflight(store: Store, matter_id: str, profile: dict, *,
              documents_override: list[dict] | None = None,
              settings_override: dict | None = None) -> list[dict]:
    docs = documents_override if documents_override is not None else store.get_bundle_documents(matter_id)
    matter = store.get_matter(matter_id)
    output_settings = settings_override if settings_override is not None else store.get_output_marks(matter_id)
    filing_review = store.get_filing_review(matter_id)
    issues = []
    if not docs:
        issues.append(dict(severity='error', code='NO_DOCUMENTS', message='Include at least one document in Bundle membership before building.', document_id=None))
    if profile['status'] != 'APPROVED':
        issues.append(dict(severity='info', code='PROFILE_NOT_APPROVED', message='This is a demo/illustrative profile, not a verified court-compliance determination.', document_id=None))
    if matter and not supported_index_script(matter['name']):
        issues.append(dict(severity='error', code='UNSUPPORTED_INDEX_SCRIPT',
                           message='The matter name contains a script outside the current Latin/Devanagari visual pilot. Generated index output is blocked until this script is verified.',
                           document_id=None))
    if matter and cover_pages(output_settings):
        cover_text = ''.join(str(output_settings.get(k, '')) for k in ('cover_heading', 'cover_reference', 'cover_title'))
        cover_text += ''.join(str(matter.get(k, '')) for k in ('petitioner', 'respondent', 'case_number'))
        if not supported_index_script(cover_text):
            issues.append(dict(severity='error', code='UNSUPPORTED_COVER_SCRIPT', document_id=None,
                message='Cover text contains a script outside the current Latin/Devanagari visual pilot. Review supported text before building.'))
    pending_review = sum(item['status'] == 'unreviewed' for item in filing_review['items'])
    if pending_review:
        issues.append(dict(severity='info', code='USER_CHECKLIST_PENDING',
                           message=f'{pending_review} user-authored checklist item(s) remain unreviewed. This checklist is not a court-rule determination.',
                           document_id=None))
    if (matter and filing_review['case_type'] and
            filing_review['case_type'].strip().casefold() != matter['case_type'].strip().casefold()):
        issues.append(dict(severity='warning', code='USER_CHECKLIST_SCOPE_MISMATCH',
                           message='The user-authored checklist case type differs from the matter case type. Review whether these items still apply.',
                           document_id=None))
    issues.extend(original_integrity_issues(store, docs))
    seen_hashes = set()
    seen_labels = set()
    for doc in docs:
        common = dict(document_id=doc['id'], target_revision=doc['revision'], artifact='working')
        if not supported_index_script(doc['title'] + doc['label']):
            issues.append(dict(severity='error', code='UNSUPPORTED_INDEX_SCRIPT',
                               message=f'{doc["title"]} contains a script outside the current Latin/Devanagari visual pilot. Generated index output is blocked until this script is verified.',
                               **common))
        mode = annexure_mode(doc, output_settings)
        if mode != 'none':
            if doc['label_status'] != 'confirmed' or not doc['label'].strip():
                issues.append(dict(severity='error', code='UNCONFIRMED_ANNEXURE_MARK',
                                   message=f'{doc["title"]} needs a confirmed register label before {mode.replace("_", " ")} output.',
                                   **common))
            elif any(ord(char) < 32 or ord(char) > 126 for char in doc['label']):
                issues.append(dict(severity='error', code='UNSUPPORTED_ANNEXURE_MARK_SCRIPT',
                                   message=f'{doc["title"]} has a label that the technical stamp font cannot render.',
                                   **common))
            if mode == 'separator' and any(ord(char) < 32 or ord(char) > 126 for char in doc['title']):
                issues.append(dict(severity='error', code='UNSUPPORTED_SEPARATOR_TITLE_SCRIPT',
                                   message=f'{doc["title"]} has a title that the technical separator font cannot render.',
                                   **common))
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
                        issues.append(dict(severity='warning', code='UNSEARCHABLE_PAGE', page_index=index, message=f'{doc["title"]}, page {index + 1} has no extractable text. Consider OCR/review.', **common))
                if annotation_count:
                    issues.append(dict(severity='warning', code='SOURCE_ANNOTATIONS_OMITTED',
                                       message=f'{doc["title"]} has {annotation_count} working-copy annotation(s) that will be omitted from volume and combined PDFs; the imported original is retained.', **common))
                if link_count:
                    issues.append(dict(severity='warning', code='SOURCE_LINKS_OMITTED',
                                       message=f'{doc["title"]} has {link_count} working-copy link(s) that will be omitted from volume and combined PDFs; generated index links remain.', **common))
        except Exception as exc:
            issues.append(dict(severity='error', code='BAD_PDF', message=f'Cannot process {doc["title"]}: {exc}', **common))
    included_ids = {d['id'] for d in docs}
    annotations = [a for a in store.list_page_annotations(matter_id) if a['document_id'] in included_ids]
    for item in annotations:
        if item.get('output_scope') == 'pdf' and item['kind'] != 'image' and not item['resolved']:
            issues.append(dict(severity='error', code='UNRESOLVED_EDITABLE_PDF_MARK',
                message='An editable PDF mark targets an older document revision; resolve or remove it before saving or building.',
                document_id=item['document_id'], target_revision=item['target_revision'],
                page_index=item['page_index'], artifact='working'))
    private_count = sum(item.get('output_scope', 'private') == 'private' for item in annotations)
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
                           message=(f'{private_count} MatterDraft private review mark(s) '
                                    'will be omitted from filing-preparation PDFs. Image overlays use a '
                                    'separate explicit flattened-content policy.'), document_id=None))
    if unresolved_images:
        issues.append(dict(severity='warning', code='UNRESOLVED_IMAGE_OVERLAYS',
                           message=(f'{unresolved_images} image overlay(s) target an older revision, '
                                    'removed document or missing page and will be omitted until reviewed.'),
                           document_id=None))
    for bookmark in store.list_manual_bookmarks(matter_id):
        if bookmark['document_id'] not in included_ids:
            issues.append(dict(severity='warning', code='EXCLUDED_BOOKMARK_OMITTED',
                message=f'Bookmark “{bookmark["title"]}” targets a reference-only or archived document and will be omitted.',
                document_id=bookmark['document_id']))
    for link in store.list_reviewed_links(matter_id):
        if link['source_document_id'] in included_ids and link['kind'] == 'internal' and link['target_document_id'] not in included_ids:
            issues.append(dict(severity='error', code='EXCLUDED_LINK_DESTINATION',
                message=f'Link “{link["title"]}” targets excluded material. Retarget or remove this link, or include its destination.',
                document_id=link['source_document_id'], target_revision=link['source_revision'],
                page_index=link['source_page_index'], artifact='working'))
    if not any(i['severity'] == 'error' for i in issues):
        issues.append(dict(severity='info', code='TECHNICAL_PREFLIGHT', message='Basic technical preflight completed. Legal completeness, authenticity, signatures, limitation and registry acceptance are not verified.', document_id=None))
    try:
        snapshot = store.build_input_digest(matter_id)
    except (OSError, ValueError):
        snapshot = None
    for issue in issues:
        issue['input_snapshot'] = snapshot
        identity = {key: issue.get(key) for key in
                    ('code', 'document_id', 'target_revision', 'page_index', 'message')}
        issue['finding_id'] = hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()[:24]
    return issues


def index_layout(settings: dict | None = None) -> tuple[int, int]:
    if (settings or {}).get('index_style') == 'review':
        return (8, 65)
    return (16, 35) if ((settings or {}).get('index_style') == 'readable' or (settings or {}).get('index_date')) else (INDEX_ROWS_PER_PAGE, 25)


def index_row_y(offset: int, settings: dict) -> int:
    return (220 if settings.get('index_style') == 'review' else 187) + offset * index_layout(settings)[1]


def index_link_rect(offset: int, settings: dict) -> pymupdf.Rect:
    y = index_row_y(offset, settings)
    if settings.get('index_style') == 'review':
        return pymupdf.Rect(30, y-22, 565, y+38)
    readable = index_layout(settings)[1] == 35
    return pymupdf.Rect(90, y-(20 if readable else 14), 545, y+(15 if readable else 8))


def index_pages_for(doc_count: int, settings: dict | None = None) -> int:
    return max(1, math.ceil(doc_count / index_layout(settings)[0]))


def cover_pages(settings: dict | None = None) -> int:
    return int((settings or {}).get('front_matter') == 'cover_index')

def annexure_mode(document: dict, settings: dict) -> str:
    if document.get('kind') != 'annexure':
        return 'none'
    return settings.get('annexure_overrides', {}).get(
        document['id'], settings.get('annexure_mode', 'none'))


def volume_rows(documents: list[dict], volume_no: int,
                settings: dict | None = None) -> tuple[list[dict], int]:
    """The single page map used by preview, PDF rendering, links and CSV."""
    settings = settings or {}
    index_count = index_pages_for(len(documents), settings)
    position = index_count + cover_pages(settings)
    rows = []
    for doc in documents:
        mode = annexure_mode(doc, settings)
        separator_page = position + 1 if mode == 'separator' else None
        if separator_page is not None:
            position += 1
        start = position + 1
        end = position + doc['page_count']
        rows.append(dict(id=doc['id'], part=doc['part'], kind=doc['kind'],
                         label=doc['label'], title=doc['title'], document_date=doc.get('document_date'),
                         revision=doc.get('revision'), source_start=doc.get('_source_start', 0), source_end=doc.get('_source_start', 0)+doc['page_count'], start_page=start,
                         end_page=end, volume=volume_no, total_pages=doc['page_count'],
                         annexure_mode=mode, separator_page=separator_page))
        position = end
    return rows, index_count


def bates_page_labels(plans: list[list[dict]], settings: dict) -> list[list[str | None]]:
    """Number selected physical pages from the same volume rows as CSV/bookmarks."""
    all_labels = []
    next_number = settings.get('bates_start', 1)
    for volume_number, group in enumerate(plans, start=1):
        if settings.get('bates_restart_per_volume'):
            next_number = settings.get('bates_start', 1)
        rows, index_count = volume_rows(group, volume_number, settings)
        labels: list[str | None] = [None] * (rows[-1]['end_page'] if rows else index_count)
        if settings.get('bates_enabled'):
            included = set(range(cover_pages(settings), cover_pages(settings) + index_count)) if settings.get('bates_include_index') else set()
            for row in rows:
                if row['separator_page'] is not None and settings.get('bates_include_separator'):
                    included.add(row['separator_page'] - 1)
                included.update(range(row['start_page'] - 1, row['end_page']))
            for index in sorted(included):
                labels[index] = (settings.get('bates_prefix', '') +
                                 str(next_number).zfill(settings.get('bates_padding', 6)))
                next_number += 1
        all_labels.append(labels)
    return all_labels


def roman_number(number: int) -> str:
    # Extended Roman notation is explicit: thousands repeat M.
    result = ''
    for value, symbol in [(1000,'M'),(900,'CM'),(500,'D'),(400,'CD'),(100,'C'),
                          (90,'XC'),(50,'L'),(40,'XL'),(10,'X'),(9,'IX'),(5,'V'),(4,'IV'),(1,'I')]:
        count, number = divmod(number, value)
        result += symbol * count
    return result


def numbering_maps(plans: list[list[dict]], settings: dict) -> list[dict]:
    """Labels decorate physical positions; they never become destinations or identities."""
    result, combined_offset = [], 0
    start = settings.get('numbering_start', 1)
    next_number, previous_section = start, None
    for vno, group in enumerate(plans, 1):
        rows, indexes = volume_rows(group, vno, settings)
        front = cover_pages(settings) + indexes
        count = rows[-1]['end_page'] if rows else front
        if settings.get('numbering_restart', 'volume') == 'volume':
            next_number = start
        pages = []
        by_position = {}
        for row in rows:
            for physical in range(row['separator_page'] or row['start_page'], row['end_page']+1):
                by_position[physical] = row
        for physical in range(1, count+1):
            row = by_position.get(physical)
            is_front = physical <= front
            front_policy = settings.get('numbering_front_matter', 'physical')
            if is_front and front_policy != 'physical':
                printed = ''
                label = roman_number(physical).lower() if front_policy == 'roman' else ''
            else:
                section = row['part'] if row else None
                if row and settings.get('numbering_restart') == 'section' and section != previous_section:
                    next_number = start
                if row:
                    previous_section = section
                style = settings.get('numbering_style', 'arabic')
                number = str(next_number) if style == 'arabic' else roman_number(next_number)
                if style == 'roman_lower':
                    number = number.lower()
                prefix = settings.get('numbering_prefix', '').replace('{volume}', str(vno)).replace('{section}', str(section or ''))
                printed = label = prefix + number
                next_number += 1
            source_index = physical-row['start_page']+row['source_start'] if row and physical >= row['start_page'] else None
            pages.append(dict(physical_page=physical, combined_page=combined_offset+physical,
                printed_label=printed, pdf_label=label if settings.get('numbering_pdf_labels', True) else str(physical),
                document_id=row['id'] if source_index is not None else None, page_index=source_index,
                revision=row['revision'] if source_index is not None else None))
        for row in rows:
            row.update(printed_start=pages[row['start_page']-1]['printed_label'],
                       printed_end=pages[row['end_page']-1]['printed_label'],
                       combined_start_page=combined_offset+row['start_page'],
                       combined_end_page=combined_offset+row['end_page'])
        result.append(dict(rows=rows, pages=pages))
        combined_offset += count
    return result


def number_stamp_rect(page, settings: dict):
    position = settings.get('numbering_position', 'bottom_center')
    inset = 24 if position == 'bottom_center' else 12 * 72 / 25.4
    top = inset if position.startswith('top') else page.rect.height-inset-11
    return pymupdf.Rect(inset, top, page.rect.width-inset, top+16)


def stamp_page_number(page, record: dict, settings: dict):
    label = record['printed_label']
    if not label:
        return
    position = settings.get('numbering_position', 'bottom_center')
    rect = number_stamp_rect(page, settings)
    align = 0 if position.endswith('left') else 2 if position.endswith('right') else 1
    if page.insert_textbox(rect, label, fontsize=9, align=align, color=(.24,.28,.33)) < 0:
        raise ProcessingError('Number label does not fit this page; shorten the prefix or change numbering style.')


def back_index_link(page, target: int, stamp: bool = True):
    rect = pymupdf.Rect(34, page.rect.height-58, 130, page.rect.height-43)
    if stamp:
        page.insert_text((34, page.rect.height-47), 'Back to index', fontsize=8, color=(.16,.40,.75))
    page.insert_link({'kind':pymupdf.LINK_GOTO, 'from':rect, 'page':target, 'to':pymupdf.Point(0,0)})


def pdf_label_rules(pages: list[dict], offset: int = 0) -> list[dict]:
    return [{'startpage':offset+i, 'prefix':p['pdf_label'], 'style':''} for i,p in enumerate(pages)]


def set_output_page_labels(pdf, rules: list[dict]):
    # PyMuPDF's set_page_labels interpolates unescaped UTF-8 into a PDF literal.
    # Encode proper PDF text strings, including punctuation in user prefixes.
    nums = ' '.join(str(r['startpage']) + ' << /P ' + pymupdf.get_pdf_str(r.get('prefix','')) + ' >>' for r in rules)
    pdf.xref_set_key(pdf.pdf_catalog(), 'PageLabels', '<< /Nums [' + nums + '] >>')


def printed_range(row: dict) -> str:
    first, last = row.get('printed_start', str(row['start_page'])), row.get('printed_end', str(row['end_page']))
    return first if first == last else first + '–' + last


def render_numbering_preview(store: Store, matter_id: str, max_pages: int, keep_groups: bool,
                             settings: dict, document_id: str, page_number: int, revision: int) -> dict:
    import base64
    plan = bundle_preview(store, matter_id, max_pages, keep_groups, settings)
    doc = next((d for d in store.get_bundle_documents(matter_id) if d['id'] == document_id), None)
    if not doc or doc['revision'] != revision or page_number > doc['page_count']:
        raise ProcessingError('Select a page in a current included working document. The target is missing, excluded or changed.')
    volume = next(v for v in plan['volumes'] if any(p['document_id']==document_id and p['page_index']==page_number-1 for p in v['pages']))
    record = next(p for p in volume['pages'] if p['document_id']==document_id and p['page_index']==page_number-1)
    with open_pdf(store.working_path(doc)) as source, pymupdf.open() as output:
        output.insert_pdf(source, from_page=page_number-1, to_page=page_number-1, links=False, annots=False)
        # Apply printable marks using the same revision-bound renderer as export.
        annotations = [a for a in store.list_page_annotations(matter_id) if a['document_id']==document_id and a['page_index']==page_number-1]
        for item in annotations:
            if item['kind'] == 'image' and item['asset_relpath']:
                item['_asset_path'] = str(store.matter_dir(matter_id) / item['asset_relpath'])
        apply_page_annotations(output, annotations, {document_id:-(page_number-1)})
        stamp_page_number(output[0], record, settings)
        if settings.get('numbering_back_link'):
            back_index_link(output[0], 0)
        row = next(r for r in volume['rows'] if r['id']==document_id)
        apply_output_marks(output[0], settings, bates_text=volume['bates_labels'][record['physical_page']-1],
            label_text=doc['label'] if row['annexure_mode']=='label_stamp' and (page_number==1 or settings.get('annexure_stamp_scope')=='all') else None)
        image = base64.b64encode(output[0].get_pixmap(matrix=pymupdf.Matrix(1.2,1.2),alpha=False).tobytes('png')).decode()
        return dict(record, volume=volume['number'], document_title=doc['title'], source_pages=doc['page_count'],
            combined_pages=plan['combined_pages'], bates=volume['bates_labels'][record['physical_page']-1],
            image='data:image/png;base64,'+image, text=output[0].get_text(), input_snapshot=store.build_input_digest(matter_id))


def bundle_preview(store: Store, matter_id: str, max_pages: int,
                   keep_groups: bool, settings: dict | None = None, *, documents_override: list[dict] | None = None) -> dict:
    """Read-only view of the exact current volume/index/page-number plan."""
    documents = documents_override if documents_override is not None else store.get_bundle_documents(matter_id)
    settings = settings if settings is not None else store.get_output_marks(matter_id)
    bookmarks = store.list_manual_bookmarks(matter_id)
    document_titles = {document['id']: document['title'] for document in store.get_documents(matter_id)}
    plans = volume_plan(documents, max_pages, keep_groups, settings) if documents else []
    bates_labels = bates_page_labels(plans, settings)
    numbered = numbering_maps(plans, settings)
    volumes = []
    destinations = {}
    combined_offset = 0
    for number, group in enumerate(plans, start=1):
        rows = numbered[number-1]['rows']
        index_count = index_pages_for(len(group), settings)
        page_count = rows[-1]['end_page'] if rows else index_count
        for row in rows:
            for i in range(row['total_pages']):
                destinations[(row['id'], row['source_start']+i)] = (number, row['start_page']+i, combined_offset)
            row['combined_start_page'] = combined_offset + row['start_page']
            row['combined_end_page'] = combined_offset + row['end_page']
            row['bates_first'] = bates_labels[number - 1][row['start_page'] - 1]
            row['bates_last'] = bates_labels[number - 1][row['end_page'] - 1]
            row['separator_bates'] = (bates_labels[number - 1][row['separator_page'] - 1]
                                      if row['separator_page'] is not None else None)
        estimated_bytes = sum(round(file_size(store.working_path(d))*d['page_count']/d.get('_source_total', d['page_count'])) for d in group) + (page_count-sum(d['page_count'] for d in group))*2048
        volumes.append(dict(estimated_bytes=estimated_bytes, number=number, index_pages=index_count, cover_pages=cover_pages(settings),
                            page_count=page_count, combined_start_page=combined_offset + 1,
                            combined_end_page=combined_offset + page_count,
                            rows=rows, pages=numbered[number-1]['pages'], bates_labels=bates_labels[number - 1]))
        combined_offset += page_count
    manual = []
    for item in bookmarks:
        destination = destinations.get((item['document_id'], item['page_index']))
        resolved = bool(item['resolved'] and destination)
        manual.append(dict(id=item['id'], title=item['title'], resolved=resolved,
                           document_title=document_titles.get(item['document_id'], 'Archived document'),
                           document_page=item['page_index'] + 1,
                           volume=destination[0] if resolved else None,
                           printed_label=volumes[destination[0]-1]['pages'][destination[1]-1]['printed_label'] if resolved else None,
                           volume_page=destination[1] if resolved else None,
                           combined_page=destination[2] + destination[1] if resolved else None))
    return dict(volumes=volumes, combined_pages=combined_offset,
                source_pages=sum(d['page_count'] for d in documents),
                generated_pages=combined_offset-sum(d['page_count'] for d in documents),
                included_documents=len(documents), manual_bookmarks=manual,
                part_extracts=[{'part': part, 'documents': sum(row['part'] == part for volume in volumes for row in volume['rows']),
                                'pages': sum(row['total_pages'] for volume in volumes for row in volume['rows'] if row['part'] == part)}
                               for part in range(1, 5)
                               if settings.get('part_extracts_enabled') and any(
                                   row['part'] == part for volume in volumes for row in volume['rows'])],
                bates_policy={'enabled': settings.get('bates_enabled', False),
                    'include_index': settings.get('bates_include_index', False),
                    'include_separator': settings.get('bates_include_separator', False),
                    'restart_per_volume': settings.get('bates_restart_per_volume', False)},
                numbering='Printed labels and PDF labels are distinct from physical volume and combined positions; all destinations use this engine map.')


def supplemental_documents(store: Store, matter_id: str, selected_ids: list[str]) -> tuple[list[dict], list[dict]]:
    """Resolve exact selected identities into ordered contiguous source runs."""
    from .supplements import selection
    if len(selected_ids) != len(set(selected_ids)):
        raise ProcessingError('Selected page identities must be unique')
    resolved, issues = selection(store, matter_id, selected_ids)
    if issues:
        raise ProcessingError(issues[0]['reason'])
    docs = {d['id']: d for d in store.get_documents(matter_id)}
    runs = []
    for page in resolved:
        doc = docs[page['document_id']]
        if runs and runs[-1]['id'] == doc['id'] and runs[-1]['_source_start'] + runs[-1]['page_count'] == page['page_index']:
            runs[-1]['page_count'] += 1
        else:
            runs.append({**doc, 'page_count': 1, '_source_start': page['page_index'],
                         '_source_total': doc['page_count'], 'group_key': ''})
    return runs, resolved


def supplemental_preview(store: Store, matter_id: str, selected_ids: list[str], settings: dict) -> dict:
    """Resolve exact selected identities into contiguous runs, then the shared map.

    Selection order is explicit. Source positions, printed labels and physical positions
    remain distinct. Each noncontiguous run has a separate index entry.
    """
    runs, resolved = supplemental_documents(store, matter_id, selected_ids)
    result = bundle_preview(store, matter_id, settings['max_pages_per_volume'],
                            settings['keep_groups_intact'], settings['output_marks'], documents_override=runs)
    identity = {(p['document_id'], p['page_index']): p['id'] for p in resolved}
    for volume in result['volumes']:
        for page in volume['pages']:
            page['stable_page_id'] = identity.get((page['document_id'], page['page_index']))
    result['included_documents'] = len({p['document_id'] for p in resolved})
    result['selected_page_ids'] = selected_ids
    return result


def _source_page_fingerprints(path: Path) -> list[str]:
    """Stable rendered/text fingerprints used only to prove a pure insertion.

    They do not create cross-revision stable identities. Any mismatch fails closed.
    """
    values = []
    with open_pdf(path) as pdf:
        for page in pdf:
            pix = page.get_pixmap(matrix=pymupdf.Matrix(1, 1), alpha=False,
                                  colorspace=pymupdf.csGRAY)
            digest = hashlib.sha256()
            digest.update(f'{page.rect.width:.3f}:{page.rect.height:.3f}:{page.rotation}:'.encode())
            digest.update(pix.samples)
            digest.update(page.get_text('text').encode('utf-8', 'replace'))
            values.append(digest.hexdigest())
    return values


def _baseline_source_contract(prior: dict) -> tuple[dict[tuple[str, int], dict], dict]:
    contract = prior.get('validation_contract', {}).get('combined.pdf')
    if not isinstance(contract, dict) or not isinstance(contract.get('sources'), dict):
        raise ProcessingError('The selected baseline lacks source-position evidence required for insertion reconciliation.')
    printed = contract.get('printed_labels')
    labels = contract.get('labels')
    if not isinstance(printed, list) or not isinstance(labels, list):
        raise ProcessingError('The selected baseline lacks printed/viewer label evidence required for insertion reconciliation.')
    by_source = {}
    for physical, source in contract['sources'].items():
        try:
            index = int(physical)
            key = (source['document_id'], int(source['page_index']))
            by_source[key] = {'combined_position': index + 1,
                              'printed_label': printed[index], 'viewer_label': labels[index]}
        except (KeyError, TypeError, ValueError, IndexError) as exc:
            raise ProcessingError('The selected baseline source map is incomplete or inconsistent.') from exc
    return by_source, contract


def insert_suffix_labels(base_label: str, count: int, existing_labels) -> list[str]:
    """The only preserved-label notation currently generated by the engine."""
    match = re.fullmatch(r'(.*\d+)', str(base_label))
    if not match:
        raise ProcessingError(
            f'Baseline label "{base_label}" cannot use the supported decimal-suffix insertion notation.')
    labels = [f'{match.group(1)}.{index}' for index in range(1, count + 1)]
    collision = next((label for label in labels if label in set(existing_labels)), None)
    if collision:
        raise ProcessingError(
            f'Generated insert label "{collision}" collides with an existing or inserted printed label.')
    return labels


def supplemental_policy_preview(store: Store, matter_id: str, plan: dict) -> dict:
    """Validate and explain the exact reviewed supplemental output policy."""
    from .supplements import baseline, catalog, selection, snapshot
    if plan.get('input_snapshot') != snapshot(store, matter_id):
        raise ProcessingError('Matter inputs changed. Reload material before reviewing the insertion policy.')
    selected_ids = plan.get('selected_page_ids') or []
    if not selected_ids:
        raise ProcessingError('Select at least one exact source page before reviewing an insertion policy.')
    mode = plan.get('insertion_policy')
    if mode not in ('separate_supplement', 'preserve_printed_labels'):
        raise ProcessingError('Choose either a separate supplement or preservation of supported printed labels.')
    prior = baseline(store, matter_id, plan['baseline_build_id'])
    settings = plan['settings']
    mapped = supplemental_preview(store, matter_id, selected_ids, settings)
    common = {'mode': mode, 'baseline_build_id': prior['id'],
              'baseline_created_at': prior['created_at'], 'plan_id': plan.get('id'),
              'plan_version': plan.get('version'), 'selected_page_ids': selected_ids,
              'selected_source_pages': len(selected_ids), 'artifact_map': mapped,
              'blockers': []}
    if mode == 'separate_supplement':
        common.update(summary='A distinct supplemental artifact will use its own reviewed numbering. The baseline stays unchanged.',
                      labels_by_page_id={}, preview_rows=[], volume_boundaries=[],
                      effects={'index_rows': sum(len(v['rows']) for v in mapped['volumes']),
                               'bookmarks': 'Rebuilt inside the supplemental artifact from its deterministic map.',
                               'links': 'Only revision-anchored destinations present in the supplemental artifact can be retained.',
                               'viewer_labels': 'Generated from the supplemental numbering settings.',
                               'physical_positions': f"{mapped['combined_pages']} physical pages in the distinct artifact."})
        return common

    baseline_revisions = prior.get('input_revisions')
    if not isinstance(baseline_revisions, list) or not baseline_revisions:
        raise ProcessingError('The selected baseline lacks exact revision-bound page IDs required for preservation.')
    source_contract, combined_contract = _baseline_source_contract(prior)
    current_catalog = {d['id']: d for d in catalog(store, matter_id)}
    current_docs = {d['id']: d for d in store.get_documents(matter_id)}
    resolved, issues = selection(store, matter_id, selected_ids)
    if issues:
        raise ProcessingError(issues[0]['reason'])
    baseline_by_doc = {item['document_id']: item for item in baseline_revisions}
    blockers = []
    if any(page['document_id'] not in baseline_by_doc for page in resolved):
        blockers.append({'code': 'NEW_DOCUMENT_INSERTION_UNSUPPORTED',
                         'message': 'Preserved insert labels currently require selected pages inside a document recorded by the baseline. Build a separate supplement for new-document pages.'})
    expected_order = sorted(resolved, key=lambda page: (
        next((i for i, item in enumerate(baseline_revisions) if item['document_id'] == page['document_id']), 10**9),
        page['page_index']))
    if [p['id'] for p in expected_order] != selected_ids:
        blockers.append({'code': 'INSERT_SELECTION_ORDER_UNSUPPORTED',
                         'message': 'Preserve-label insertion requires selected pages in current source order. Reorder the selection or build a separate supplement.'})
    identities = {}
    with store.connect() as con:
        for row in con.execute('SELECT * FROM source_page_identities WHERE matter_id=?', (matter_id,)):
            identities[row['id']] = dict(row)
    selected = set(selected_ids)
    groups = []
    for document_id, baseline_item in baseline_by_doc.items():
        selected_pages = [p for p in resolved if p['document_id'] == document_id]
        if not selected_pages:
            continue
        current = current_catalog.get(document_id)
        doc = current_docs.get(document_id)
        baseline_ids = baseline_item.get('page_ids')
        if not current or not doc or not isinstance(baseline_ids, list) or not baseline_ids:
            blockers.append({'code': 'DELETED_BASELINE_REFERENCE',
                             'document_id': document_id,
                             'message': 'A baseline document or its exact page identities are missing. Preserved references cannot be proven.'})
            continue
        baseline_rows = [identities.get(page_id) for page_id in baseline_ids]
        if any(row is None for row in baseline_rows):
            blockers.append({'code': 'DELETED_BASELINE_REFERENCE', 'document_id': document_id,
                             'message': 'A baseline page identity is missing. Preserved references cannot be proven.'})
            continue
        current_ids = [p['id'] for p in current['pages']]
        remaining_ids = [page_id for page_id in current_ids if page_id not in selected]
        if len(remaining_ids) != len(baseline_ids):
            blockers.append({'code': 'DELETED_BASELINE_REFERENCE', 'document_id': document_id,
                             'message': 'Removing the selected insert pages does not restore the baseline page count. A page was deleted, replaced, or an insertion was not selected.'})
            continue
        try:
            baseline_path = store.matter_dir(matter_id) / baseline_rows[0]['working_relpath']
            baseline_fingerprints = _source_page_fingerprints(baseline_path)
            current_fingerprints = _source_page_fingerprints(store.working_path(doc))
        except (OSError, ValueError) as exc:
            blockers.append({'code': 'BASELINE_BYTES_UNAVAILABLE', 'document_id': document_id,
                             'message': 'Baseline or current working bytes could not be read for exact insertion reconciliation.'})
            continue
        remaining_fingerprints = [current_fingerprints[i] for i, page_id in enumerate(current_ids)
                                  if page_id not in selected]
        if remaining_fingerprints != baseline_fingerprints:
            code = ('AMBIGUOUS_REPLACEMENT' if len(set(baseline_fingerprints)) != len(baseline_fingerprints)
                    else 'CHANGED_BASELINE_REFERENCE')
            blockers.append({'code': code, 'document_id': document_id,
                             'message': 'Earlier baseline pages no longer reconcile in the same order after removing selected inserts. Replacements, changes, reordering, and ambiguous duplicate pages are unsupported.'})
            continue
        selected_positions = [i for i, page_id in enumerate(current_ids) if page_id in selected]
        for position in selected_positions:
            anchor_index = sum(1 for i in range(position) if current_ids[i] not in selected) - 1
            if anchor_index < 0:
                blockers.append({'code': 'INSERT_BEFORE_FIRST_UNSUPPORTED', 'document_id': document_id,
                                 'message': 'Preserved decimal-suffix labels require an earlier baseline page. Inserts before the first baseline page are unsupported.'})
                continue
            anchor_row = baseline_rows[anchor_index]
            anchor_key = (document_id, anchor_row['page_index'])
            anchor = source_contract.get(anchor_key)
            if not anchor:
                blockers.append({'code': 'MISSING_BASELINE_MAP', 'document_id': document_id,
                                 'message': 'The insertion anchor has no baseline combined-position and printed-label evidence.'})
                continue
            if groups and groups[-1]['document_id'] == document_id and groups[-1]['anchor_index'] == anchor_index:
                groups[-1]['page_ids'].append(current_ids[position])
                groups[-1]['current_positions'].append(position + 1)
            else:
                groups.append({'document_id': document_id, 'title': doc['title'],
                               'anchor_index': anchor_index, 'anchor_source_index': anchor_row['page_index'],
                               'anchor': anchor, 'page_ids': [current_ids[position]],
                               'current_positions': [position + 1]})
    if blockers:
        common.update(summary='Preserved labels are blocked until every baseline reference reconciles.',
                      labels_by_page_id={}, preview_rows=[], volume_boundaries=[], blockers=blockers,
                      effects={'index_rows': 'Not recalculated', 'bookmarks': 'Not recalculated',
                               'links': 'Not recalculated', 'viewer_labels': 'Not accepted',
                               'physical_positions': 'Not accepted'})
        return common

    existing_labels = {str(label) for label in combined_contract['printed_labels'] if str(label)}
    labels_by_page_id = {}
    for group in groups:
        try:
            group['labels'] = insert_suffix_labels(
                group['anchor']['printed_label'], len(group['page_ids']),
                existing_labels | set(labels_by_page_id.values()))
        except ProcessingError as exc:
            blockers.append({'code': ('INSERT_LABEL_COLLISION' if 'collides' in str(exc)
                                      else 'UNSUPPORTED_BASELINE_LABEL'),
                             'document_id': group['document_id'], 'message': str(exc)})
            continue
        labels_by_page_id.update(zip(group['page_ids'], group['labels']))
    if blockers:
        common.update(summary='Preserved labels are blocked by unsupported or colliding notation.',
                      labels_by_page_id={}, preview_rows=[], volume_boundaries=[], blockers=blockers,
                      effects={'index_rows': 'Not recalculated', 'bookmarks': 'Not recalculated',
                               'links': 'Not recalculated', 'viewer_labels': 'Not accepted',
                               'physical_positions': 'Not accepted'})
        return common

    cumulative = 0
    preview_rows = []
    for group in sorted(groups, key=lambda item: item['anchor']['combined_position']):
        anchor_position = group['anchor']['combined_position'] + cumulative
        preview_rows.append({'material': 'Earlier material',
                             'source_position': f"Baseline source {group['anchor_source_index'] + 1} · current source {group['current_positions'][0] - 1}",
                             'printed_label': group['anchor']['printed_label'],
                             'combined_position': anchor_position, 'kind': 'earlier'})
        for index, (page_id, source_position, label) in enumerate(zip(
                group['page_ids'], group['current_positions'], group['labels'])):
            if index in (0, len(group['page_ids']) - 1) or len(group['page_ids']) <= 2:
                preview_rows.append({'material': 'Inserted material', 'stable_page_id': page_id,
                                     'source_position': f'Current source {source_position}',
                                     'printed_label': label,
                                     'combined_position': anchor_position + index + 1,
                                     'kind': 'inserted'})
        cumulative += len(group['page_ids'])
        next_key = (group['document_id'], group['anchor_source_index'] + 1)
        if next_key in source_contract:
            nxt = source_contract[next_key]
            preview_rows.append({'material': f"Earlier source page {group['anchor_source_index'] + 2}",
                                 'source_position': f"Current source {group['current_positions'][-1] + 1}",
                                 'printed_label': nxt['printed_label'],
                                 'combined_position': nxt['combined_position'] + cumulative,
                                 'kind': 'earlier'})
    volume_boundaries, old_end = [], 0
    for number in range(1, int(prior.get('volumes', 0)) + 1):
        contract = prior.get('validation_contract', {}).get(f'volume-{number:02}.pdf', {})
        pages = int(contract.get('pages', 0))
        if not pages:
            continue
        old_end += pages
        shift = sum(len(group['page_ids']) for group in groups
                    if group['anchor']['combined_position'] <= old_end)
        volume_boundaries.append({'volume': number, 'baseline_end': old_end,
                                  'reconciled_end': old_end + shift, 'shift': shift})
    common.update(summary='Earlier printed/viewer labels stay fixed; only supported decimal-suffix insert labels are added. Physical positions and navigation are recalculated.',
                  labels_by_page_id=labels_by_page_id, preview_rows=preview_rows,
                  volume_boundaries=volume_boundaries, blockers=[],
                  effects={'index_rows': len(prior.get('page_map_snapshot') or []),
                           'bookmarks': f"{len(combined_contract.get('toc') or [])} stored outline entries will be rebuilt against reconciled physical positions.",
                           'links': f"{sum(len(v) for v in (combined_contract.get('links') or {}).values())} stored internal index/link destinations will be recalculated.",
                           'viewer_labels': f'{len(existing_labels)} existing printed labels retained; {len(labels_by_page_id)} reviewed insert labels added.',
                           'physical_positions': f'{len(labels_by_page_id)} inserted source pages shift later combined positions and affected volume boundaries.'})
    return common


def apply_supplement_policy(numbered: list[dict], resolved: list[dict], policy_preview: dict):
    """Apply reviewed source-page labels to the same map used by PDF/CSV/navigation."""
    labels = policy_preview.get('labels_by_page_id') or {}
    if not labels:
        return
    identity = {(page['document_id'], page['page_index']): page['id'] for page in resolved}
    for volume in numbered:
        for page in volume['pages']:
            label = labels.get(identity.get((page.get('document_id'), page.get('page_index'))))
            if label is not None:
                page['printed_label'] = label
                page['pdf_label'] = label
        for row in volume['rows']:
            row.update(printed_start=volume['pages'][row['start_page']-1]['printed_label'],
                       printed_end=volume['pages'][row['end_page']-1]['printed_label'])


def front_matter_preview(store: Store, matter_id: str, max_pages: int, keep_groups: bool,
                         settings: dict, volume: int = 1, index_page: int = 1) -> dict:
    import base64
    plan = bundle_preview(store, matter_id, max_pages, keep_groups, settings)
    plan['input_snapshot'] = store.build_input_digest(matter_id)
    plan['output_marks'] = settings
    plan['pages'] = []
    if not plan['volumes']:
        return plan
    volume = min(volume, len(plan['volumes']))
    selected = plan['volumes'][volume-1]
    index_page = min(index_page, selected['index_pages'])
    plan.update(selected_volume=volume, selected_index_page=index_page)
    with pymupdf.open() as pdf:
        render_front_matter(pdf, selected['rows'], dict(store.get_matter(matter_id), _volume_total=len(plan['volumes'])), volume, settings, index_page)
        for i, page in enumerate(pdf):
            is_cover = bool(cover_pages(settings) and i == 0)
            physical = 1 if is_cover else cover_pages(settings)+index_page
            stamp_page_number(page, selected['pages'][physical-1], settings)
            label = selected['bates_labels'][physical-1]
            if label:
                apply_output_marks(page, {**settings, 'watermark_enabled':False, 'timestamp_enabled':False}, bates_text=label)
            plan['pages'].append({'kind':'cover' if is_cover else 'index', 'physical_page':physical,
                'text': page.get_text(), 'image': 'data:image/png;base64,' + base64.b64encode(
                    page.get_pixmap(matrix=pymupdf.Matrix(1.2,1.2), alpha=False).tobytes('png')).decode()})
    return plan


def page_map_entries(preview: dict) -> list[dict]:
    """Small, immutable comparison snapshot from the same map used for output."""
    return [{**{key: row[key] for key in ('id', 'part', 'label', 'title', 'volume',
            'start_page', 'end_page', 'combined_start_page', 'combined_end_page', 'printed_start', 'printed_end')},
             'source_start': row.get('source_start', 0), 'source_end': row.get('source_end', row['end_page']-row['start_page']+1),
             'printed_labels': [page['printed_label'] for page in volume['pages'][row['start_page']-1:row['end_page']]]}
            for volume in preview['volumes'] for row in volume['rows']]


def compare_bundle_plan(store: Store, matter_id: str, prior: dict,
                        max_pages: int, keep_groups: bool) -> dict:
    """Explain only changes supported by a recorded page-map snapshot."""
    if not isinstance(prior.get('page_map_snapshot'), list) or not isinstance(prior.get('input_revisions'), list):
        return {'available': False, 'reason': 'This older build did not record enough page-map detail for a reliable comparison.'}
    current = bundle_preview(store, matter_id, max_pages, keep_groups)
    old_rows = prior['page_map_snapshot']
    new_rows = page_map_entries(current)
    if any(not isinstance(row, dict) or not all(key in row for key in
           ('id', 'part', 'label', 'title', 'volume', 'start_page', 'end_page',
            'combined_start_page', 'combined_end_page')) for row in old_rows):
        return {'available': False, 'reason': 'The stored page-map snapshot is incomplete.'}
    row_key = lambda row: (row['id'], row.get('source_start', 0))
    old_by_id = {row_key(row): row for row in old_rows}
    new_by_id = {row_key(row): row for row in new_rows}
    shared_ids = set(old_by_id) & set(new_by_id)
    old_shared_order = [row_key(row) for row in old_rows if row_key(row) in shared_ids]
    new_shared_order = [row_key(row) for row in new_rows if row_key(row) in shared_ids]
    old_positions = {doc_id: index for index, doc_id in enumerate(old_shared_order)}
    new_positions = {doc_id: index for index, doc_id in enumerate(new_shared_order)}
    if any(not isinstance(item, dict) or not item.get('document_id') or not isinstance(item.get('revision'), int) for item in prior['input_revisions']):
        return {'available': False, 'reason': 'Stored revision evidence is incomplete.'}
    old_revisions = {item['document_id']: item for item in prior['input_revisions']}
    if any(row['id'] not in old_revisions for row in old_rows):
        return {'available': False, 'reason': 'Stored document revision evidence is missing.'}
    current_docs = {item['id']: item for item in store.get_documents(matter_id)}
    changes = []
    for doc_id in list(dict.fromkeys([*old_by_id, *new_by_id])):
        old, new = old_by_id.get(doc_id), new_by_id.get(doc_id)
        before = old_revisions.get(doc_id[0], {})
        after = current_docs.get(doc_id[0], {})
        reasons = []
        if old is None:
            reasons.append('added')
        elif new is None:
            reasons.append('removed from current bundle')
        else:
            if before.get('revision') != after.get('revision') or (before.get('working_sha256') and
                    before['working_sha256'] != store.sha256(store.working_path(after))):
                reasons.append('working PDF changed')
            if old_positions[doc_id] != new_positions[doc_id]:
                reasons.append('order changed')
            if old['title'] != new['title']:
                reasons.append('title changed')
            if old['part'] != new['part']:
                reasons.append('Part changed')
            if old['label'] != new['label']:
                reasons.append('label changed')
            if any(old[key] != new[key] for key in ('volume', 'start_page', 'end_page',
                    'combined_start_page', 'combined_end_page')):
                reasons.append('page range shifted')
        if reasons:
            changes.append({'document_id': doc_id[0], 'title': (new or old)['title'],
                            'reasons': reasons, 'before': old, 'after': new})
    settings = prior.get('build_settings') or {}
    setting_changes = []
    if settings.get('max_pages_per_volume') != max_pages:
        setting_changes.append('Volume page cap')
    if bool(settings.get('keep_groups_intact')) != keep_groups:
        setting_changes.append('Keep groups together')
    if prior.get('output_marks') != store.get_output_marks(matter_id):
        setting_changes.append('Output formatting and marks')
    if prior.get('filing_review') != store.get_filing_review(matter_id):
        setting_changes.append('User-authored review checklist')
    old_bookmarks = {item['id']: item for item in prior.get('manual_bookmark_inputs', [])}
    new_bookmarks = {item['id']: item for item in store.list_manual_bookmarks(matter_id)}
    bookmark_changes = sorted(item['title'] for key, item in new_bookmarks.items()
                              if key not in old_bookmarks or any(item.get(field) != old_bookmarks[key].get(field)
                              for field in ('title', 'document_id', 'page_index', 'target_revision', 'parent_id', 'position')))
    bookmark_changes.extend(item['title'] or 'Untitled bookmark' for key, item in old_bookmarks.items()
                            if key not in new_bookmarks)
    matches_inputs = (prior.get('effective_input_digest') == store.build_input_digest(matter_id, effective=True)
                      if prior.get('effective_input_digest') else
                      prior.get('build_input_digest') == store.build_input_digest(matter_id))
    document_changes = []
    old_ids = list(dict.fromkeys(r['id'] for r in old_rows))
    new_ids = list(dict.fromkeys(r['id'] for r in new_rows))
    for doc_id in dict.fromkeys([*old_ids, *new_ids]):
        before = old_revisions.get(doc_id)
        after = current_docs.get(doc_id) if doc_id in new_ids else None
        reasons = list(dict.fromkeys(reason for change in changes if change['document_id'] == doc_id for reason in change['reasons']))
        status = 'added' if doc_id not in old_ids else 'omitted' if doc_id not in new_ids else 'changed' if reasons else 'unchanged'
        old = [r for r in old_rows if r['id'] == doc_id]
        new = [r for r in new_rows if r['id'] == doc_id]
        document_changes.append(dict(document_id=doc_id, title=(new or old)[0]['title'], status=status, reasons=reasons,
            before_revision=before['revision'] if before else None, after_revision=after['revision'] if after else None,
            before_sha256=before.get('working_sha256') if before else None,
            after_sha256=store.sha256(store.working_path(after)) if after else None,
            before_source_pages=sum(r['end_page']-r['start_page']+1 for r in old),
            after_source_pages=sum(r['end_page']-r['start_page']+1 for r in new)))
    return {'available': True, 'prior_build_id': prior['id'], 'prior_created_at': prior['created_at'],
            'prior_pages': prior['pages'], 'current_pages': current['combined_pages'],
            'documents': document_changes, 'input_snapshot': store.build_input_digest(matter_id),
            'changes': changes, 'setting_changes': setting_changes,
            'bookmark_changes': bookmark_changes,
            'unresolved_bookmarks': [item['title'] for item in current['manual_bookmarks'] if not item['resolved']],
            'other_inputs_changed': (not matches_inputs
                                     and not changes and not setting_changes and not bookmark_changes),
            'current_matches_prior': (matches_inputs
                                      and not setting_changes)}


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


def split_units(docs: list[dict], keep_groups: bool, settings: dict) -> list[list[dict]]:
    """Only reviewed, revision-bound boundaries may break an imported document."""
    splits = settings.get('volume_splits', {})
    units = []
    for group in make_units(docs, keep_groups):
        pending = []
        for doc in group:
            split = splits.get(doc['id'])
            if not split:
                pending.append(doc)
                continue
            if split['revision'] != doc['revision']:
                raise ProcessingError(f'Split review is stale for {doc["title"]}; review its current revision')
            cuts = split['after_pages']
            if not cuts or cuts != sorted(set(cuts)) or cuts[0] < 1 or cuts[-1] >= doc['page_count']:
                raise ProcessingError(f'Invalid internal split boundary for {doc["title"]}')
            if pending:
                units.append(pending)
                pending = []
            start = 0
            for end in [*cuts, doc['page_count']]:
                units.append([{**doc, '_source_start': start, '_source_total': doc['page_count'],
                    'page_count': end-start, '_force_break': start > 0}])
                start = end
        if pending:
            units.append(pending)
    return units


def source_destinations(rows: list[dict], offset: int = 0) -> dict:
    return {(row['id'], row.get('source_start', 0)+i): offset+row['start_page']-1+i
            for row in rows for i in range(row['total_pages'])}


def mapped_page(destinations: dict, document_id: str, page_index: int):
    if any(isinstance(key, tuple) for key in destinations):
        return destinations.get((document_id, page_index))
    start = destinations.get(document_id)
    return None if start is None else start+page_index


def volume_plan(docs: list[dict], max_pages: int, keep_groups: bool,
                settings: dict | None = None) -> list[list[dict]]:
    settings = settings or {}
    volumes = []
    current = []
    page_count = 0
    for unit in split_units(docs, keep_groups, settings):
        if current and unit[0].get('_force_break'):
            volumes.append(current)
            current, page_count = [], 0
        unit_pages = sum(d['page_count'] + int(annexure_mode(d, settings) == 'separator')
                         for d in unit)
        next_index_pages = index_pages_for(len(current) + len(unit), settings) + cover_pages(settings)
        if page_count + unit_pages + next_index_pages <= max_pages:
            current.extend(unit)
            page_count += unit_pages
            continue
        if current:
            volumes.append(current)
            current = []
            page_count = 0
        if unit_pages + index_pages_for(len(unit), settings) + cover_pages(settings) > max_pages:
            joined = ', '.join(d['title'] for d in unit)
            raise ProcessingError(f'Document/group ({joined}) exceeds {max_pages}-page volume cap including index. No document was split. Increase the cap or revise the grouping policy.')
        current.extend(unit)
        page_count += unit_pages
    if current:
        volumes.append(current)
    return volumes


def clean_title(text: str, limit=64) -> str:
    """Base-14 fonts only support a limited character repertoire in this application."""
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
        start = mapped_page(destinations, bookmark['document_id'], bookmark['page_index'])
        if not bookmark['resolved'] or start is None:
            continue
        parent_level = resolved_levels.get(bookmark['parent_id'], 1)
        level = parent_level + 1
        resolved_levels[bookmark['id']] = level
        entries.append([level, bookmark['title'],
                        start])
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
        source_start = mapped_page(document_starts, link['source_document_id'], link['source_page_index'])
        if not link.get('resolved'):
            unresolved += 1
            continue
        if source_start is None:
            continue
        source_page_index = source_start
        if not 0 <= source_page_index < output.page_count:
            raise ProcessingError('Reviewed link source differs from the deterministic page map')
        page = output[source_page_index]
        action = {'from': _reviewed_link_rect(page, link)}
        if link['kind'] == 'external':
            action.update(kind=pymupdf.LINK_URI, uri=link['uri'])
            expected_uri.setdefault(source_page_index, []).append(link['uri'])
        else:
            target_start = mapped_page(document_starts, link['target_document_id'], link['target_page_index'])
            if target_start is None:
                omitted_cross_output += 1
                continue
            destination = target_start
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


def render_part_extracts(combined_file: Path, rows: list[dict],
                         document_starts: dict[str, int], staging: Path,
                         cancelled, numbering_pages: list[dict] | None = None, settings: dict | None = None) -> list[dict]:
    """Optional source-page extracts; destinations originate in the combined map."""
    artifacts = []
    with open_pdf(combined_file) as combined:
        for part_number in range(1, 5):
            selected = [row for row in rows if row['part'] == part_number]
            if not selected:
                continue
            filename = f'part-{part_number:02}.pdf'
            with pymupdf.open() as output:
                toc = []
                label_rules = []
                printed_labels, viewer_labels, label_regions = [], [], []
                for row in selected:
                    if cancelled():
                        raise ProcessingError('Operation cancelled')
                    start = mapped_page(document_starts, row['id'], row.get('source_start', 0))
                    end = start + row['total_pages'] - 1
                    if end >= combined.page_count:
                        raise ProcessingError('Part extract differs from the deterministic page map')
                    toc.append([1, (row['label'] + ' - ' if row['label'] else '') + row['title'],
                                output.page_count + 1])
                    for i in range(start, end+1):
                        record = numbering_pages[i] if numbering_pages else {'printed_label':combined[i].get_label()}
                        label = record.get('pdf_label', combined[i].get_label()) if (settings or {}).get('numbering_pdf_labels', True) else str(output.page_count+i-start+1)
                        label_rules.append({'startpage':output.page_count+i-start,'prefix':label,'style':''})
                        viewer_labels.append(label)
                        printed_labels.append(record['printed_label'])
                        label_regions.append(list(number_stamp_rect(combined[i], settings or {})))
                    output.insert_pdf(combined, from_page=start, to_page=end,
                                      annots=False, links=False)
                output.set_toc(toc)
                set_output_page_labels(output, label_rules)
                output.set_metadata({'title': f'Technical Part {part_number} extract',
                                     'subject': 'DRAFT MatterDraft Studio - not court certified',
                                     'creator': 'MatterDraft Studio'})
                save_pdf(output, staging / filename, garbage=4, deflate=True)
            validate_navigation(staging / filename,
                                sum(row['total_pages'] for row in selected), toc, {})
            artifacts.append({'part': part_number, 'filename': filename,
                              'documents': len(selected),
                              'pages': sum(row['total_pages'] for row in selected),
                              'validation': {'pages': len(printed_labels), 'toc': toc, 'links': {}, 'labels': viewer_labels, 'printed_labels':printed_labels, 'label_regions':label_regions}})
    return artifacts


def apply_page_annotations(output: pymupdf.Document, annotations: list[dict],
                           document_starts: dict[str, int], cancelled=None) -> dict:
    """Flatten explicitly printable marks; private review annotations stay out."""
    if any(isinstance(key, tuple) for key in document_starts):
        annotations = [{**a, 'document_id': '__output__', 'page_index': destination}
                       for a in annotations
                       if (destination := mapped_page(document_starts, a['document_id'], a['page_index'])) is not None]
        document_starts = {'__output__': 0}
    inserted = unresolved_images = private_omitted = 0
    for item in annotations:
        if cancelled and cancelled():
            raise ProcessingError('Operation cancelled')
        start = document_starts.get(item['document_id'])
        if start is None:
            continue
        if item['kind'] != 'image':
            if item.get('output_scope', 'private') == 'private':
                private_omitted += 1
            continue
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
                          keep_proportion=True, overlay=True)
        inserted += 1
    mark_summary = apply_pdf_marks(output, annotations, document_starts)
    return {'image_overlays_inserted': inserted,
            'unresolved_image_overlays': unresolved_images,
            'private_review_annotations_omitted': private_omitted,
            **mark_summary}


def render_front_matter(output: pymupdf.Document, rows: list[dict], matter: dict,
                        volume_no: int, output_marks: dict, index_page: int | None = None):
    """The same generated pages for preview and export; no source-file mutation."""
    rows_per_page, row_step = index_layout(output_marks)
    index_count = index_pages_for(len(rows), output_marks)
    if cover_pages(output_marks):
        page = output.new_page(width=A4_W, height=A4_H)
        def centred(text, top, bottom, size=16, bold=False):
            page.insert_htmlbox(pymupdf.Rect(65, top, A4_W-65, bottom), html_escape(text),
                css=f'* {{font-family:sans-serif;text-align:center;font-size:{size}px;font-weight:{700 if bold else 400};}}')
        centred(output_marks.get('cover_heading') or 'MATTER BUNDLE', 68, 145, 22, True)
        centred(output_marks.get('cover_reference') or matter.get('case_number') or 'Reference not recorded', 160, 225, 20)
        page.draw_line((65, 250), (A4_W-65, 250), color=(.82,.86,.90))
        centred(matter.get('petitioner') or matter['name'], 290, 375, 28, True)
        if matter.get('respondent'):
            centred('v.', 390, 425, 20)
            centred(matter['respondent'], 445, 535, 28, True)
        centred(output_marks.get('cover_title') or 'DOCUMENT BUNDLE', 590, 665, 32, True)
        centred(f'Volume {volume_no} of {matter.get("_volume_total", volume_no)}', 690, 730, 20, True)
        centred(f"Physical pages 1–{rows[-1]['end_page'] if rows else index_count+1}", 738, 765, 16)
        centred('DRAFT — NOT COURT CERTIFIED', 779, 802, 8)
    for i in (range(index_count) if index_page is None else [index_page - 1]):
        page = output.new_page(width=A4_W, height=A4_H)
        if output_marks.get('index_style') == 'review':
            ink = (.09, .15, .23)
            link = (.16, .40, .75)
            page.insert_textbox(pymupdf.Rect(36, 54, A4_W-36, 115), f'INDEX - VOLUME {volume_no}',
                fontsize=25, fontname='hebo', align=pymupdf.TEXT_ALIGN_CENTER, color=ink)
            page.insert_text((36, 158), 'Document / date' if output_marks.get('index_date') else 'Document', fontsize=18, fontname='hebo', color=ink)
            page.insert_text((490, 158), 'Pages', fontsize=18, fontname='hebo', color=ink)
            page.draw_line((36, 180), (A4_W-36, 180), color=(.82,.86,.90))
            for offset, row in enumerate(rows[i*rows_per_page:(i+1)*rows_per_page]):
                y = index_row_y(offset, output_marks)
                title = (row['label'] + ' · ' if row['label'] else '') + row['title']
                content = '<b>' + html_escape(title) + '</b>'
                if output_marks.get('index_date'):
                    content += '<br><span style="font-size:15px;color:#17263A">' + html_escape(row.get('document_date') or 'Date not recorded') + '</span>'
                page.insert_htmlbox(pymupdf.Rect(36,y-20,475,y+36), content,
                    css='* {font-family:sans-serif;font-size:19px;line-height:1.3;color:#2865BE;}')
                page.insert_htmlbox(pymupdf.Rect(475,y-18,560,y+36), html_escape(printed_range(row)), css='* {font-family:sans-serif;font-size:14px;color:#2865BE;}')
                page.draw_line((36,y+40),(A4_W-36,y+40),color=(.82,.86,.90),width=.6)
            page.insert_textbox(pymupdf.Rect(36,750,A4_W-36,782),f'Index {i+1} of {index_count}',fontsize=15,align=pymupdf.TEXT_ALIGN_CENTER,color=ink)
            page.insert_text((36,796),'DRAFT - NOT COURT CERTIFIED',fontsize=8,color=(.45,.48,.52))
            continue
        page.draw_rect(pymupdf.Rect(0, 0, A4_W, 10), color=(0.10, .25, .46), fill=(0.10, .25, .46))
        page.insert_text((50, 50), 'MATTERDRAFT / INDEX', fontsize=10, fontname='hebo', color=(.12,.32,.58))
        page.insert_text((50, 82), f'Volume {volume_no} - Document Index', fontsize=19, fontname='hebo', color=(.08,.16,.29))
        if any(ord(char) > 126 for char in matter['name']):
            page.insert_htmlbox(pymupdf.Rect(50, 94, 555, 135),
                html_escape(matter['name']), css='* {font-family:sans-serif;font-size:9px;}')
        else:
            page.insert_textbox(pymupdf.Rect(50, 94, 555, 135), clean_title(matter['name'], 95), fontsize=9)
        page.insert_text((50, 156), 'PART', fontsize=9, fontname='hebo')
        page.insert_text((100, 156), 'DOCUMENT / PARTICULARS', fontsize=9, fontname='hebo')
        page.insert_text((490, 156), 'PAGES', fontsize=9, fontname='hebo')
        page.draw_line((50, 164), (545, 164), color=(.7,.75,.8), width=.8)
        subset = rows[i * rows_per_page:(i + 1) * rows_per_page]
        for offset, row in enumerate(subset):
            ypos = index_row_y(offset, output_marks)
            page.insert_text((50, ypos), str(row['part']), fontsize=9)
            display_title = (row['label'] + '  ' if row['label'] else '') + row['title']
            if row_step == 35 or any(ord(char) > 126 for char in display_title):
                title_rect = (pymupdf.Rect(100, ypos - 18, 475, ypos + 14)
                              if row_step == 35
                              else pymupdf.Rect(100, ypos - 13, 475, ypos + 7))
                page.insert_htmlbox(title_rect, html_escape(display_title) +
                    ('<br><span style="font-size:8px">' + html_escape(row.get('document_date') or 'Date not recorded') + '</span>' if output_marks.get('index_date') else ''),
                    css='* {font-family:sans-serif;font-size:10px;line-height:1.1;}')
            else:
                page.insert_text((100, ypos), clean_title(display_title, 66), fontsize=9)
            start_end = printed_range(row)
            page.insert_htmlbox(pymupdf.Rect(480,ypos-12,560,ypos+12),html_escape(start_end),css='* {font-family:sans-serif;font-size:9px;}')
            # The target pages do not exist yet; insert links after adding all documents.
            if offset % 2 == 0:
                line_y = ypos + (16 if row_step == 35 else 8)
                page.draw_line((50, line_y), (545, line_y), color=(.92,.93,.95), width=.5)
        page.insert_text((50, 774), 'Index entries open their source pages. Verify legal completeness separately.', fontsize=8, color=(.45,.48,.52))
        page.insert_text((50, 796), 'DRAFT - NOT COURT CERTIFIED', fontsize=8, color=(.55,.32,.24))


def render_volume(documents: list[dict], matter: dict, path: Path, volume_no: int,
                  cancelled=None, bookmarks: list[dict] | None = None,
                  reviewed_links: list[dict] | None = None,
                  page_annotations: list[dict] | None = None,
                  output_marks: dict | None = None,
                  mark_at: datetime | str | None = None,
                  bates_labels: list[str | None] | None = None, numbering_map: dict | None = None):
    """Create an indexed, paginated, bookmarked volume and return page-map records."""
    output_marks = output_marks or {}
    rows, index_count = volume_rows(documents, volume_no, output_marks)
    numbering_map = numbering_map or numbering_maps([documents], output_marks)[0]
    rows = numbering_map['rows']
    rows_per_page, row_step = index_layout(output_marks)
    bates_labels = bates_labels or [None] * (rows[-1]['end_page'] if rows else index_count)

    with pymupdf.open() as output:
        render_front_matter(output, rows, matter, volume_no, output_marks)

        toc = ([[1, f'Volume {volume_no}: Cover', 1]] if cover_pages(output_marks) else []) + [[1, f'Volume {volume_no}: Index', cover_pages(output_marks)+1]]
        current_part = None
        for row, d in zip(rows, documents):
            if cancelled and cancelled():
                raise ProcessingError('Operation cancelled')
            if row['separator_page'] is not None:
                separator = output.new_page(width=A4_W, height=A4_H)
                separator.insert_text((50, 52), 'MATTERDRAFT / TECHNICAL SEPARATOR',
                                      fontsize=10, fontname='hebo', color=(.12, .32, .58))
                separator.insert_textbox(pymupdf.Rect(60, 170, 535, 275),
                                         clean_title(d['label'], 60), fontsize=28,
                                         fontname='hebo', align=pymupdf.TEXT_ALIGN_CENTER)
                separator.insert_textbox(pymupdf.Rect(60, 290, 535, 390),
                                         clean_title(d['title'], 120), fontsize=16,
                                         align=pymupdf.TEXT_ALIGN_CENTER)
                separator.insert_text((60, 770),
                                      'DRAFT technical separator - review before filing', fontsize=8)
            with open_pdf(d['_working_path']) as source:
                if source.page_count != d.get('_source_total', d['page_count']):
                    raise ProcessingError(f'Page count changed for {d["title"]} while building. Retry.')
                # Source annotations/links are omitted by policy; generated index links
                # are inserted from the deterministic page map after document insertion.
                for first in range(d.get('_source_start', 0), d.get('_source_start', 0)+d['page_count'], 10):
                    if cancelled and cancelled():
                        raise ProcessingError('Operation cancelled')
                    output.insert_pdf(source, from_page=first,
                                      to_page=min(first + 9, d.get('_source_start', 0)+d['page_count'] - 1),
                                      annots=False, links=False)
            if row['part'] != current_part:
                current_part = row['part']
                toc.append([1, f'Part {current_part}', row['start_page']])
            if row['separator_page'] is not None:
                toc.append([2, f'{d["label"]} separator', row['separator_page']])
            title = (row['label'] + ' - ' if row['label'] else '') + row['title']
            toc.append([2, title, row['start_page']])

        document_starts = source_destinations(rows)
        annotation_summary = apply_page_annotations(
            output, page_annotations or [], document_starts, cancelled)

        for i, record in enumerate(numbering_map['pages']):
            if cancelled and cancelled():
                raise ProcessingError('Operation cancelled')
            stamp_page_number(output[i], record, output_marks)
            if record['document_id'] and output_marks.get('numbering_back_link'):
                back_index_link(output[i], cover_pages(output_marks))

        mark_count = mark_overlap_count = 0
        if output_marks.get('bates_enabled'):
            bates_only = {**output_marks, 'watermark_enabled': False,
                          'timestamp_enabled': False}
            for page_index, label in enumerate(bates_labels):
                if cancelled and cancelled():
                    raise ProcessingError('Operation cancelled')
                if label and (cover_pages(output_marks) <= page_index < cover_pages(output_marks)+index_count or any(
                        row['separator_page'] == page_index + 1 for row in rows)):
                    results = apply_output_marks(output[page_index], bates_only,
                                                 bates_text=label)
                    mark_count += len(results)
                    mark_overlap_count += sum(bool(item['overlaps_content']) for item in results)
        for row, document in zip(rows, documents):
            for source_page in range(document['page_count']):
                if cancelled and cancelled():
                    raise ProcessingError('Operation cancelled')
                label_text = (document['label'] if row['annexure_mode'] == 'label_stamp' and
                              (source_page+row['source_start'] == 0 or output_marks.get('annexure_stamp_scope') == 'all')
                              else None)
                results = apply_output_marks(
                    output[row['start_page'] - 1 + source_page], output_marks, mark_at,
                    bates_text=bates_labels[row['start_page'] - 1 + source_page],
                    label_text=label_text)
                mark_count += len(results)
                mark_overlap_count += sum(bool(item['overlaps_content']) for item in results)

        # Link each index row to its true 0-based destination.
        for i in range(index_count) if output_marks.get('numbering_linked_index', True) else []:
            page = output[cover_pages(output_marks)+i]
            subset = rows[i * rows_per_page:(i + 1) * rows_per_page]
            for offset, row in enumerate(subset):
                ypos = index_row_y(offset, output_marks)
                page.insert_link({'kind': pymupdf.LINK_GOTO,
                                  'from': index_link_rect(offset, output_marks),
                                  'page': row['start_page'] - 1, 'to': pymupdf.Point(0, 0)})
        reviewed_summary = insert_reviewed_links(
            output, reviewed_links or [], document_starts)
        toc.extend(manual_toc(bookmarks or [], {key: value+1 for key, value in document_starts.items()}))
        output.set_toc(toc)
        if output_marks.get('numbering_pdf_labels', True):
            set_output_page_labels(output, pdf_label_rules(numbering_map['pages']))
        label_regions = [list(number_stamp_rect(page, output_marks)) for page in output]
        output.set_metadata({'title': f'{matter["name"]} - Volume {volume_no}',
                             'subject': 'DRAFT MatterDraft Studio - not court certified',
                             'creator': 'MatterDraft Studio'})
        save_pdf(output, path, garbage=4, deflate=True)

    expected_links = {}
    for index, row in enumerate(rows) if output_marks.get('numbering_linked_index', True) else []:
        expected_links.setdefault(cover_pages(output_marks) + index // rows_per_page, []).append(row['start_page'] - 1)
    if output_marks.get('numbering_back_link'):
        for i, record in enumerate(numbering_map['pages']):
            if record['document_id']:
                expected_links.setdefault(i, []).append(cover_pages(output_marks))
    for page_index, destinations in reviewed_summary['expected_goto'].items():
        expected_links.setdefault(page_index, []).extend(destinations)
    validate_navigation(path, rows[-1]['end_page'] if rows else index_count, toc, expected_links)
    validate_reviewed_links(path, reviewed_summary['expected_uri'])

    with open_pdf(path) as check:
        if check.page_count > matter['max_pages_per_volume']:
            raise ProcessingError('Internal error: resulting volume exceeded page cap')
        if len(check.get_toc()) < 2 or sum(len(check[cover_pages(output_marks)+i].get_links()) for i in range(index_count)) != (len(rows) if output_marks.get('numbering_linked_index', True) else 0):
            raise ProcessingError('Internal index/bookmark validation failed')
    return rows, index_count, {
        'validation': {'pages': rows[-1]['end_page'] if rows else index_count,
            'toc': toc, 'links': {str(k): v for k, v in expected_links.items()},
            'labels': [p['pdf_label'] for p in numbering_map['pages']],
            'printed_labels': [p['printed_label'] for p in numbering_map['pages']], 'label_regions': label_regions,
            'sources': {str(row['start_page'] - 1 + i): {'document_id': d['id'],
                'target_revision': d['revision'], 'page_index': row['source_start']+i}
                for row, d in zip(rows, documents) for i in range(d['page_count'])}},
        'reviewed_links': {key: reviewed_summary[key] for key in
                           ('retained', 'omitted_cross_output', 'unresolved')},
        'page_annotations': annotation_summary,
        'marks': {'inserted': mark_count, 'overlaps_content': mark_overlap_count},
    }


def build_bundle(store: Store, matter_id: str, export_id: str, progress, cancelled,
                 signature_acknowledged: bool = False,
                 supplement_plan: dict | None = None,
                 variant_plan: dict | None = None) -> dict:
    matter = store.get_matter(matter_id)
    if not matter:
        raise ProcessingError('Matter not found')
    profiles = load_profiles()
    profile = profiles[matter['court_profile']]
    artifact_kind = 'variant' if variant_plan else 'supplement' if supplement_plan else 'primary'
    policy_preview = None
    resolved_selection = []
    if supplement_plan:
        from .supplements import build_ready, plan_contract
        ready, reason = build_ready(store, matter_id, supplement_plan)
        if not ready:
            raise ProcessingError(reason)
        policy_preview = supplemental_policy_preview(store, matter_id, supplement_plan)
        if policy_preview['blockers']:
            raise ProcessingError(policy_preview['blockers'][0]['message'])
        documents, resolved_selection = supplemental_documents(
            store, matter_id, supplement_plan['selected_page_ids'])
        current_by_id = {d['id']: d for d in store.get_documents(matter_id)}
        source_documents = [current_by_id[doc_id] for doc_id in dict.fromkeys(
            page['document_id'] for page in resolved_selection)]
        selected_coordinates = {(page['document_id'], page['page_index']) for page in resolved_selection}
        output_marks = supplement_plan['settings']['output_marks']
        output_preset = None
        build_input_digest = plan_contract(supplement_plan)
        problems = original_integrity_issues(store, source_documents)
        for doc in source_documents:
            if not supported_index_script((doc.get('label') or '') + (doc.get('title') or '')):
                problems.append({'severity': 'error', 'code': 'UNSUPPORTED_INDEX_SCRIPT',
                                 'document_id': doc['id'],
                                 'message': f'Generated index text for {doc["title"]} uses an unsupported script.'})
        membership_snapshot = dict(bundle_id=supplement_plan['id'],
            revision=supplement_plan['version'], selection_mode='exact_revision_pages',
            selected_page_ids=supplement_plan['selected_page_ids'],
            documents=[dict(document_id=d['id'], working_revision=d['revision'],
                            source_page_count=d['page_count'],
                            selected_page_ids=[page['id'] for page in resolved_selection
                                               if page['document_id'] == d['id']])
                       for d in source_documents])
        build_settings = {**supplement_plan['settings'],
                          'court_profile': matter['court_profile'],
                          'supplement_plan_id': supplement_plan['id'],
                          'supplement_plan_version': supplement_plan['version'],
                          'baseline_build_id': supplement_plan['baseline_build_id'],
                          'insertion_policy': supplement_plan['insertion_policy']}
        bookmarks = [b for b in store.list_manual_bookmarks(matter_id)
                     if (b['document_id'], b['page_index']) in selected_coordinates]
        reviewed_links = [link for link in store.list_reviewed_links(matter_id)
                          if (link['source_document_id'], link['source_page_index']) in selected_coordinates]
        page_annotations = [a for a in store.list_page_annotations(matter_id)
                            if (a['document_id'], a['page_index']) in selected_coordinates]
        input_revisions = [{'document_id': d['id'], 'revision': d['revision'],
                            'page_ids': [page['id'] for page in resolved_selection if page['document_id'] == d['id']],
                            'source_page_count': d['page_count'],
                            'working_sha256': store.sha256(store.working_path(d))}
                           for d in source_documents]
    elif variant_plan:
        from . import variants
        current, reason = variants.is_current(store, matter_id, variant_plan)
        if not current:
            raise ProcessingError(reason)
        documents = variants.ordered_documents(store, matter_id, variant_plan['included_ids'])
        source_documents = documents
        output_marks = variant_plan['settings']['output_marks']
        output_preset = None
        build_input_digest = variants.plan_contract(variant_plan)
        problems = preflight(store, matter_id, profile, documents_override=documents,
                             settings_override=output_marks)
        membership_snapshot = dict(bundle_id=variant_plan['id'], revision=variant_plan['version'],
            selection_mode='whole_documents', documents=variant_plan['sources'])
        build_settings = {**variant_plan['settings'], 'court_profile':matter['court_profile'],
                          'variant_plan_id':variant_plan['id'],
                          'variant_plan_version':variant_plan['version'],
                          'baseline':variant_plan['baseline']}
        included_ids = set(variant_plan['included_ids'])
        bookmarks = [b for b in store.list_manual_bookmarks(matter_id) if b['document_id'] in included_ids]
        reviewed_links = [link for link in store.list_reviewed_links(matter_id) if link['source_document_id'] in included_ids]
        page_annotations = [a for a in store.list_page_annotations(matter_id) if a['document_id'] in included_ids]
        input_revisions = [dict(document_id=item['document_id'], revision=item['working_revision'],
                                page_ids=item['page_ids'], source_page_count=item['source_page_count'],
                                working_sha256=item['working_sha256']) for item in variant_plan['sources']]
    else:
        documents = store.get_bundle_documents(matter_id)
        source_documents = documents
        output_marks = store.get_output_marks(matter_id)
        output_preset = store.get_applied_output_preset(matter_id)
        problems = preflight(store, matter_id, profile)
        build_input_digest = store.build_input_digest(matter_id)
        membership = store.bundle_membership(matter_id)
        membership_snapshot = dict(bundle_id=membership['bundle_id'], revision=membership['revision'],
            selection_mode='whole_documents',
            documents=[dict(document_id=d['id'], working_revision=d['revision'],
                            source_page_count=d['page_count'], position=d['position']) for d in documents])
        build_settings = {'max_pages_per_volume': matter['max_pages_per_volume'],
                          'keep_groups_intact': matter['keep_groups_intact'],
                          'court_profile': matter['court_profile']}
        included_ids = {d['id'] for d in documents}
        bookmarks = [b for b in store.list_manual_bookmarks(matter_id) if b['document_id'] in included_ids]
        reviewed_links = [link for link in store.list_reviewed_links(matter_id) if link['source_document_id'] in included_ids]
        page_annotations = [a for a in store.list_page_annotations(matter_id) if a['document_id'] in included_ids]
        from .supplements import catalog
        page_catalog = {d['id']: d for d in catalog(store, matter_id, {doc['id'] for doc in documents})}
        input_revisions = [{'document_id': d['id'], 'revision': d['revision'],
                            'page_ids': [p['id'] for p in page_catalog[d['id']]['pages']],
                            'source_page_count': d['page_count'],
                            'working_sha256': store.sha256(store.working_path(d))}
                           for d in documents]
    signature_risks = risk_summary(source_documents)
    if signature_risks and not signature_acknowledged:
        raise ProcessingError('Signature-risk acknowledgment is required before building transformed bundle PDFs')
    for item in page_annotations:
        if item['kind'] == 'image' and item['asset_relpath']:
            item['_asset_path'] = str(store.matter_dir(matter_id) / item['asset_relpath'])
    filing_review = store.get_filing_review(matter_id)
    mark_at = datetime.now(timezone.utc)
    mark_at_iso = mark_at.isoformat(timespec='seconds')
    progress(5, 'Checking source documents')
    if any(x['severity'] == 'error' for x in problems):
        raise ProcessingError('Preflight failed: ' + '; '.join(x['message'] for x in problems if x['severity']=='error'))
    bookmark_inputs = [{key: (item[key] if item['resolved'] or key != 'title' else None)
                        for key in ('id', 'title', 'document_id',
                        'page_index', 'target_revision', 'parent_id', 'position')}
                       for item in bookmarks]
    reviewed_link_inputs = [{key: (item[key] if item['resolved'] or key not in ('title', 'uri') else None)
                            for key in ('id', 'title',
                            'source_document_id', 'source_page_index', 'source_revision',
                            'rect_x', 'rect_y', 'rect_width', 'rect_height', 'kind',
                            'target_document_id', 'target_page_index', 'target_revision',
                            'uri', 'position')} | {'resolved': item['resolved']}
                            for item in reviewed_links]
    # Export reports must prove which annotations influenced a build without
    # copying private review text or internal asset paths into filing artifacts.
    annotation_inputs = [{key: item[key] for key in (
        'id', 'document_id', 'page_index', 'target_revision', 'kind',
        'rect_x', 'rect_y', 'rect_width', 'rect_height', 'color', 'position', 'output_scope')}
        | {'resolved': item['resolved'],
           'body_present': bool(item['body']),
           'asset_sha256': (store.sha256(Path(item['_asset_path']))
                            if item['kind'] == 'image' and item.get('_asset_path')
                            and Path(item['_asset_path']).is_file() else None)}
        for item in page_annotations]
    label_inputs = [{'document_id': d['id'], 'position': d['position'],
                     'label': d['label'],
                     'label_status': d.get('label_status', 'unreviewed'),
                     'label_source': d.get('label_source', 'legacy' if d['label'] else 'none'),
                     'group_key': d['group_key'],
                     'group_role': d.get('group_role', 'member')}
                    for d in source_documents]
    signature_inputs = [{'document_id': d['id'], 'signature_status': d['signature_status'],
                         'certificate_signature_status': d['certificate_signature_status'],
                         'certificate_signature_count': d['certificate_signature_count'],
                         'visible_signature_status': d['visible_signature_status']}
                        for d in source_documents]
    docs = [dict(d, _working_path=str(store.working_path(d))) for d in documents]
    selected_settings = supplement_plan['settings'] if supplement_plan else variant_plan['settings'] if variant_plan else None
    max_pages = selected_settings['max_pages_per_volume'] if selected_settings else matter['max_pages_per_volume']
    keep_groups = selected_settings['keep_groups_intact'] if selected_settings else matter['keep_groups_intact']
    plans = volume_plan(docs, max_pages, keep_groups,
                        output_marks)
    bates_labels = bates_page_labels(plans, output_marks)
    numbered = numbering_maps(plans, output_marks)
    if supplement_plan:
        apply_supplement_policy(numbered, resolved_selection, policy_preview)
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
                group, dict(matter, _volume_total=len(plans), max_pages_per_volume=max_pages), staging / filename, vno, cancelled, bookmarks,
                reviewed_links, page_annotations, output_marks, mark_at,
                bates_labels[vno - 1], numbered[vno - 1])
            volume_rows.append(dict(filename=filename, rows=rows, index_count=index_count,
                                    content_summary=content_summary))
            all_rows.extend(rows)

        progress(70, 'Creating combined bookmarked PDF')
        combined_file = staging / 'combined.pdf'
        with pymupdf.open() as combined:
            global_toc = []
            label_rules = []
            combined_labels, combined_printed, combined_regions = [], [], []
            combined_sources = {}
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
                    combined_document_starts.update(source_destinations([row], offset))
                    combined_destinations.update({key: value+1 for key, value in source_destinations([row], offset).items()})
                    if current_part != row['part']:
                        current_part = row['part']
                        global_toc.append([2, f'Part {current_part}', destination + 1])
                    if row['separator_page'] is not None:
                        global_toc.append([3, f'{row["label"]} separator',
                                           offset + row['separator_page']])
                    global_toc.append([3, (row['label'] + ' - ' if row['label'] else '') + row['title'], destination + 1])
                    rows_per_page, row_step = index_layout(output_marks)
                    idx_page = offset + cover_pages(output_marks) + rno // rows_per_page
                    if output_marks.get('numbering_linked_index', True):
                        global_links.setdefault(idx_page, []).append(destination)
                        combined[idx_page].insert_link({'kind': pymupdf.LINK_GOTO,
                            'from': index_link_rect(rno % rows_per_page, output_marks),
                            'page': destination, 'to': pymupdf.Point(0, 0)})
                    if output_marks.get('numbering_back_link'):
                        for physical in range(row['start_page'], row['end_page']+1):
                            back_index_link(combined[offset+physical-1], offset+cover_pages(output_marks), stamp=False)
                            global_links.setdefault(offset+physical-1, []).append(offset+cover_pages(output_marks))
                if output_marks.get('numbering_pdf_labels', True):
                    label_rules.extend(pdf_label_rules(numbered[vno-1]['pages'], offset))
                combined_labels.extend(volume['content_summary']['validation']['labels'])
                combined_printed.extend(volume['content_summary']['validation']['printed_labels'])
                combined_regions.extend(volume['content_summary']['validation']['label_regions'])
                combined_sources.update({str(int(k)+offset): v for k,v in volume['content_summary']['validation']['sources'].items()})
                offset += count
            if label_rules:
                set_output_page_labels(combined, label_rules)
            else:
                combined_labels = [str(i+1) for i in range(combined.page_count)]
            combined_link_summary = insert_reviewed_links(
                combined, reviewed_links, combined_document_starts)
            for page_index, destinations in combined_link_summary['expected_goto'].items():
                global_links.setdefault(page_index, []).extend(destinations)
            global_toc.extend(manual_toc(bookmarks, combined_destinations))
            combined.set_toc(global_toc)
            combined.set_metadata({'title': f'{matter["name"]} - Combined Filing Bundle',
                                   'subject': 'DRAFT MatterDraft Studio - not court certified',
                                   'creator': 'MatterDraft Studio'})
            save_pdf(combined, combined_file, garbage=4, deflate=True)

        validate_navigation(combined_file, offset, global_toc, global_links)
        validate_reviewed_links(combined_file, combined_link_summary['expected_uri'])
        part_extracts = (render_part_extracts(combined_file, all_rows,
                         combined_document_starts, staging, cancelled, [p for v in numbered for p in v['pages']], output_marks)
                         if output_marks.get('part_extracts_enabled') else [])

        progress(83, 'Checking bookmarks and generating index CSV')
        with open_pdf(combined_file) as final:
            if not final.get_toc() or len(final.get_toc()) < len(all_rows):
                raise ProcessingError('Combined bookmarks validation failed')
            if output_marks.get('numbering_linked_index', True) and sum(len(final[i].get_links()) for i in range(final.page_count)) < len(all_rows):
                raise ProcessingError('Combined index links validation failed')
            combined_pages = final.page_count

        csvpath = staging / 'index.csv'
        csv_stream = io.StringIO(newline='')
        writer = csv.DictWriter(csv_stream, fieldnames=['volume','part','kind','label','title','source_start','source_end',
            'separator_page','start_page','end_page','total_pages','printed_start','printed_end','combined_start_page','combined_end_page'])
        writer.writeheader()
        writer.writerows({key:r[key] for key in writer.fieldnames} for r in all_rows)
        write_bytes(csvpath, csv_stream.getvalue().encode('utf-8-sig'))
        progress(85, 'Processing selected generated outputs')
        processing = [process_generated_pdf(staging / filename, output_marks, cancelled)
                      for filename in ['combined.pdf', *[v['filename'] for v in volume_rows],
                                       *[p['filename'] for p in part_extracts]]]
        volume_limit = output_marks.get('volume_max_bytes')
        exceptions = [dict(filename=v['filename'], bytes=file_size(staging/v['filename']), limit_bytes=volume_limit,
                           scope='individual_volume', document_ids=[r['id'] for r in v['rows']])
                      for v in volume_rows if volume_limit and file_size(staging/v['filename']) > volume_limit]
        if exceptions:
            error = ProcessingError('Generated volume PDF exceeds the selected workflow byte limit. Review size recovery; no export was published.')
            error.recovery = {'input_snapshot': build_input_digest, 'size_exceptions': exceptions}
            raise error
        office_files = []
        for kind, source, writer in [('docx', combined_file, word_draft),
                                     ('xlsx', csvpath, lambda p: excel_index(read_bytes(p))),
                                     ('pptx', combined_file, powerpoint_draft)]:
            if not output_marks.get('office_' + kind):
                continue
            if cancelled():
                raise ProcessingError('Operation cancelled')
            filename = ('index' if kind == 'xlsx' else 'combined') + '-review.' + kind
            try:
                write_bytes(staging / filename, writer(source))
            except (ValueError, OSError) as exc:
                raise ProcessingError(f'{kind.upper()} review draft failed: {exc}') from exc
            office_files.append(filename)
        file_bytes = file_size(combined_file)
        if any(any(ord(c)>126 for c in row['title']+row['label']) for row in all_rows):
            problems.append(dict(severity='warning', code='NON_LATIN_INDEX', document_id=None,
                                 message='Printed index uses shaped fallback fonts for non-Latin titles. Visually inspect every generated row and saved PDF; text extraction and script coverage are not verified. PDF outlines retain Unicode titles.'))
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
                               if item.get('output_scope', 'private') == 'private']
        mark_overlap_count = sum(
            volume['content_summary']['marks']['overlaps_content'] for volume in volume_rows)
        if mark_overlap_count:
            problems.append(dict(severity='warning', code='OUTPUT_MARK_OVERLAP',
                                 document_id=None,
                                 message=f'{mark_overlap_count} output mark placement(s) intersect detectable text, image, drawing, link or generated page-number content. Review every marked page; signatures and seals are not reliably detectable.'))
        problems.append(dict(severity='info', code='NOT_A_FILING_APPROVAL', document_id=None,
                             message='Technical checks passed or reported. Filing acceptance, legal validity and registry objections need human review.'))
        report = {'matter_id': matter_id, 'export_id': export_id, 'artifact_kind': artifact_kind,
                  'variant_plan_id': variant_plan['id'] if variant_plan else None,
                  'variant_plan_version': variant_plan['version'] if variant_plan else None,
                  'variant_name': variant_plan['name'] if variant_plan else None,
                  'variant_baseline': variant_plan['baseline'] if variant_plan else None,
                  'supplement_plan_id': supplement_plan['id'] if supplement_plan else None,
                  'supplement_plan_version': supplement_plan['version'] if supplement_plan else None,
                  'baseline_build_id': supplement_plan['baseline_build_id'] if supplement_plan else None,
                  'insertion_policy': supplement_plan['insertion_policy'] if supplement_plan else None,
                  'policy_preview': ({key: policy_preview[key] for key in ('mode','summary','preview_rows','volume_boundaries','effects')}
                                     if policy_preview else None),
                  'profile': profile['id'],
                  'profile_status': profile['status'], 'created_at': utcnow(),
                  'matter_updated_at': matter['updated_at'], 'input_revisions': input_revisions,
                  'label_inputs': label_inputs, 'bundle_membership': membership_snapshot,
                  'signature_inputs': signature_inputs,
                  'signature_risk_acknowledged': bool(signature_risks and signature_acknowledged),
                  'build_input_digest': build_input_digest,
                  'build_settings': build_settings, 'manual_bookmark_inputs': bookmark_inputs,
                  'reviewed_link_inputs': reviewed_link_inputs,
                  'page_annotation_inputs': annotation_inputs,
                  'output_marks': output_marks, 'output_preset': output_preset,
                  'processing': processing, 'office_review_files': office_files,
                  'output_mark_timestamp': mark_at_iso,
                  'page_map': all_rows,
                  'part_extracts': part_extracts,
                  'bates_policy': {'enabled': output_marks['bates_enabled'],
                      'include_index': output_marks['bates_include_index'],
                      'include_separator': output_marks['bates_include_separator'],
                      'restart_per_volume': output_marks['bates_restart_per_volume']},
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
                      'private_review_annotations_omitted': len(private_annotations),
                      'editable_marks_inserted': sum(volume['content_summary']['page_annotations']['editable_marks_inserted'] for volume in volume_rows),
                      'redactions_applied': sum(volume['content_summary']['page_annotations']['redactions_applied'] for volume in volume_rows)},
                  'interactive_content_policy': EXPORT_INTERACTIVE_CONTENT_POLICY,
                  'total_input_documents': len(source_documents), 'selected_source_pages': len(resolved_selection) if supplement_plan else None,
                  'volume_count': len(plans),
                  'combined_pages': combined_pages, 'combined_bytes': file_bytes,
                  'checks': problems, 'volumes': [{'name':v['filename'],'items':len(v['rows'])} for v in volume_rows]}
        write_bytes(staging / 'validation-report.json', json.dumps(report, indent=2).encode('utf-8'))
        readme = ('MatterDraft Studio - DRAFT ' + ('AUDIENCE VARIANT ' if variant_plan else 'SUPPLEMENTAL ' if supplement_plan else '') + 'EXPORT\n\n'
                  'This package is NOT court-certified and is NOT proof of filing.\n'
                  'The selected rules are illustrative technical checks only.\n'
                  'Original evidence and signed files must be reviewed independently.\n'
                  'Working-copy PDF annotation objects and link actions are omitted from volume and combined PDFs.\n'
                  'Flattened marks already in page content remain visible.\n'
                  'Resolved PDF-scope marks and image overlays are flattened into new source-document pages; staged redactions remove content in the exported PDFs. Private review notes are omitted.\n'
                  'Generated index links, bookmarks and explicitly reviewed MatterDraft links are retained when their revision-anchored targets resolve.\n'
                  'Cross-volume reviewed links are available only in combined.pdf; configured output marks apply to source document pages, not index pages.\n'
                  'See validation-report.json for omissions, mark settings and detectable overlap warnings.\n'
                  'Every PDF page number in an index is the 1-based page position WITHIN that volume.\n'
                  'combined.pdf concatenates all generated volumes, including their separate indexes.\n')
        write_bytes(staging / 'READ_ME_FIRST.txt', readme.encode('utf-8'))
        progress(90, 'Packaging export ZIP')
        zip_file = staging / 'filing-package.zip'
        part_filenames = [item['filename'] for item in part_extracts]
        def fill_zip(output):
            with zipfile.ZipFile(output, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=6) as z:
                for filename in ['combined.pdf', *[v['filename'] for v in volume_rows],
                                 *part_filenames, *office_files,
                                 'index.csv', 'validation-report.json', 'READ_ME_FIRST.txt']:
                    if cancelled():
                        raise ProcessingError('Operation cancelled')
                    with z.open(filename, 'w') as member:
                        for chunk in iter_plain_chunks(staging / filename):
                            if cancelled():
                                raise ProcessingError('Operation cancelled')
                            member.write(chunk)
        if protected(zip_file):
            with encrypted_sink(zip_file, store._data_key) as output:
                fill_zip(output)
            with EncryptedReader(zip_file, store._data_key) as source:
                with zipfile.ZipFile(source) as archive:
                    if archive.testzip() is not None:
                        raise ProcessingError('Export ZIP integrity verification failed')
        else:
            fill_zip(zip_file)
            with zipfile.ZipFile(zip_file) as archive:
                if archive.testzip() is not None:
                    raise ProcessingError('Export ZIP integrity verification failed')
        filenames = ['filing-package.zip', 'combined.pdf', *[v['filename'] for v in volume_rows],
                     *part_filenames, *office_files, 'index.csv', 'validation-report.json']
        # The ZIP may exceed the cap even when each PDF fits. Check everything offered
        # for download before the staging directory can be published as an export.
        check_export_sizes(staging, filenames, profile.get('max_file_mb'))
        page_snapshot = (page_map_entries({'volumes': numbered}) if supplement_plan else
                         page_map_entries(bundle_preview(store, matter_id, max_pages, keep_groups,
                                                         output_marks, documents_override=documents)))
        export_manifest = dict(id=export_id, created_at=report['created_at'], matter_id=matter_id,
                               artifact_kind=artifact_kind,
                               variant_plan_id=variant_plan['id'] if variant_plan else None,
                               variant_plan_version=variant_plan['version'] if variant_plan else None,
                               variant_name=variant_plan['name'] if variant_plan else None,
                               variant_baseline=variant_plan['baseline'] if variant_plan else None,
                               supplement_plan_id=supplement_plan['id'] if supplement_plan else None,
                               supplement_plan_version=supplement_plan['version'] if supplement_plan else None,
                               baseline_build_id=supplement_plan['baseline_build_id'] if supplement_plan else None,
                               insertion_policy=supplement_plan['insertion_policy'] if supplement_plan else None,
                               insertion_policy_contract=(policy_preview if supplement_plan else None),
                               pages=combined_pages, volumes=len(plans),
                               matter_updated_at=matter['updated_at'], input_revisions=input_revisions,
                               label_inputs=label_inputs, bundle_membership=membership_snapshot,
                               signature_inputs=signature_inputs,
                               signature_risk_acknowledged=bool(signature_risks and signature_acknowledged),
                               build_input_digest=build_input_digest,
                               effective_input_digest=(build_input_digest if supplement_plan or variant_plan else store.build_input_digest(matter_id, effective=True)),
                               redaction_inputs=[r for d in source_documents for r in store.list_redactions(d['id'])
                                                 if not supplement_plan or (d['id'], r['page_index']) in selected_coordinates],
                               validation_profile=profile,
                               validation_contract={
                                   **{v['filename']: v['content_summary']['validation'] for v in volume_rows},
                                   **{p['filename']: p['validation'] for p in part_extracts},
                                   'combined.pdf': {'pages': offset, 'toc': global_toc,
                                       'links': {str(k): v for k,v in global_links.items()},
                                       'labels': combined_labels, 'printed_labels':combined_printed, 'label_regions':combined_regions, 'sources': combined_sources}},
                               page_map_snapshot=page_snapshot,
                               part_extracts=part_extracts,
                               build_settings=build_settings, manual_bookmark_inputs=bookmark_inputs,
                               reviewed_link_inputs=reviewed_link_inputs,
                               page_annotation_inputs=annotation_inputs,
                               output_marks=output_marks, output_preset=output_preset,
                               processing=processing, office_review_files=office_files,
                               filing_review=filing_review,
                               output_mark_timestamp=mark_at_iso,
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
        if supplement_plan and supplement_plan.get('input_snapshot') != store.build_input_digest(matter_id):
            raise ProcessingError('Supplement inputs changed during assembly; no export was published')
        if variant_plan and not variants.is_current(store, matter_id, variant_plan)[0]:
            raise ProcessingError('Variant inputs changed during assembly; no export was published')
        current_contract = (plan_contract(supplement_plan) if supplement_plan else
                            variants.plan_contract(variant_plan) if variant_plan else store.build_input_digest(matter_id))
        if current_contract != build_input_digest:
            raise ProcessingError('Build inputs or profile changed during assembly; no export was published')
        final_dir = export_root / export_id
        if final_dir.exists():
            raise ProcessingError('Export id collision')
        for artifact in staging.iterdir():
            with artifact.open('rb') as stream:
                os.fsync(stream.fileno())
        # Atomic publish on the same filesystem; staged files vanish on failures.
        os.replace(staging, final_dir)
    from .validation import validate_output
    progress(97, 'Checking saved output files')
    validate_output(store, matter_id, export_id)
    progress(100, 'Bundle ready for review')
    return export_manifest
