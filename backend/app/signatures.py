"""Conservative PDF signature-indicator inspection.

This module does not validate certificates, identities, trust chains, signing
times, revocation, or whether a signature covers the current bytes. It only
records structural PDF indicators so callers can warn before transformations.
"""
from __future__ import annotations

import re
from pathlib import Path

import pymupdf

from .file_io import open_pdf

CERTIFICATE_SIGNATURE_DETECTED = 'certificate_signature_detected'
NO_CERTIFICATE_SIGNATURE_DETECTED = 'no_certificate_signature_detected'
SIGNATURE_INSPECTION_INCONCLUSIVE = 'inspection_inconclusive'
VISIBLE_SIGNATURE_ONLY = 'visible_signature_only'
UNSIGNED_REVIEWED = 'unsigned'

_SIGNATURE_TYPE = re.compile(r'/Type\s*/Sig(?:\s|/|>>|$)')
_BYTE_RANGE = re.compile(r'/ByteRange\s*\[')
_CONTENTS = re.compile(r'/Contents\s*(?:<|\()')


def combined_signature_status(certificate_status: str, visible_status: str) -> str:
    if certificate_status == 'detected':
        return CERTIFICATE_SIGNATURE_DETECTED
    if certificate_status == 'unknown':
        return SIGNATURE_INSPECTION_INCONCLUSIVE
    if visible_status == 'present':
        return VISIBLE_SIGNATURE_ONLY
    if visible_status == 'absent':
        return UNSIGNED_REVIEWED
    return NO_CERTIFICATE_SIGNATURE_DETECTED


def inspect_source_signature(path: Path, suffix: str) -> dict:
    """Return certificate-signature indicators for an immutable import.

    Visible handwriting or a scanned autograph cannot be determined reliably
    from generic PDF/image structure, so that remains a separate reviewer field.
    """
    suffix = suffix.lower()
    if suffix != '.pdf':
        certificate_status = 'not_detected'
        note = ('The input is an image, not a PDF certificate container. '
                'Any visible or scanned signature still requires reviewer classification.')
        return {
            'certificate_signature_status': certificate_status,
            'certificate_signature_count': 0,
            'signature_inspection_note': note,
            'visible_signature_status': 'unreviewed',
            'signature_status': combined_signature_status(certificate_status, 'unreviewed'),
        }

    try:
        with open_pdf(path) as document:
            if document.needs_pass:
                raise ValueError('password-protected PDF')
            strong_xrefs: set[int] = set()
            suspicious = False
            read_failed = False

            # Signature fields may point at a signature dictionary through /V.
            for page in document:
                try:
                    widgets = page.widgets(types=[pymupdf.PDF_WIDGET_TYPE_SIGNATURE]) or ()
                    for widget in widgets:
                        value_type, value = document.xref_get_key(widget.xref, 'V')
                        if value_type == 'xref':
                            try:
                                value_xref = int(value.split()[0])
                                raw = document.xref_object(value_xref)
                                if _BYTE_RANGE.search(raw) and _CONTENTS.search(raw):
                                    strong_xrefs.add(value_xref)
                                else:
                                    suspicious = True
                            except (ValueError, RuntimeError):
                                suspicious = True
                        elif value_type not in ('null', 'none') and value.strip() not in ('', 'null'):
                            # A populated direct value is unusual. Do not call it
                            # unsigned merely because this parser cannot resolve it.
                            suspicious = True
                except (RuntimeError, ValueError):
                    read_failed = True

            # Document timestamp signatures and certification signatures are not
            # required to have a visible widget. Inspect signature dictionaries.
            if document.xref_length() > 200_000:
                read_failed = True
            else:
                for xref in range(1, document.xref_length()):
                    try:
                        raw = document.xref_object(xref)
                    except (RuntimeError, ValueError):
                        read_failed = True
                        continue
                    if not _SIGNATURE_TYPE.search(raw):
                        continue
                    if _BYTE_RANGE.search(raw) and _CONTENTS.search(raw):
                        strong_xrefs.add(xref)
                    else:
                        suspicious = True

            if strong_xrefs:
                certificate_status = 'detected'
                note = (f'Found {len(strong_xrefs)} PDF certificate-signature indicator '
                        f'{"dictionary" if len(strong_xrefs) == 1 else "dictionaries"}. '
                        'Cryptographic validity and coverage were not checked.')
            elif suspicious or read_failed:
                certificate_status = 'unknown'
                note = ('PDF signature inspection was inconclusive. No cryptographic '
                        'validation was attempted; treat transformations cautiously.')
            else:
                certificate_status = 'not_detected'
                note = ('No PDF certificate-signature dictionary with ByteRange and Contents '
                        'indicators was detected. This does not rule out a visible or scanned signature.')
    except Exception as exc:
        certificate_status = 'unknown'
        strong_xrefs = set()
        note = (f'PDF signature inspection was inconclusive ({type(exc).__name__}). '
                'No cryptographic validation was attempted.')

    visible_status = 'unreviewed'
    return {
        'certificate_signature_status': certificate_status,
        'certificate_signature_count': len(strong_xrefs),
        'signature_inspection_note': note,
        'visible_signature_status': visible_status,
        'signature_status': combined_signature_status(certificate_status, visible_status),
    }


def transformation_risk(document: dict) -> bool:
    return document.get('signature_status') in {
        CERTIFICATE_SIGNATURE_DETECTED,
        SIGNATURE_INSPECTION_INCONCLUSIVE,
        VISIBLE_SIGNATURE_ONLY,
    }


def risk_summary(documents: list[dict]) -> list[dict]:
    return [
        {'document_id': document['id'], 'title': document['title'],
         'signature_status': document.get('signature_status', SIGNATURE_INSPECTION_INCONCLUSIVE)}
        for document in documents if transformation_risk(document)
    ]
