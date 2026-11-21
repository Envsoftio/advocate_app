"""Exercise the protected Rust store and Python/Rust encrypted format compatibility."""
from contextlib import contextmanager
import hashlib
import json
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
from zipfile import ZipFile

import pymupdf
from fastapi.testclient import TestClient

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'backend'))

from app.portable import parse_protected_package_stream, protected_package_header
from app.main import create_app
from app.secure_db import connect_encrypted
from app.vault import EncryptedReader, create_key_envelope, decrypt_bytes, encrypt_bytes, unlock_key

EXAMPLE = ROOT / 'rust-backend/target/debug/examples/vault_contract'
PASSWORD = 'Synthetic vault α password'
NEW_PASSWORD = 'Synthetic rotated vault β password'
SAMPLES = ROOT / 'samples/rust_pdf_corpus'


def formats(root):
    root.mkdir()
    plain = b'Python/Rust synthetic stream ' * 81000
    (root / 'plain.bin').write_bytes(plain)
    envelope, recovery, key = create_key_envelope(PASSWORD)
    (root / 'python.json').write_text(json.dumps(dict(password=PASSWORD, recovery=recovery, envelope=envelope)))
    encrypt_bytes(plain, root / 'inner.bin', key)
    encrypt_bytes(protected_package_header(envelope) + (root / 'inner.bin').read_bytes(), root / 'nested.bin', key)
    with ZipFile(root / 'plain.zip', 'w') as package:
        package.writestr('manifest.json', b'{"testing":true}')
        package.writestr('working/synthetic.pdf', plain)
    with connect_encrypted(root / 'python.sqlite3', key) as python_database:
        python_database.execute('CREATE TABLE sample (name TEXT)')
        python_database.execute('INSERT INTO sample VALUES (?)', ('Python synthetic row',))
    with sqlite3.connect(root / 'legacy.sqlite3') as legacy:
        legacy.execute('PRAGMA journal_mode=WAL')
        legacy.execute('PRAGMA user_version=6')
        legacy.execute('CREATE TABLE sample (name TEXT)')
        legacy.execute('INSERT INTO sample VALUES (?)', ('Legacy committed WAL row',))
        legacy.commit()
        result = json.loads(subprocess.check_output([EXAMPLE, 'formats', root]))
        assert legacy.execute('SELECT name FROM sample').fetchone() == ('Legacy committed WAL row',)
    rust_key = unlock_key(result['envelope'], PASSWORD)
    assert unlock_key(result['envelope'], result['recovery'], recovery=True) == rust_key
    assert decrypt_bytes(root / 'rust.bin', rust_key) == plain
    with (root / 'rust.cbmatter').open('rb') as stream:
        rust_envelope, offset = parse_protected_package_stream(stream)
        assert rust_envelope == result['envelope']
        with EncryptedReader(stream, rust_key, offset=offset) as decoded, ZipFile(decoded) as archive:
            assert archive.read('manifest.json') == b'{"testing":true}'
            assert archive.read('working/synthetic.pdf') == plain
    for database, expected in [('rust.sqlite3', 'Rust synthetic row'), ('migrated.sqlite3', 'Legacy committed WAL row')]:
        assert not (root / database).read_bytes().startswith(b'SQLite format 3\x00')
        with connect_encrypted(root / database, rust_key) as check:
            assert check.execute('SELECT name FROM sample').fetchone()[0] == expected
            if database == 'migrated.sqlite3':
                assert check.execute('PRAGMA user_version').fetchone()[0] == 6


@contextmanager
def running_vault(root):
    with subprocess.Popen([EXAMPLE, 'vault', root], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True) as proc:
        def request(method, path, body=None, status=200, **fields):
            proc.stdin.write(json.dumps(dict(method=method, path=path, body=body, **fields)) + '\n')
            proc.stdin.flush()
            output = proc.stdout.readline()
            assert output, 'Rust fixture process exited before sending a reply'
            response = json.loads(output)
            if status is None:
                return response
            assert response['status'] == status, (method, path, status, response)
            return response['body']

        try:
            yield request
        finally:
            proc.stdin.close()
            assert proc.wait(timeout=10) == 0


def lifecycle(root):
    with running_vault(root) as rust:
        assert rust('GET', '/vault/status') == {'state': 'setup', 'enabled': True}
        rust('GET', '/matters', status=423)
        rust('POST', '/vault/setup', {'password': 'short'}, status=422)
        created = rust('POST', '/vault/setup', {'password': PASSWORD})
        assert created['state'] == 'unlocked'
        assert subprocess.run([EXAMPLE, 'vault', root], capture_output=True, input=b'').returncode != 0
        rust('POST', '/vault/unlock', {'credential': PASSWORD}, status=403)
        mid = rust('POST', '/matters', {'name': 'Synthetic protected matter', 'court_profile': 'general-demo'}, status=201)['id']
        rust('IMPORT', mid, status=422, source=str(SAMPLES / 'encrypted.pdf'), password='wrong')
        assert rust('GET', f'/matters/{mid}/documents') == []
        signed = rust('IMPORT', mid, status=201, source=str(SAMPLES / 'certificate.pdf'))[0]
        encrypted = rust('IMPORT', mid, status=201, source=str(SAMPLES / 'encrypted.pdf'), password='synthetic-user', included=True)[0]
        assert signed['certificate_signature_status'] == 'detected'
        rust('POST', f'/documents/{signed["id"]}/pages/rotate',
             {'page_index': 0, 'degrees': 90, 'expected_revision': 1}, status=409)
        assert encrypted['bundle_included'] == 1
        encrypted_path = f"/documents/{encrypted['id']}"
        info = rust('GET', f'{encrypted_path}/original-info')
        assert info['password_protected'] and info['page_count'] is None
        assert bytes(rust('SNAPSHOT', encrypted_path, {'kind': 'original'})['bytes']) == (SAMPLES / 'encrypted.pdf').read_bytes()
        assert signed['source_sha256'] == hashlib.sha256((SAMPLES / 'certificate.pdf').read_bytes()).hexdigest()
        rust('DELETE', f'/documents/{signed["id"]}')
        assert rust('GET', f'/matters/{mid}/archived-documents')[0]['id'] == signed['id']
        key = unlock_key(json.loads((root / 'vault.json').read_bytes())['envelope'], created['recovery_key'], recovery=True)
        rotated = rust('POST', '/vault/rotate', {'new_password': NEW_PASSWORD})
    with running_vault(root) as rust:
        assert rust('GET', '/vault/status') == {'state': 'locked', 'enabled': True}
        rust('POST', '/vault/unlock', {'credential': PASSWORD}, status=403)
        rust('POST', '/vault/unlock', {'credential': created['recovery_key'], 'recovery': True}, status=403)
        rust('POST', '/vault/unlock', {'credential': rotated['recovery_key'], 'recovery': True})
        assert rust('GET', f'/matters/{mid}/documents') == [encrypted]
        rust('POST', f'/matters/{mid}/archived-documents/{signed["id"]}/restore')
        assert len(rust('GET', f'/matters/{mid}/documents')) == 2
        for document in (signed, encrypted):
            files = root / 'documents-v1' / document['id']
            original = decrypt_bytes(files / 'original.pdf', key)
            working = decrypt_bytes(files / 'revision-1.pdf', key)
            assert original == (SAMPLES / document['original_name']).read_bytes()
            assert original.startswith(b'%PDF') and working.startswith(b'%PDF')
            assert (working == original) == (document == signed)
            assert all((files / name).read_bytes().startswith(b'CBENC1\n') for name in ('original.pdf', 'revision-1.pdf'))
        damaged = root / 'documents-v1' / encrypted['id'] / 'revision-1.pdf'
        damaged.write_bytes(damaged.read_bytes()[:-1])
        rust('GET', f'/matters/{mid}/documents', status=409)
    with connect_encrypted(root / 'rust-matters.sqlite3', key) as database:
        assert database.execute('SELECT count(*) FROM documents').fetchone()[0] == 2
    assert b'Synthetic protected matter' not in (root / 'rust-matters.sqlite3').read_bytes()


def revision_fixture(path):
    with pymupdf.open() as pdf:
        first = pdf.new_page(width=400, height=520)
        first.insert_text((75, 80), 'Synthetic rotation source')
        first.set_cropbox(pymupdf.Rect(20, 20, 380, 500))
        first.set_rotation(90)
        pdf.new_page().insert_text((75, 80), 'Synthetic retained destination')
        pdf[0].insert_link({'kind': pymupdf.LINK_GOTO, 'from': pymupdf.Rect(50, 90, 170, 115), 'page': 1})
        pdf.set_toc([[1, 'Source display title', 1], [1, 'Destination', 2]])
        pdf.set_page_labels([{'startpage': 0, 'prefix': 'A-', 'style': 'D', 'firstpagenum': 1}])
        pdf.save(path)


def read_current(path, key, document_id):
    with connect_encrypted(path / 'rust-matters.sqlite3', key) as database:
        relative = database.execute('SELECT working_relpath FROM documents WHERE id=?', (document_id,)).fetchone()[0]
    return decrypt_bytes(path / relative, key)


def pdf_semantics(raw):
    with pymupdf.open(stream=raw, filetype='pdf') as pdf:
        return {
            'toc': pdf.get_toc(),
            'pages': [(page.get_text(), page.rotation, tuple(page.cropbox), page.get_label(),
                       [link['page'] for link in page.get_links() if link['kind'] == pymupdf.LINK_GOTO],
                       hashlib.sha256(page.get_pixmap(dpi=36).samples).hexdigest()) for page in pdf],
        }


def revisions(root):
    root.mkdir()
    source = root / 'rotation.pdf'
    revision_fixture(source)
    raw = source.read_bytes()
    rust_root = root / 'rust'
    token = 'vault-revision-regression'
    auth = {'Authorization': f'Bearer {token}'}
    with running_vault(rust_root) as rust, TestClient(create_app(root / 'python', token, require_vault=True)) as python:
        setup = rust('POST', '/vault/setup', {'password': PASSWORD})
        key = unlock_key(json.loads((rust_root / 'vault.json').read_bytes())['envelope'], setup['recovery_key'], recovery=True)
        python.post('/vault/setup', headers=auth, json={'password': PASSWORD}).raise_for_status()
        metadata = {'name': 'Synthetic retained working revisions', 'court_profile': 'general-demo'}
        rust_matter = rust('POST', '/matters', metadata, status=201)['id']
        python_matter = python.post('/matters', headers=auth, json=metadata).json()['id']
        rust_id = rust('IMPORT', rust_matter, status=201, source=str(source))[0]['id']
        response = python.post(f'/matters/{python_matter}/documents/raw', content=raw, headers={**auth,
            'Content-Type': 'application/octet-stream', 'X-CourtBundle-Filename': source.name})
        response.raise_for_status()
        python_id = response.json()[0]['id']

        def current_pair(revision):
            rust_history = rust('GET', f'/documents/{rust_id}/revisions')
            python_history = python.get(f'/documents/{python_id}/revisions', headers=auth).json()
            assert [item['revision'] for item in rust_history] == list(range(revision, 0, -1))
            comparable = ('revision', 'page_count', 'available', 'current')
            assert [[item[k] for k in comparable] for item in rust_history] == [
                [item[k] for k in comparable] for item in python_history]
            python_store = python.app.state.store
            from app.file_io import read_bytes
            expected_pdf = read_bytes(python_store.working_path(python_store.get_document(python_id)))
            actual_pdf = read_current(rust_root, key, rust_id)
            assert pdf_semantics(actual_pdf) == pdf_semantics(expected_pdf), (revision, pdf_semantics(actual_pdf), pdf_semantics(expected_pdf))
            return actual_pdf

        assert current_pair(1) == raw
        rust_route = f'/documents/{rust_id}/pages/rotate'
        python_route = f'/documents/{python_id}/pages/rotate'
        rotation = {'expected_revision': 1, 'page_index': 0, 'degrees': 180}
        assert rust('POST', rust_route, rotation)['revision'] == 2
        python.post(python_route, headers=auth, json=rotation).raise_for_status()
        rotated = current_pair(2)
        rust('POST', rust_route, rotation, status=409)
        assert python.post(python_route, headers=auth, json=rotation).status_code == 409
        for operation, revision in [('undo', 3), ('redo', 4)]:
            rust_state = rust('POST', f'/matters/{rust_matter}/history/{operation}')
            python_state = python.post(f'/matters/{python_matter}/history/{operation}', headers=auth)
            assert rust_state == python_state.json()
            assert current_pair(revision) == (raw if operation == 'undo' else rotated)
        restore = {'expected_revision': 4, 'expected_sha256': hashlib.sha256(raw).hexdigest()}
        assert rust('POST', f'/documents/{rust_id}/revisions/1/restore', restore)['revision'] == 5
        python.post(f'/documents/{python_id}/revisions/1/restore', headers=auth, json=restore).raise_for_status()
        assert current_pair(5) == raw
        for number in (1, 2, 5):
            expected = next(item for item in python.get(f'/documents/{python_id}/revisions', headers=auth).json()
                            if item['revision'] == number)
            actual = next(item for item in rust('GET', f'/documents/{rust_id}/revisions')
                          if item['revision'] == number)
            py_pdf = python.get(f'/documents/{python_id}/revisions/{number}/pdf',
                                params={'sha256': expected['sha256']}, headers=auth)
            assert py_pdf.status_code == 200
            rust_pdf = rust('SNAPSHOT', f'/documents/{rust_id}',
                            {'kind': 'retained', 'revision': number, 'sha256': actual['sha256']})
            assert pdf_semantics(bytes(rust_pdf['bytes'])) == pdf_semantics(py_pdf.content)
        assert decrypt_bytes(rust_root / 'documents-v1' / rust_id / 'original.pdf', key) == raw

        second = ROOT / 'samples/02_impugned_order.pdf'
        rust_second = rust('IMPORT', rust_matter, status=201, source=str(second))[0]['id']
        python_second = python.post(f'/matters/{python_matter}/documents/raw', content=second.read_bytes(),
            headers={**auth, 'Content-Type': 'application/octet-stream',
                     'X-CourtBundle-Filename': second.name}).json()[0]['id']
        py_documents = f'/matters/{python_matter}/documents'
        rust_documents = f'/matters/{rust_matter}/documents'
        py_order = [python_second, python_id]
        rust_order = [rust_second, rust_id]
        for left, right in [(py_order[:1], rust_order[:1]), ([python_id] * 2, [rust_id] * 2)]:
            bad = python.post(f'{py_documents}/reorder', headers=auth, json={'ids': left})
            assert bad.status_code == 422
            assert rust('POST', f'{rust_documents}/reorder', {'ids': right}, status=422) == bad.json()
        previous = None
        for _ in range(2):
            py_docs = python.post(f'{py_documents}/reorder', headers=auth, json={'ids': py_order})
            py_docs.raise_for_status()
            rust_docs = rust('POST', f'{rust_documents}/reorder', {'ids': rust_order})
            assert [doc['id'] for doc in rust_docs] == rust_order
            keys = ('title', 'position', 'revision', 'source_sha256')
            assert [[doc[k] for k in keys] for doc in rust_docs] == [
                [doc[k] for k in keys] for doc in py_docs.json()]
            current = (rust_docs, py_docs.json())
            if previous is not None:
                assert current == previous
            previous = current
        for direction, ordered in [('undo', rust_order[::-1]), ('redo', rust_order)]:
            expected = python.post(f'/matters/{python_matter}/history/{direction}', headers=auth)
            assert expected.status_code == 200
            assert rust('POST', f'/matters/{rust_matter}/history/{direction}') == expected.json()
            assert [doc['id'] for doc in rust('GET', rust_documents)] == ordered
        assert read_current(rust_root, key, rust_id) == raw
        assert decrypt_bytes(rust_root / 'documents-v1' / rust_id / 'original.pdf', key) == raw
    with running_vault(rust_root) as rust:
        rust('POST', '/vault/unlock', {'credential': PASSWORD})
        assert rust('GET', f'/documents/{rust_id}/revisions')[0]['revision'] == 5


if __name__ == '__main__':
    subprocess.run(['cargo', 'build', '--quiet', '--locked', '--offline', '--manifest-path',
                    str(ROOT / 'rust-backend/Cargo.toml'), '--example', 'vault_contract'], check=True)
    with tempfile.TemporaryDirectory(prefix='matterdraft-vault-contract-') as directory:
        root = Path(directory)
        formats(root / 'formats')
        lifecycle(root / 'vault')
        revisions(root / 'revisions')
    print('Python/Rust vault formats, SQLCipher migration, protected PDF lifecycle and working-revision parity passed')
