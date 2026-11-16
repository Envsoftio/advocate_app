"""Read-only, snapshot-bound page screening. Results are candidates, never approvals."""
from __future__ import annotations
import json
import math
import pymupdf
from .file_io import open_pdf, read_bytes
from .pdf_ops import ProcessingError, inspect_ocr_page, ocr_provenance
from .storage import utcnow

METHOD = 'page-screening-v1'


def report_path(store, matter_id, snapshot):
    return store.matter_dir(matter_id) / 'validation' / 'preflight' / f'analysis-{snapshot}.json'


def current_report(store, matter_id):
    snapshot = store.build_input_digest(matter_id, effective=True)
    path = report_path(store, matter_id, snapshot)
    if not path.exists():
        return None
    report = json.loads(read_bytes(path))
    return report if report.get('method') == METHOD and report.get('input_digest') == snapshot else None


def screen_page(page):
    # Bound memory even for maliciously oversized page boxes. Render the visible
    # crop, including annotations. Any result still requires source inspection.
    scale = min(1.0, 1600 / max(page.rect.width, page.rect.height, 1))
    pix = page.get_pixmap(matrix=pymupdf.Matrix(scale, scale), colorspace=pymupdf.csGRAY, alpha=False)
    samples = pix.samples
    dark = sum(value < 245 for value in samples)
    ink = dark / max(1, len(samples))
    text = bool(page.get_text('text').strip())
    # Extractable text vetoes blank classification (including hidden OCR).
    candidate = not text and ink <= 0.0001
    return {'blank_candidate': candidate, 'ink_fraction': round(ink, 7),
            'has_text': text, 'has_images': bool(page.get_images()),
            'render_width': pix.width, 'render_height': pix.height}


def scan(store, matter_id, expected_snapshot, progress, cancelled):
    from .validation import _write
    with store.matter_lock(matter_id):
        if store.build_input_digest(matter_id, effective=True) != expected_snapshot:
            raise ProcessingError('Saved inputs changed before page screening. Run checks and start again.')
        docs = store.get_bundle_documents(matter_id)
        if not docs:
            raise ProcessingError('Include documents before screening pages')
        total = sum(d['page_count'] for d in docs)
        rows, providers = [], {}
        marked_pages = {(a['document_id'], a['page_index']) for a in store.list_page_annotations(matter_id)
                        if a.get('output_scope') == 'pdf' and a.get('resolved')}
        for doc in docs:
            runs = json.loads(doc.get('ocr_provenance_json') or '[]')
            language = runs[-1].get('language', 'eng') if runs else 'eng'
            with open_pdf(store.working_path(doc)) as pdf:
                if len(pdf) != doc['page_count']:
                    raise ProcessingError('Working page count changed; repair the document before screening')
                for index, page in enumerate(pdf):
                    if cancelled():
                        raise ProcessingError('Page screening cancelled; no new results published')
                    progress(round(95 * len(rows) / max(1, total)), f'Screening page {len(rows) + 1} of {total}')
                    row = {'document_id': doc['id'], 'target_revision': doc['revision'], 'page_index': index,
                           'language': language}
                    try:
                        row.update(screen_page(page))
                        if (doc['id'], index) in marked_pages:
                            row['blank_candidate'] = False
                        eligible = row['has_images'] or doc['ocr_status'] in ('processed', 'needs_ocr', 'no_new_text')
                        if row['blank_candidate']:
                            row['ocr_outcome'] = 'not_applicable_blank_candidate'
                        elif not eligible:
                            row['ocr_outcome'] = 'not_applicable_native_text'
                        else:
                            if language not in providers:
                                try:
                                    providers[language] = ocr_provenance(language)
                                except (OSError, RuntimeError, ProcessingError):
                                    providers[language] = {'language': language, 'provider': 'unavailable'}
                            measured = inspect_ocr_page(store.working_path(doc), index, language, cancelled=cancelled)
                            confidence = measured.get('confidence')
                            if confidence is not None and (not math.isfinite(confidence) or not 0 <= confidence <= 100):
                                raise ProcessingError('Invalid OCR confidence measurement')
                            row.update(confidence=confidence, low_region_count=len(measured['regions']))
                            row['ocr_outcome'] = ('unavailable' if not measured['available'] else
                                'no_words' if confidence is None else
                                'low_confidence' if confidence < 75 or measured['regions'] else 'measured')
                    except (OSError, RuntimeError, ValueError, ProcessingError):
                        if cancelled():
                            raise ProcessingError('Page screening cancelled; no new results published')
                        # Never persist recognized words, exception text or stale metadata
                        # into a report that could accompany a redacted artifact.
                        row['ocr_outcome'] = 'unavailable'
                    rows.append(row)
        if cancelled():
            raise ProcessingError('Page screening cancelled; no new results published')
        if store.build_input_digest(matter_id, effective=True) != expected_snapshot:
            raise ProcessingError('Saved inputs changed during screening; results were not published')
        report = {'method': METHOD, 'input_digest': expected_snapshot, 'created_at': utcnow(),
                  'pages': rows, 'providers': providers, 'threshold': 75,
                  'blank_ink_fraction_threshold': 0.0001}
        _write(report_path(store, matter_id, expected_snapshot), report)
        return {'pages_screened': len(rows), 'input_digest': expected_snapshot,
                'blank_candidates': sum(bool(r.get('blank_candidate')) for r in rows),
                'ocr_attention': sum(r['ocr_outcome'] in ('low_confidence', 'no_words', 'unavailable') for r in rows)}


def findings(store, matter_id):
    try:
        report = current_report(store, matter_id)
    except (OSError, ValueError):
        report = None
    if report is None:
        return [dict(severity='info', code='PAGE_SCREENING_NOT_RUN', document_id=None,
                     message='Blank-page and batch OCR confidence screening has not run for these saved inputs. Use Screen pages to measure candidates; this is separate from legal review.')]
    result = []
    for row in report['pages']:
        common = {key: row[key] for key in ('document_id', 'target_revision', 'page_index')}
        common.update(artifact='working', severity='warning')
        page = row['page_index'] + 1
        if row.get('blank_candidate'):
            result.append(dict(code='POSSIBLE_BLANK_PAGE',
                message=f'Source page {page} appears nearly blank in the rendered crop. Faint marks, signatures or cropped content may still exist. Review before deciding to keep or remove it; nothing was deleted.', **common))
        outcome = row['ocr_outcome']
        if outcome == 'low_confidence':
            result.append(dict(code='LOW_OCR_CONFIDENCE',
                message=f'Source page {page}: separate Tesseract pass measured {row["confidence"]}% mean word confidence with {row["low_region_count"]} flagged regions (threshold 75%). This is not text accuracy; compare the embedded text with the scan.', **common))
        elif outcome == 'no_words':
            result.append(dict(code='OCR_NO_WORDS', message=f'Source page {page}: OCR screening recognized no words. Inspect readability and language; this does not prove the page is blank.', **common))
        elif outcome == 'unavailable':
            result.append(dict(code='PAGE_SCREENING_UNAVAILABLE', message=f'Source page {page}: rendering or OCR confidence could not be measured. Inspect the source and local OCR availability, then retry screening.', **common))
    measured = sum(p['ocr_outcome'] in ('measured', 'low_confidence', 'no_words') for p in report['pages'])
    result.append(dict(severity='info', code='PAGE_SCREENING_COMPLETED', document_id=None,
        message=f'Screened {len(report["pages"])} source pages; OCR measurements available for {measured}. Native-text and possible blank pages skip OCR. Results are review aids, not verification of text, signatures or filing completeness.'))
    return result
