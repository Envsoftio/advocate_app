import pymupdf


def upload(client, auth, matter_id, name):
    with pymupdf.open() as pdf:
        pdf.new_page().insert_text((72, 90), name)
        content = pdf.tobytes()
    response = client.post(f'/matters/{matter_id}/documents', headers=auth,
        files=[('files', (name, content, 'application/pdf'))])
    assert response.status_code == 201, response.text
    return response.json()[0]


def test_document_dates_batch_edits_and_history_are_atomic(client, auth, matter):
    docs = [upload(client, auth, matter['id'], name) for name in
            ('z-last.pdf', 'a-first.pdf', 'middle.pdf')]

    dated = client.patch(f'/documents/{docs[0]["id"]}', headers=auth, json={
        'document_date': '2026-02-03', 'date_status': 'confirmed', 'date_source': 'user'})
    assert dated.status_code == 200, dated.text
    assert dated.json()['date_status'] == 'confirmed'
    partial = client.patch(f'/documents/{docs[1]["id"]}', headers=auth,
                           json={'document_date': '2026-02-04'})
    assert partial.status_code == 422
    invalid_date = client.patch(f'/documents/{docs[1]["id"]}', headers=auth, json={
        'document_date': '2026-02-31', 'date_status': 'confirmed', 'date_source': 'user'})
    assert invalid_date.status_code == 422
    guessed_confirmed = client.patch(f'/documents/{docs[1]["id"]}', headers=auth, json={
        'document_date': '2026-02-04', 'date_status': 'confirmed', 'date_source': 'filename'})
    assert guessed_confirmed.status_code == 422
    suggested = client.patch(f'/documents/{docs[1]["id"]}', headers=auth, json={
        'document_date': '2026-02-03', 'date_status': 'suggested', 'date_source': 'filename'})
    assert suggested.status_code == 200, suggested.text

    selected = [docs[0]['id'], docs[2]['id']]
    result = client.patch(f'/matters/{matter["id"]}/documents/batch', headers=auth,
                          json={'ids': selected, 'kind': 'annexure', 'part': 2,
                                'group_key': 'linked-set'})
    assert result.status_code == 200, result.text
    by_id = {doc['id']: doc for doc in result.json()}
    assert all(by_id[doc_id]['kind'] == 'annexure' and by_id[doc_id]['part'] == 2
               and by_id[doc_id]['group_key'] == 'linked-set' for doc_id in selected)
    assert by_id[docs[1]['id']]['kind'] == 'other'

    assert client.post(f'/matters/{matter["id"]}/history/undo', headers=auth).status_code == 200
    undone = {doc['id']: doc for doc in client.get(
        f'/matters/{matter["id"]}/documents', headers=auth).json()}
    assert all(undone[doc_id]['kind'] == 'other' and undone[doc_id]['part'] == 1
               and undone[doc_id]['group_key'] == '' for doc_id in selected)
    assert client.post(f'/matters/{matter["id"]}/history/redo', headers=auth).status_code == 200
    redone = {doc['id']: doc for doc in client.get(
        f'/matters/{matter["id"]}/documents', headers=auth).json()}
    assert all(redone[doc_id]['group_key'] == 'linked-set' for doc_id in selected)

    before = redone
    failed = client.patch(f'/matters/{matter["id"]}/documents/batch', headers=auth,
                          json={'ids': [docs[0]['id'], 'missing'], 'part': 4})
    assert failed.status_code == 422
    after = {doc['id']: doc for doc in client.get(
        f'/matters/{matter["id"]}/documents', headers=auth).json()}
    assert [(key, after[key]['part']) for key in after] == [(key, before[key]['part']) for key in before]


def test_sort_preview_is_stable_preserves_date_review_and_groups(client, auth, matter):
    docs = [upload(client, auth, matter['id'], name) for name in
            ('same-b.pdf', 'unknown.pdf', 'same-a.pdf', 'earliest.pdf', 'ambiguous-03-04.pdf')]
    dates = [
        ('2026-03-01', 'confirmed', 'user', ''),
        ('', 'unknown', 'none', ''),
        ('2026-03-01', 'suggested', 'filename', ''),
        ('2025-12-31', 'confirmed', 'user', ''),
        ('', 'ambiguous', 'filename', '03/04/2026'),
    ]
    for doc, (value, status, source, note) in zip(docs, dates):
        response = client.patch(f'/documents/{doc["id"]}', headers=auth, json={
            'document_date': value, 'date_status': status, 'date_source': source,
            'date_note': note})
        assert response.status_code == 200, response.text
    for doc, label in zip(docs[:3], ('P-1', 'p-1', 'P-3')):
        assert client.patch(f'/documents/{doc["id"]}', headers=auth,
                            json={'label': label}).status_code == 200

    preview = client.post(f'/matters/{matter["id"]}/documents/sort-preview', headers=auth,
                          json={'mode': 'document_date', 'unknown_dates': 'last'})
    assert preview.status_code == 200, preview.text
    body = preview.json()
    assert body['ids'] == [docs[3]['id'], docs[0]['id'], docs[2]['id'], docs[1]['id'], docs[4]['id']]
    assert body['rows'][2]['date_status'] == 'suggested'
    assert body['rows'][-1]['date_status'] == 'ambiguous'
    assert body['rows'][-1]['date_note'] == '03/04/2026'
    assert body['label_review'] == {'duplicates': ['P-1'], 'gaps': ['P-2']}
    assert body['unknown_dates'] == 'last'
    assert client.get(f'/matters/{matter["id"]}/documents', headers=auth).json()[0]['id'] == docs[0]['id']

    # Grouped documents remain a single unit even when filename sorting would split them.
    batch = client.patch(f'/matters/{matter["id"]}/documents/batch', headers=auth,
                         json={'ids': [docs[0]['id'], docs[2]['id']], 'group_key': 'translation'})
    assert batch.status_code == 200
    by_filename = client.post(f'/matters/{matter["id"]}/documents/sort-preview', headers=auth,
                              json={'mode': 'filename'}).json()
    positions = [by_filename['ids'].index(docs[0]['id']), by_filename['ids'].index(docs[2]['id'])]
    assert abs(positions[0] - positions[1]) == 1
    assert by_filename['group_policy'].startswith('keep grouped')

    commit = client.post(f'/matters/{matter["id"]}/documents/reorder', headers=auth,
                         json={'ids': body['ids']})
    assert commit.status_code == 200, commit.text
    assert [doc['id'] for doc in commit.json()] == body['ids']
    final = {doc['id']: doc for doc in commit.json()}
    assert final[docs[2]['id']]['date_status'] == 'suggested'


def test_raw_upload_records_source_timestamp_without_using_it_as_document_date(
        client, auth, matter, make_pdf):
    response = client.post(f'/matters/{matter["id"]}/documents/raw', headers={
        **auth, 'Content-Type': 'application/octet-stream',
        'X-CourtBundle-Filename': 'timestamped.pdf',
        'X-CourtBundle-File-Mtime': '2026-10-15T08:30:00.000Z'}, content=make_pdf(1))
    assert response.status_code == 201, response.text
    doc = response.json()[0]
    assert doc['source_file_mtime'] == '2026-10-15T08:30:00.000Z'
    assert doc['document_date'] == ''
    assert doc['date_status'] == 'unknown'
