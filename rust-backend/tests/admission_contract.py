"""Compare the Rust preparation core with live Python imports of synthetic PDFs."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from urllib.parse import quote

import pymupdf
from fastapi.testclient import TestClient

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'backend'))

from app.file_io import read_bytes
from app.main import create_app

AUTH = {'Authorization': 'Bearer synthetic-admission-parity'}
USER_PASSWORD = 'synthetic input password'
OWNER_PASSWORD = 'synthetic owner password'
CORPUS = ROOT / 'samples/rust_pdf_corpus'


def encrypted_fixture(destination, *, signature=None, permissions=4095, user=USER_PASSWORD,
                      algorithm=pymupdf.PDF_ENCRYPT_AES_256, blank_widget=False):
    with pymupdf.open(ROOT / 'samples/01_petition.pdf') as pdf:
        pdf.set_toc([[1, 'Synthetic outline', 2]])
        pdf.set_page_labels([{'startpage': 0, 'prefix': 'A-', 'style': 'D', 'firstpagenum': 1}])
        pdf[0].insert_link({'kind': pymupdf.LINK_GOTO, 'from': pymupdf.Rect(20, 20, 60, 40), 'page': 2})
        pdf[2].set_rotation(90)
        if signature is not None:
            pdf.update_object(pdf.get_new_xref(), signature)
        if blank_widget:
            add_unsigned_widget(pdf[0])
        pdf.save(destination, encryption=algorithm, owner_pw=OWNER_PASSWORD,
                 user_pw=user, permissions=permissions)


def add_unsigned_widget(page):
    widget = pymupdf.Widget()
    widget.field_name = 'Synthetic empty signature field'
    widget.field_type = pymupdf.PDF_WIDGET_TYPE_SIGNATURE
    widget.rect = pymupdf.Rect(20, 45, 160, 78)
    return page.add_widget(widget).xref


def describe_pdf(content):
    with pymupdf.open(stream=content, filetype='pdf') as pdf:
        assert not pdf.needs_pass
        return {
            'toc': pdf.get_toc(),
            'pages': [
                {
                    'text': page.get_text(),
                    'rotation': page.rotation,
                    'bounds': tuple(page.rect),
                    'cropbox': tuple(page.cropbox),
                    'label': page.get_label(),
                    'links': [{key: value for key, value in link.items() if key not in {'xref', 'id'}}
                              for link in page.get_links()],
                    'pixels': hashlib.sha256(page.get_pixmap(dpi=36).samples).hexdigest(),
                }
                for page in pdf
            ],
        }


def main(root):
    encrypted = root / 'encrypted-navigation.pdf'
    encrypted_fixture(encrypted)
    restricted = root / 'encrypted-restricted.pdf'
    encrypted_fixture(restricted, permissions=pymupdf.PDF_PERM_PRINT)
    ambiguous = root / 'encrypted-ambiguous.pdf'
    encrypted_fixture(ambiguous, signature='<< /Type /Sig /Contents <00> >>')
    detected = root / 'encrypted-indicator.pdf'
    encrypted_fixture(detected, signature='<< /Type /Sig /ByteRange [0 100 200 300] /Contents <01020304> >>')
    empty_password = root / 'encrypted-empty-password.pdf'
    encrypted_fixture(empty_password, user='')
    unsigned_form = root / 'unsigned-signature-widget.pdf'
    with pymupdf.open(CORPUS / 'unsigned.pdf') as pdf:
        add_unsigned_widget(pdf[0])
        assert pdf.get_sigflags() == 3
        pdf.save(unsigned_form)
    encrypted_form = root / 'encrypted-unsigned-signature-widget.pdf'
    encrypted_fixture(encrypted_form, blank_widget=True)
    inherited_forms = []
    for suffix, value in (('empty', 'null'), ('unresolved', '<< /Contents (unresolved) >>')):
        inherited = root / f'inherited-signature-field-{suffix}.pdf'
        with pymupdf.open(CORPUS / 'unsigned.pdf') as pdf:
            widget = add_unsigned_widget(pdf[0])
            field = pdf.get_new_xref()
            pdf.update_object(field, f'<< /FT /Sig /T (Inherited signature) /Kids [{widget} 0 R] >>')
            pdf.xref_set_key(widget, 'FT', 'null')
            pdf.xref_set_key(widget, 'Parent', f'{field} 0 R')
            pdf.xref_set_key(widget, 'V', value)
            pdf.xref_set_key(pdf.pdf_catalog(), 'AcroForm/Fields', f'[{field} 0 R]')
            pdf.save(inherited)
        inherited_forms.append(inherited)
    cases = []

    def add(source, status, password=None, acknowledge=False, *, rewrite=False):
        cases.append(dict(source=str(source), status=status, password=password, acknowledge=acknowledge,
                          working=str(root / f'working-{len(cases)}.pdf'), rewrite=rewrite))

    for path in sorted(CORPUS.glob('*.pdf')):
        add(path, 422 if path.stem in {'encrypted', 'malformed'} else 201)
    add(CORPUS / 'encrypted.pdf', 201, 'synthetic-user', rewrite=True)
    for name in ('01_petition.pdf', '02_impugned_order.pdf', '05_hindi_scan.pdf', '06_hindi_mixed.pdf'):
        add(ROOT / 'samples' / name, 201)
    add(encrypted, 422)
    add(encrypted, 422, 'wrong password')
    add(encrypted, 201, USER_PASSWORD, rewrite=True)
    add(encrypted, 201, OWNER_PASSWORD, rewrite=True)
    add(restricted, 422, USER_PASSWORD)
    add(restricted, 201, OWNER_PASSWORD, rewrite=True)
    add(ambiguous, 409, USER_PASSWORD)
    add(ambiguous, 201, USER_PASSWORD, True, rewrite=True)
    add(detected, 409, USER_PASSWORD)
    add(detected, 201, USER_PASSWORD, True, rewrite=True)
    add(empty_password, 201)
    add(unsigned_form, 201)
    add(encrypted_form, 201, USER_PASSWORD, rewrite=True)
    for inherited in inherited_forms:
        add(inherited, 201)
    for name, algorithm in [('rc4-40', pymupdf.PDF_ENCRYPT_RC4_40),
                            ('rc4-128', pymupdf.PDF_ENCRYPT_RC4_128),
                            ('aes-128', pymupdf.PDF_ENCRYPT_AES_128)]:
        path = root / f'encrypted-{name}.pdf'
        encrypted_fixture(path, algorithm=algorithm)
        add(path, 201, USER_PASSWORD, rewrite=True)
        add(path, 201, OWNER_PASSWORD, rewrite=True)
        limited = root / f'encrypted-{name}-limited.pdf'
        encrypted_fixture(limited, algorithm=algorithm, permissions=0)
        add(limited, 422, USER_PASSWORD)
        add(limited, 201, OWNER_PASSWORD, rewrite=True)

    run = subprocess.run(
        ['cargo', 'run', '--quiet', '--offline', '--locked', '--manifest-path',
         str(ROOT / 'rust-backend/Cargo.toml'), '--example', 'admission_contract'],
        input=json.dumps(cases), text=True, check=True, capture_output=True,
    )
    with TestClient(create_app(root / 'python-store', 'synthetic-admission-parity')) as client:
        for case, rust in zip(cases, json.loads(run.stdout), strict=True):
            compare_case(client, case, rust)
    print(f'{len(cases)} live Python/Rust PDF admission cases passed')


def compare_case(client, case, rust):
    source = Path(case['source'])
    original = source.read_bytes()
    matter = client.post('/matters', headers=AUTH, json={
        'name': 'Synthetic PDF admission', 'court_profile': 'general-demo',
    })
    matter.raise_for_status()
    matter_id = matter.json()['id']
    headers = {**AUTH, 'Content-Type': 'application/octet-stream', 'X-CourtBundle-Filename': source.name}
    if case['password'] is not None:
        headers['X-CourtBundle-Pdf-Password'] = quote(case['password'], safe='')
    if case['acknowledge']:
        headers['X-CourtBundle-Signature-Acknowledge'] = 'true'
    response = client.post(f'/matters/{matter_id}/documents/raw', headers=headers, content=original)
    expected = case['status']
    context = (source.name, case['acknowledge'], response.json(), rust)
    assert response.status_code == expected == rust['status'], context
    rust_working = Path(case['working'])
    assert source.read_bytes() == original, source.name
    if expected != 201:
        assert response.json()['detail'] == rust['detail'], context
        assert not rust_working.exists(), context
        assert client.get(f'/matters/{matter_id}/documents', headers=AUTH).json() == [], context
        return
    python = response.json()[0]
    for field, value in rust['signature'].items():
        assert python[field] == value, (source.name, field, python[field], value)
    for field in ('page_count', 'ocr_status'):
        assert python[field] == rust[field], (source.name, field, python[field], rust[field])
    store = client.app.state.store
    assert read_bytes(store.original_path(python)) == original, source.name
    python_working = read_bytes(store.working_path(python))
    candidate = rust_working.read_bytes()
    if case['rewrite']:
        assert python_working != original and candidate != original, source.name
        reference = describe_pdf(python_working)
        actual = describe_pdf(candidate)
        for field in reference:
            assert reference[field] == actual[field], (source.name, field, reference[field], actual[field])
    else:
        assert python_working == original == candidate, source.name


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='matterdraft-admission-') as directory:
        main(Path(directory))
