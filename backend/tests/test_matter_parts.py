import time

import pymupdf

def wait_for_job(client, auth, job_id):
    for _ in range(200):
        job = client.get(f'/jobs/{job_id}', headers=auth).json()
        if job['status'] in ('completed', 'failed', 'cancelled'):
            return job
        time.sleep(.05)
    raise AssertionError('Build did not finish')


def test_parts_are_matter_scoped_and_occupied_parts_cannot_be_removed(client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}'
    assert matter['parts'] == [1, 2, 3, 4]
    assert matter['next_part'] == 5
    added = client.post(base + '/parts', headers=auth)
    assert added.status_code == 201, added.text
    assert added.json()['parts'] == [1, 2, 3, 4, 5]
    assert added.json()['next_part'] == 6
    second = client.post('/matters', headers=auth, json={'name':'Second synthetic matter'})
    assert second.status_code == 201
    assert second.json()['parts'] == [1, 2, 3, 4]

    imported = client.post(base + '/documents?included=true', headers=auth,
        files=[('files', ('synthetic.pdf', make_pdf(1), 'application/pdf'))])
    assert imported.status_code == 201, imported.text
    doc = imported.json()[0]
    assert client.patch(f'/documents/{doc["id"]}', headers=auth, json={'part': 6}).status_code == 422
    moved = client.patch(f'/documents/{doc["id"]}', headers=auth, json={'part': 5})
    assert moved.status_code == 200, moved.text
    assert client.delete(base + '/parts/5', headers=auth).status_code == 409
    assert client.patch(f'/documents/{doc["id"]}', headers=auth, json={'part': 1}).status_code == 200
    removed = client.delete(base + '/parts/5', headers=auth)
    assert removed.status_code == 200, removed.text
    assert removed.json()['parts'] == [1, 2, 3, 4]
    assert client.post(base + '/parts', headers=auth).json()['parts'] == [1, 2, 3, 4, 6]
    assert client.patch(f'/documents/{doc["id"]}', headers=auth, json={'part': 5}).status_code == 422


def test_new_part_uses_one_page_map_for_preview_extract_and_portable_restore(client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}'
    assert client.post(base + '/parts', headers=auth).status_code == 201
    raw = make_pdf(2, 'Dynamic Part five')
    imported = client.post(base + '/documents?included=true', headers=auth,
        files=[('files', ('part-five.pdf', raw, 'application/pdf'))])
    assert imported.status_code == 201, imported.text
    doc = imported.json()[0]
    assert client.patch(f'/documents/{doc["id"]}', headers=auth,
                        json={'part': 5}).status_code == 200
    marks = client.get(base + '/output-marks', headers=auth).json()
    marks['part_extracts_enabled'] = True
    assert client.put(base + '/output-marks', headers=auth, json=marks).status_code == 200
    preview = client.post(base + '/bundle-preview', headers=auth,
        json={'max_pages_per_volume': 300, 'keep_groups_intact': True})
    assert preview.status_code == 200, preview.text
    assert preview.json()['part_extracts'] == [{'part': 5, 'documents': 1, 'pages': 2}]
    started = client.post(base + '/build', headers=auth)
    assert started.status_code == 202, started.text
    assert wait_for_job(client, auth, started.json()['id'])['status'] == 'completed'
    export = client.get(base + '/exports', headers=auth).json()[0]
    assert export['part_extracts'][0]['filename'] == 'part-05.pdf'
    store = client.app.state.store
    with pymupdf.open(store.get_export_file(matter['id'], export['id'], 'part-05.pdf')) as pdf:
        assert pdf.page_count == 2 and 'Dynamic Part five' in pdf[0].get_text()
    assert store.original_path(doc).read_bytes() == raw

    backup = client.get(base + '/portable', headers=auth)
    assert backup.status_code == 200
    restored = client.post('/matters/restore', headers=auth,
        files={'file': ('dynamic.cbmatter', backup.content, 'application/zip')})
    assert restored.status_code == 201, restored.text
    assert restored.json()['parts'] == [1, 2, 3, 4, 5]
    restored_docs = client.get(f'/matters/{restored.json()["id"]}/documents', headers=auth).json()
    assert restored_docs[0]['part'] == 5


def test_part_removal_checks_filing_review_and_history(client, auth, matter):
    base = f'/matters/{matter["id"]}'
    review = {'court':'', 'case_type':'', 'stage':'', 'source_note':'',
              'items':[{'title':'Synthetic check', 'part':4, 'status':'unreviewed'}]}
    assert client.put(base + '/filing-review', headers=auth, json=review).status_code == 200
    blocked = client.delete(base + '/parts/4', headers=auth)
    assert blocked.status_code == 409 and 'checklist' in blocked.json()['detail']
    review['items'][0]['part'] = None
    assert client.put(base + '/filing-review', headers=auth, json=review).status_code == 200
    assert client.delete(base + '/parts/4', headers=auth).json()['parts'] == [1, 2, 3]
    assert client.post(base + '/history/undo', headers=auth).status_code == 200
    assert client.get(base, headers=auth).json()['parts'] == [1, 2, 3, 4]
    assert client.post(base + '/history/redo', headers=auth).status_code == 200
    assert client.get(base, headers=auth).json()['parts'] == [1, 2, 3]


def test_first_part_can_be_removed_and_new_import_uses_remaining_part(client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}'
    assert client.delete(base + '/parts/1', headers=auth).json()['parts'] == [2, 3, 4]
    imported = client.post(base + '/documents?included=true', headers=auth,
        files=[('files', ('remaining-part.pdf', make_pdf(1), 'application/pdf'))])
    assert imported.status_code == 201, imported.text
    doc = imported.json()[0]
    assert doc['part'] == 2
    assert client.delete(base + '/parts/2', headers=auth).status_code == 409
    assert client.delete(f'/documents/{doc["id"]}', headers=auth).status_code == 200
    blocked = client.delete(base + '/parts/2', headers=auth)
    assert blocked.status_code == 409 and 'archived' in blocked.json()['detail']
