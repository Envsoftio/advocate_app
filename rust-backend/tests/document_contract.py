"""Synthetic Python/Rust import contract subset; run after building the Rust example."""
import json
import pathlib
import subprocess
import sys
import tempfile
from hashlib import sha256

repo = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(repo / 'backend'))
from app.main import create_app  # noqa: E402
from fastapi.testclient import TestClient  # noqa: E402

sample = (repo / 'samples/01_petition.pdf').read_bytes()
with tempfile.TemporaryDirectory(prefix='matterdraft-document-contract-') as temp:
    base = pathlib.Path(temp)
    app = create_app(base / 'python', token='synthetic', require_vault=False)
    driver = subprocess.Popen(
        [str(repo / 'rust-backend/target/debug/examples/contract'), str(base / 'rust')],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
    )

    def rust(method, path, body=None):
        driver.stdin.write(json.dumps({'method': method, 'path': path, 'body': body}) + '\n')
        driver.stdin.flush()
        return json.loads(driver.stdout.readline())

    try:
        with TestClient(app) as client:
            headers = {'Authorization': 'Bearer synthetic'}
            py_matter = client.post('/matters', json={'name': 'Synthetic PDF'}, headers=headers).json()
            rs_matter = rust('POST', '/matters', {'name': 'Synthetic PDF'})['body']
            ids = {'python': py_matter['id'], 'rust': rs_matter['id']}
            python_upload = client.post(
                f"/matters/{ids['python']}/documents/raw", content=sample,
                headers={**headers, 'Content-Type': 'application/octet-stream', 'X-CourtBundle-Filename': '01_petition.pdf'},
            )
            rust_upload = rust('IMPORT', f"/matters/{ids['rust']}/documents/raw",
                               {'filename': '01_petition.pdf', 'bytes': list(sample)})
            assert (python_upload.status_code, rust_upload['status']) == (201, 201), (python_upload.text, rust_upload)
            py_doc, rs_doc = python_upload.json()[0], rust_upload['body'][0]
            for field in ('page_count', 'source_sha256', 'original_name', 'position',
                          'revision', 'bundle_included'):
                assert py_doc[field] == rs_doc[field], (field, py_doc[field], rs_doc[field])
            assert rs_doc['source_sha256'] == sha256(sample).hexdigest()
            for engine, mid in ids.items():
                if engine == 'python':
                    get = lambda path: client.get(path, headers=headers)
                    docs = get(f'/matters/{mid}/documents')
                    usage = get(f'/matters/{mid}/storage')
                    audit = get(f'/matters/{mid}/audit')
                    assert docs.status_code == usage.status_code == audit.status_code == 200
                    assert len(docs.json()) == 1
                else:
                    docs = rust('GET', f'/matters/{mid}/documents')
                    usage = rust('GET', f'/matters/{mid}/storage')
                    audit = rust('GET', f'/matters/{mid}/audit')
                    assert docs['status'] == usage['status'] == audit['status'] == 200
                    assert len(docs['body']) == 1
                    assert audit['body'][0]['action'] == 'document_imported'
            assert rust('IMPORT', f"/matters/{ids['rust']}/documents/raw",
                        {'filename': 'copy.pdf', 'bytes': list(sample)})['status'] == 409
            def both(method, py_path, rs_path, body=None):
                expected = client.request(method, py_path, headers=headers, json=body)
                actual = rust(method, rs_path, body)
                assert expected.status_code == actual['status'], (method, py_path, expected.text, actual)
                return expected.json(), actual['body']

            python_base = f"/documents/{py_doc['id']}"
            rust_base = f"/documents/{rs_doc['id']}"
            python_info, rust_info = both('GET', python_base + '/original-info', rust_base + '/original-info')
            for key in python_info.keys() - {'document_id', 'imported_at'}:
                assert python_info[key] == rust_info[key], (key, python_info, rust_info)
            sha = sha256(sample).hexdigest()
            for endpoint, selection in [
                ('/original', {'kind': 'original'}),
                ('/pdf?revision=1', {'kind': 'working', 'revision': 1}),
                (f'/revisions/1/pdf?sha256={sha}', {'kind': 'retained', 'revision': 1, 'sha256': sha}),
            ]:
                metadata = client.head(python_base + endpoint, headers=headers)
                source = rust('SNAPSHOT', rust_base, selection)
                assert source['status'] == metadata.status_code == 200
                assert source['body']['length'] == int(metadata.headers['content-length'])
                assert bytes(source['body']['bytes']) == client.get(python_base + endpoint, headers=headers).content == sample
            assert rust('SNAPSHOT', rust_base, {'kind': 'working', 'revision': 2})['status'] == 409
            assert rust('SNAPSHOT', rust_base, {'kind': 'retained', 'revision': 1, 'sha256': '0' * 64})['status'] == 409

            for left, right in [
                (python_base + '/search?revision=1&q=PeTiTiOn', rust_base + '/search?revision=1&q=PeTiTiOn'),
                (f"/matters/{ids['python']}/search?q=%20petition", f"/matters/{ids['rust']}/search?q=%20petition"),
                (python_base + '/search?revision=2&q=petition', rust_base + '/search?revision=2&q=petition'),
                (python_base + '/search?q=%20x', rust_base + '/search?q=%20x'),
            ]:
                py, rs = both('GET', left, right)
                for match in py.get('matches', []):
                    match['document_id'] = rs_doc['id']
                assert py == rs, (left, py, rs)

            from urllib.parse import quote
            for py_key, rs_key in [
                (f"{py_doc['id']}:1", f"{rs_doc['id']}:1"),
                (f"original:{py_doc['id']}", f"original:{rs_doc['id']}"),
            ]:
                position = dict(page=2, x=0.1, y=0.5, zoom='fit-width', mode='single')
                py_endpoint = f"/reader-position?key={quote(py_key, safe='')}"
                rs_endpoint = f"/reader-position?key={quote(rs_key, safe='')}"
                assert both('GET', py_endpoint, rs_endpoint) == (None, None)
                expected = client.put('/reader-position', headers=headers, json={**position, 'key': py_key})
                actual = rust('PUT', '/reader-position', {**position, 'key': rs_key})
                assert expected.status_code == actual['status'] == 200
                assert expected.json() == actual['body'] == position
                assert both('GET', py_endpoint, rs_endpoint) == (position, position)
            assert rust('GET', f"/reader-position?key={rs_doc['id']}%3A2")['status'] == 409

            def history():
                py, rs = both('GET', f"/matters/{ids['python']}/history",
                              f"/matters/{ids['rust']}/history")
                assert py == rs, (py, rs)
                return rs

            def documents(archived=False):
                suffix = 'archived-documents' if archived else 'documents'
                py, rs = both('GET', f"/matters/{ids['python']}/{suffix}",
                              f"/matters/{ids['rust']}/{suffix}")
                assert len(py) == len(rs), (suffix, py, rs)
                for left, right in zip(py, rs):
                    for key in ('source_sha256', 'position', 'revision', 'bundle_included', 'page_count'):
                        assert left[key] == right[key], (suffix, key, left[key], right[key])
                    assert bool(left['archived_at']) == bool(right['archived_at'])
                return rs

            assert history()['undo_label'] == 'Add or restore document'
            part_path = "parts/" + str(rs_doc['part'])
            py, rs = both('DELETE', f"/matters/{ids['python']}/{part_path}",
                          f"/matters/{ids['rust']}/{part_path}")
            assert py == rs and 'including archived documents' in rs['detail']
            archive, rust_archive = both('DELETE', f"/documents/{py_doc['id']}",
                                         f"/documents/{rs_doc['id']}")
            assert archive == rust_archive == {'status': 'archived', 'files_retained': True}
            assert documents() == []
            assert len(documents(True)) == 1
            py, rs = both('DELETE', f"/matters/{ids['python']}/{part_path}",
                          f"/matters/{ids['rust']}/{part_path}")
            assert py == rs and 'including archived documents' in rs['detail']
            assert history()['undo_label'] == 'Archive document'
            assert both('DELETE', f"/documents/{py_doc['id']}",
                        f"/documents/{rs_doc['id']}")[0]['detail'] == 'Document not found'
            py_restore, rs_restore = both(
                'POST', f"/matters/{ids['python']}/archived-documents/{py_doc['id']}/restore",
                f"/matters/{ids['rust']}/archived-documents/{rs_doc['id']}/restore")
            assert py_restore['position'] == rs_restore['position'] == 0
            assert py_restore['archived_at'] == rs_restore['archived_at'] is None
            assert documents(True) == []
            assert len(documents()) == 1
            assert history()['undo_count'] == 3
            assert both('POST', f"/matters/{ids['python']}/archived-documents/{py_doc['id']}/restore",
                        f"/matters/{ids['rust']}/archived-documents/{rs_doc['id']}/restore")[0]['detail'] == 'Archived document not found'
            for direction, archived in [('undo', True), ('undo', False), ('undo', True),
                                        ('redo', False), ('redo', True), ('redo', False)]:
                py, rs = both('POST', f"/matters/{ids['python']}/history/{direction}",
                              f"/matters/{ids['rust']}/history/{direction}")
                assert py == rs, (direction, py, rs)
                assert len(documents(archived)) == 1
            assert history()['undo_count'] == 3
            assert both('POST', f"/matters/{ids['python']}/history/redo",
                        f"/matters/{ids['rust']}/history/redo")[0]['detail'] == 'No edit to redo'
            for engine, mid in ids.items():
                if engine == 'python':
                    actions = [e['action'] for e in client.get(f'/matters/{mid}/audit', headers=headers).json()]
                else:
                    actions = [e['action'] for e in rust('GET', f'/matters/{mid}/audit')['body']]
                assert actions[:8] == ['edit_redone'] * 3 + ['edit_undone'] * 3 + ['document_restored', 'document_archived'], actions
            assert sha256(sample).hexdigest() == rs_doc['source_sha256']
    finally:
        driver.stdin.close()
        assert driver.wait(timeout=10) == 0
print('Synthetic import contract subset passed')
