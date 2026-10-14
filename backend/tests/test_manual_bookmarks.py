import io
import json
import zipfile
import pymupdf

from test_api import upload, wait_for_job


def bookmark(id_char, title, doc, page=0, parent=None):
    return {'id': id_char * 32, 'title': title, 'document_id': doc['id'],
            'page_index': page, 'target_revision': doc['revision'], 'parent_id': parent}


def test_manual_bookmarks_map_to_volume_and_combined_pages(client, auth, matter, make_pdf):
    first = upload(client, auth, matter, make_pdf, pages=2)
    second = upload(client, auth, matter, make_pdf, name='second.pdf', pages=2)
    base = f'/matters/{matter["id"]}'
    assert client.patch(base, headers=auth, json={'max_pages_per_volume': 3}).status_code == 200
    parent = bookmark('a', 'Reviewed anchor', first, page=1)
    child = bookmark('b', 'Nested anchor', second, page=0, parent=parent['id'])
    saved = client.put(base + '/bookmarks', headers=auth,
                       json={'entries': [parent, child]})
    assert saved.status_code == 200, saved.text
    assert all(item['resolved'] for item in saved.json())
    started = client.post(base + '/build', headers=auth)
    job = wait_for_job(client, auth, started.json()['id'])
    assert job['status'] == 'completed', job
    manifest = client.get(base + '/exports', headers=auth).json()[0]
    assert manifest['stale'] is False
    folder = client.app.state.store.exports_root(matter['id']) / manifest['id']
    with pymupdf.open(folder / 'combined.pdf') as pdf:
        manual = [item for item in pdf.get_toc() if item[1] in ('Manual bookmarks',
                                                                 'Reviewed anchor', 'Nested anchor')]
        assert manual == [[1, 'Manual bookmarks', 3], [2, 'Reviewed anchor', 3],
                          [3, 'Nested anchor', 5]]
    with pymupdf.open(folder / 'volume-02.pdf') as pdf:
        assert [item for item in pdf.get_toc() if item[1] == 'Nested anchor'] == [
            [2, 'Nested anchor', 2]]
    with zipfile.ZipFile(folder / 'filing-package.zip') as archive:
        report = json.loads(archive.read('validation-report.json'))
        assert report['build_input_digest'] == manifest['build_input_digest']


def test_bookmark_revision_review_undo_and_portable_restore(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, pages=2)
    base = f'/matters/{matter["id"]}'
    entry = bookmark('c', 'Manual target', doc, page=1)
    assert client.put(base + '/bookmarks', headers=auth,
                      json={'entries': [entry]}).status_code == 200
    assert client.post(base + '/history/undo', headers=auth).status_code == 200
    assert client.get(base + '/bookmarks', headers=auth).json() == []
    assert client.post(base + '/history/redo', headers=auth).status_code == 200
    assert client.get(base + '/bookmarks', headers=auth).json()[0]['resolved']
    rotated = client.post(f'/documents/{doc["id"]}/pages/rotate', headers=auth,
                          json={'page_index': 0, 'degrees': 90})
    assert rotated.status_code == 200
    assert client.get(base + '/bookmarks', headers=auth).json()[0]['resolved'] is False
    started = client.post(base + '/build', headers=auth)
    job = wait_for_job(client, auth, started.json()['id'])
    assert job['status'] == 'completed', job
    assert any(check['code'] == 'UNRESOLVED_MANUAL_BOOKMARKS'
               for check in job['result']['checks'])
    entry['target_revision'] = rotated.json()['revision']
    assert client.put(base + '/bookmarks', headers=auth,
                      json={'entries': [entry]}).status_code == 200
    backup = client.get(base + '/portable', headers=auth)
    assert backup.status_code == 200
    restored = client.post('/matters/restore', headers=auth,
        files={'file': ('matter.cbmatter', backup.content, 'application/zip')})
    assert restored.status_code == 201, restored.text
    restored_entries = client.get(f'/matters/{restored.json()["id"]}/bookmarks', headers=auth).json()
    assert len(restored_entries) == 1 and restored_entries[0]['resolved']
    assert restored_entries[0]['document_id'] != doc['id']


def test_bookmark_validation_rejects_bad_parent_and_stale_target(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, pages=1)
    base = f'/matters/{matter["id"]}/bookmarks'
    entry = bookmark('d', 'Valid', doc)
    bad = dict(entry, parent_id=entry['id'])
    assert client.put(base, headers=auth, json={'entries': [bad]}).status_code == 422
    bad = dict(entry, target_revision=99)
    assert client.put(base, headers=auth, json={'entries': [bad]}).status_code == 422
    assert client.get(base, headers=auth).json() == []
