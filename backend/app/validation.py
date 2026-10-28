"""Build-bound QA evidence. Review records never mutate immutable build files.

Callers hold Store.matter_lock during reads/writes/checks. Inspection is separate
from download: failed/historical PDFs stay readable, never implicitly released.
"""
from __future__ import annotations
from contextlib import ExitStack
import csv
import hashlib
import io
import json
import os
import zipfile
from pathlib import Path
import pymupdf
from .file_io import read_bytes, write_bytes, open_pdf, size, protected
from .storage import utcnow, new_id
from .vault import EncryptedReader

REVIEW_ITEMS = {
    'navigation': 'Index links and bookmarks',
    'pagination': 'Printed page references',
    'coverage': 'Document order and coverage',
    'readability': 'Scan and image readability',
}
CHECKS = {
    'files': 'Files reopen successfully',
    'counts': 'PDF counts match the page map',
    'navigation': 'Index links and bookmarks',
    'labels': 'Printed and viewer page labels',
    'sizes': 'Volume file-size limits',
    'cleanup': 'Export cleanup',
    'redaction': 'Redaction verification',
}


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def _folder(store, matter_id, build_id):
    if build_id != 'preflight' and (len(build_id) != 32 or any(c not in '0123456789abcdef' for c in build_id)):
        raise ValueError('Invalid build identity')
    return store.matter_dir(matter_id) / 'validation' / build_id


def _write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name('.' + new_id() + '.tmp')
    try:
        write_bytes(temporary, json.dumps(value, ensure_ascii=False).encode())
        with temporary.open('rb') as stream:
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def history(store, matter_id, build_id):
    folder = _folder(store, matter_id, build_id)
    records = [json.loads(read_bytes(p)) for p in folder.glob('run-*.json')]
    return sorted(records, key=lambda r: r['created_at'], reverse=True)


def manifest_for(store, matter_id, build_id):
    item = next((m for m in store.list_exports(matter_id) if m.get('id') == build_id), None)
    if not item:
        raise ValueError('Saved build not found')
    return item


def record_preflight(store, matter_id, findings):
    try:
        snapshot = store.build_input_digest(matter_id, effective=True)
    except (OSError, ValueError):
        snapshot = None
    contract = {'input_digest': snapshot, 'revisions': [
        {'document_id': d['id'], 'revision': d['revision'], 'pages': d['page_count']}
        for d in store.get_bundle_documents(matter_id)],
        'settings': store.get_output_marks(matter_id)}
    run = {'id': new_id(), 'matter_id': matter_id, 'build_id': None,
           'scope': 'preflight', 'created_at': utcnow(), 'contract': contract,
           'findings': findings}
    for finding in findings:
        finding['validation_run_id'] = run['id']
        finding['effective_input_digest'] = snapshot
        finding['blocks_build'] = finding['severity'] == 'error'
    _write(_folder(store, matter_id, 'preflight') / ('run-' + run['id'] + '.json'), run)
    return findings


def validate_output(store, matter_id, build_id, selected_files=None):
    manifest = manifest_for(store, matter_id, build_id)
    by_name = {f['name']: f for f in manifest['files']}
    selected = sorted(by_name if selected_files is None else selected_files)
    if not selected or len(set(selected)) != len(selected) or any(n not in by_name for n in selected):
        raise ValueError('Choose existing, distinct build files')
    contracts = manifest.get('validation_contract', {})
    measured, pdfs, errors = {}, {}, []
    expected = {n: by_name[n] for n in selected}
    for name in selected:
        path = store.get_export_file(matter_id, build_id, name)
        if not path:
            measured[name] = {'error': 'Missing file or content digest differs from manifest'}
            errors.append(name)
            continue
        measured[name] = {'sha256': store.sha256(path), 'bytes': size(path)}
        try:
            if name.endswith('.pdf'):
                with open_pdf(path) as pdf:
                    if pdf.needs_pass or not pdf.page_count:
                        raise ValueError('Encrypted or empty PDF')
                    # Independently decode PDF text-string labels with PDFium. MuPDF's
                    # get_label returns raw escaped prefixes for non-ASCII text.
                    import pypdfium2 as pdfium
                    from .file_io import read_bytes
                    with pdfium.PdfDocument(read_bytes(path) if protected(path) else str(path)) as label_pdf:
                        decoded_labels = [label_pdf.get_page_label(i) for i in range(len(label_pdf))]
                    labels, printed, links = [], [], {}
                    for index, page in enumerate(pdf):
                        # Parse content as well as opening the document catalog.
                        page.get_text('dict')
                        labels.append(decoded_labels[index] if pdf.get_page_labels() else str(index + 1))
                        rect = page.rect
                        regions = contracts.get(name, {}).get('label_regions')
                        clip = pymupdf.Rect(regions[index]) if regions else pymupdf.Rect(24, max(0, rect.height-36), max(24, rect.width-24), rect.height-18)
                        printed.append(page.get_text('text', clip=clip).strip())
                        destinations = [link.get('page') for link in page.get_links() if link['kind'] == pymupdf.LINK_GOTO]
                        if destinations:
                            links[str(index)] = destinations
                    pdfs[name] = {'pages': pdf.page_count, 'toc': pdf.get_toc(), 'links': links,
                                  'labels': labels, 'printed': printed}
                    measured[name]['pages'] = pdf.page_count
            elif name.endswith('.zip'):
                with ExitStack() as stack:
                    source = stack.enter_context(EncryptedReader(path, store._data_key)) if protected(path) else path
                    archive = stack.enter_context(zipfile.ZipFile(source))
                    if archive.testzip() is not None:
                        raise ValueError('ZIP integrity failure')
                    for member in archive.namelist():
                        if member in by_name:
                            with archive.open(member) as stream:
                                if hashlib.file_digest(stream, 'sha256').hexdigest() != by_name[member]['sha256']:
                                    raise ValueError('ZIP member differs from saved artifact: ' + member)
                    if not set(by_name).difference({name}).issubset(archive.namelist()):
                        raise ValueError('ZIP is missing a saved artifact')
                    measured[name]['members'] = archive.namelist()
            elif name.endswith('.csv'):
                measured[name]['rows'] = len(list(csv.DictReader(io.StringIO(read_bytes(path).decode('utf-8-sig')))))
                if measured[name]['rows'] != len(manifest.get('page_map_snapshot', [])):
                    raise ValueError('CSV row count differs from page map')
            elif name.endswith('.json'):
                report = json.loads(read_bytes(path))
                if report.get('export_id') != build_id:
                    raise ValueError('Report identifies a different build')
            else:
                raise ValueError('No parser implemented for this file type')
        except Exception as exc:
            measured[name]['error'] = str(exc)
            errors.append(name)
    rows = []
    def add(key, outcome, evidence, blocking=False, findings=None):
        rows.append({'id': key, 'name': CHECKS[key], 'outcome': outcome,
                     'evidence': evidence, 'blocking': blocking, 'findings': findings or []})
    add('files', 'failed' if errors else 'passed',
        {'summary': f'{len(selected)-len(errors)} / {len(selected)} files parsed with matching SHA-256', 'files': measured}, bool(errors))
    pdf_names = [n for n in selected if n.endswith('.pdf')]
    for key in ('counts', 'navigation', 'labels'):
        failures, missing, found = [], [], []
        for name in pdf_names:
            spec, actual = contracts.get(name), pdfs.get(name)
            if not actual:
                failures.append(name + ': could not parse PDF'); continue
            if not spec:
                missing.append(name); continue
            if key == 'counts':
                if actual['pages'] != spec['pages']:
                    failures.append(f'{name}: {actual["pages"]} pages; expected {spec["pages"]}')
            elif key == 'navigation':
                if actual['toc'] != spec['toc'] or actual['links'] != spec['links']:
                    failures.append(name + ': destinations differ from saved deterministic map')
            else:
                for index, label in enumerate(spec['labels']):
                    printed_label = spec.get('printed_labels', spec['labels'])[index]
                    if index >= actual['pages'] or actual['labels'][index] != label or (printed_label and printed_label not in actual['printed'][index]):
                        failures.append(f'{name}: physical page {index+1}, expected printed/viewer label {label}')
                        target = spec.get('sources', {}).get(str(index))
                        if target:
                            found.append({'code': 'OUTPUT_LABEL_MISMATCH', 'message': failures[-1],
                                'severity': 'error', 'artifact': 'working', 'output_file': name,
                                'output_page_index': index, **target})
            measured[name][key] = 'failed' if any(f.startswith(name + ':') for f in failures) else 'passed'
        outcome = 'failed' if failures else 'not-run' if missing else 'passed' if pdf_names else 'not-applicable'
        add(key, outcome, {'summary': '; '.join(failures) or ('No recorded contract for ' + ', '.join(missing) if missing else
            f'{len(pdf_names)} PDFs checked' if pdf_names else 'Selection contains no PDFs'), 'files': pdf_names}, outcome in ('failed','not-run'), found)
    omissions = [c['message'] for c in manifest.get('checks', []) if c.get('code') in {
        'UNRESOLVED_MANUAL_BOOKMARKS', 'EXCLUDED_BOOKMARK_OMITTED', 'UNRESOLVED_REVIEWED_LINKS'}]
    navigation = next(c for c in rows if c['id'] == 'navigation')
    if omissions and navigation['outcome'] == 'passed':
        navigation['outcome'] = 'warning'
        navigation['evidence']['summary'] = 'Recorded destinations match; omitted targets need review'
        navigation['evidence']['omissions'] = omissions
    limit = manifest.get('validation_profile', {}).get('max_file_mb')
    too_large = [n for n in selected if limit and measured[n].get('bytes', 0) > limit * 1024 * 1024]
    add('sizes', 'failed' if too_large else 'passed' if limit else 'not-applicable',
        {'summary': f'{limit} MB limit; {len(too_large)} exceed it' if limit else 'No byte limit configured in saved profile',
         'limit_mb': limit, 'bytes': {n: measured[n].get('bytes') for n in selected}}, bool(too_large))
    add('cleanup', 'not-run', {'summary': 'Optional general export cleanup is not implemented; not selected for this build'})
    redactions = manifest.get('redaction_inputs', []) + [a for a in manifest.get('page_annotation_inputs', []) if a.get('kind') == 'redaction']
    add('redaction', 'not-run' if redactions else 'not-applicable',
        {'summary': 'Content removal needs independent verification; a visible black mark is not proof' if redactions else
         'No recorded redaction operations in this build; this does not certify imported content.'}, bool(redactions))
    run = {'id': new_id(), 'matter_id': matter_id, 'build_id': build_id, 'scope': 'output',
           'created_at': utcnow(), 'input_digest': manifest.get('effective_input_digest'),
           'input_revisions': manifest.get('input_revisions', []), 'settings': {'build': manifest.get('build_settings'),
               'output_marks': manifest.get('output_marks'), 'profile': manifest.get('validation_profile'), 'preset': manifest.get('output_preset')},
           'selected_files': selected, 'artifact_digest': digest(manifest['files']),
           'expected_files': expected, 'measured_files': measured, 'checks': rows}
    for row in rows:
        for finding in row['findings']:
            finding['finding_id'] = digest([run['id'], row['id'], finding])[:24]
            finding['validation_run_id'] = run['id']
    _write(_folder(store, matter_id, build_id) / ('run-' + run['id'] + '.json'), run)
    return run


def review_state(store, matter_id, build_id, manifest=None):
    manifest = manifest or manifest_for(store, matter_id, build_id)
    path = _folder(store, matter_id, build_id) / 'review.json'
    return json.loads(read_bytes(path)) if path.exists() else {
        'version': 0, 'build_id': build_id, 'artifact_digest': digest(manifest['files']),
        'items': {key: False for key in REVIEW_ITEMS}, 'updated_at': None}


def save_review(store, matter_id, build_id, payload):
    manifest = manifest_for(store, matter_id, build_id)
    current = review_state(store, matter_id, build_id, manifest)
    if payload.get('artifact_digest') != digest(manifest['files']) or payload.get('version') != current['version']:
        raise ValueError('Review changed or artifact identity differs; reload before saving')
    items = payload.get('items')
    if not isinstance(items, dict) or set(items) != set(REVIEW_ITEMS) or any(type(v) is not bool for v in items.values()):
        raise ValueError('Supply all four human-review items as booleans')
    result = {**current, 'items': items, 'version': current['version'] + 1, 'updated_at': utcnow()}
    _write(_folder(store, matter_id, build_id) / 'review.json', result)
    return result


def status(store, matter_id, build_id):
    manifest = manifest_for(store, matter_id, build_id)
    runs = history(store, matter_id, build_id)
    # A later selection-only run cannot replace the latest complete assessment.
    run = next((r for r in runs if set(r['selected_files']) == {f['name'] for f in manifest['files']}), None)
    review = review_state(store, matter_id, build_id, manifest)
    reasons = []
    if not run or run['artifact_digest'] != digest(manifest['files']):
        reasons.append('Run checks for all saved output files')
    else:
        reasons.extend(c['name'] + ': ' + c['outcome'] for c in run['checks']
                       if c['outcome'] == 'failed' or (c['blocking'] and c['outcome'] == 'not-run' and c['id'] != 'redaction'))
    if any(not store.get_export_file(matter_id, build_id, f['name']) for f in manifest['files']):
        reasons.append('Saved artifact bytes no longer match the manifest')
    technical_ready = not reasons
    release_reasons = reasons[:]
    if run:
        release_reasons.extend(c['name'] + ': ' + c['outcome'] for c in run['checks'] if c['id'] == 'redaction' and c['blocking'])
    if manifest['stale']:
        release_reasons.append('Current effective inputs differ; this is a historical build')
    if review['artifact_digest'] != digest(manifest['files']) or not all(review['items'].values()):
        release_reasons.append('Human review is incomplete for this build')
    return {'build_id': build_id, 'run': run, 'history': runs, 'review': review,
            'review_labels': REVIEW_ITEMS, 'stale': manifest['stale'],
            'can_download': technical_ready, 'can_release': not release_reasons,
            'download_reasons': reasons, 'release_reasons': release_reasons}


def require_download(store, matter_id, build_id, release=False):
    state = status(store, matter_id, build_id)
    if not state['can_release' if release else 'can_download']:
        raise ValueError('; '.join(state['release_reasons' if release else 'download_reasons']))


def finding_record(store, matter_id, run_id, finding_id):
    if any(not value or len(value) > 64 or any(c not in '0123456789abcdef' for c in value) for value in (run_id, finding_id)):
        raise ValueError('Invalid finding identity')
    paths = list((store.matter_dir(matter_id) / 'validation').glob('*/run-' + run_id + '.json'))
    if len(paths) != 1:
        raise ValueError('Finding run is unavailable')
    run = json.loads(read_bytes(paths[0]))
    findings = run.get('findings', []) + [f for check in run.get('checks', []) for f in check.get('findings', [])]
    finding = next((f for f in findings if f.get('finding_id') == finding_id), None)
    if not finding:
        raise ValueError('Finding is not part of this run')
    doc = store.get_document(finding.get('document_id')) if finding.get('document_id') else None
    applicable = bool(doc and doc['matter_id'] == matter_id and not doc.get('archived_at') and
                      finding.get('target_revision') == doc['revision'] and
                      (finding.get('page_index') is None or 0 <= finding['page_index'] < doc['page_count']))
    path = paths[0].parent / ('note-' + run_id + '-' + finding_id + '.json')
    note = json.loads(read_bytes(path)) if path.exists() else {'note': '', 'acknowledged': False}
    return {'finding': finding, 'applicable': applicable, 'review': note}, path
