import time

import pymupdf


def test_add_text_preserves_prior_content_and_page_components(client, auth, matter, make_pdf):
    source = make_pdf(1, 'ORIGINAL')
    doc = client.post(f'/matters/{matter["id"]}/documents?included=true', headers=auth,
                      files=[('files', ('source.pdf', source, 'application/pdf'))]).json()[0]
    store = client.app.state.store
    source_hash = store.sha256(store.original_path(doc))
    endpoint = f'/documents/{doc["id"]}/pages/text-overlay'

    def place(x, y, text):
        return client.post(endpoint, headers=auth, json={
            'page_index': 0, 'x': x, 'y': y, 'text': text, 'font_size': 12})

    first = place(.2, .3, 'FIRST LINE')
    assert first.status_code == 200, first.text
    with pymupdf.open(store.working_path(first.json())) as pdf:
        original_box = pdf[0].search_for('ORIGINAL')[0]
        first_box = pdf[0].search_for('FIRST LINE')[0]

    annotation = client.post(f'/documents/{doc["id"]}/annotations', headers=auth, json={
        'page_index': 0, 'expected_revision': 2, 'kind': 'comment',
        'rect_x': .7, 'rect_y': .7, 'rect_width': .1, 'rect_height': .05,
        'body': 'Retain this', 'color': 'yellow'})
    assert annotation.status_code == 201, annotation.text
    annotation_id = annotation.json()[0]['id']
    pix = pymupdf.Pixmap(pymupdf.csRGB, pymupdf.IRect(0, 0, 20, 20), False)
    pix.clear_with(0x235BA2)
    image = client.post(f'/documents/{doc["id"]}/annotations/image', headers=auth,
                        data={'page_index': '0', 'expected_revision': '2',
                              'rect_x': '.65', 'rect_y': '.5',
                              'rect_width': '.15', 'rect_height': '.1'},
                        files={'file': ('mark.png', pix.tobytes('png'), 'image/png')})
    assert image.status_code == 201, image.text
    image_id = next(item['id'] for item in image.json() if item['kind'] == 'image')

    second = place(.2, .4, 'SECOND LINE\nNEXT LINE')
    assert second.status_code == 200, second.text
    with pymupdf.open(store.working_path(second.json())) as pdf:
        assert pdf[0].search_for('ORIGINAL')[0] == original_box
        assert pdf[0].search_for('FIRST LINE')[0] == first_box
        assert pdf[0].search_for('SECOND LINE')
        assert pdf[0].search_for('NEXT LINE')
    items = client.get(f'/matters/{matter["id"]}/annotations', headers=auth).json()
    assert next(item for item in items if item['id'] == annotation_id)['resolved'] is True
    assert next(item for item in items if item['id'] == image_id)['resolved'] is True
    assert store.sha256(store.original_path(doc)) == source_hash

    collision = place(.2, .3, 'OVERLAP')
    assert collision.status_code == 422
    assert 'overlaps' in collision.json()['detail']
    assert store.get_document(doc['id'])['revision'] == second.json()['revision']
    browser_collision = client.post(endpoint, headers={
        **auth, 'X-CourtBundle-Review-Protocol': 'challenge'}, json={
        'page_index': 0, 'x': .2, 'y': .3, 'text': 'OVERLAP', 'font_size': 12})
    assert browser_collision.status_code == 200
    assert browser_collision.json()['placement_rejected'] is True
    assert store.get_document(doc['id'])['revision'] == second.json()['revision']
    image_collision = place(.67, .53, 'COVER IMAGE')
    assert image_collision.status_code == 422
    assert 'component' in image_collision.json()['detail']
    assert store.get_document(doc['id'])['revision'] == second.json()['revision']

    assert client.post(f'/matters/{matter["id"]}/history/undo', headers=auth).status_code == 200
    assert all(item['resolved'] for item in client.get(
        f'/matters/{matter["id"]}/annotations', headers=auth).json())
    assert client.post(f'/matters/{matter["id"]}/history/redo', headers=auth).status_code == 200
    assert all(item['resolved'] for item in client.get(
        f'/matters/{matter["id"]}/annotations', headers=auth).json())


def test_text_overlay_is_visible_searchable_undoable_and_exported(client, auth, matter, make_pdf):
    source = make_pdf(1, 'ORIGINAL')
    response = client.post(f'/matters/{matter["id"]}/documents?included=true', headers=auth,
                           files=[('files', ('source.pdf', source, 'application/pdf'))])
    assert response.status_code == 201, response.text
    doc = response.json()[0]
    store = client.app.state.store
    original_hash = store.sha256(store.original_path(doc))
    url = f'/documents/{doc["id"]}/pages/text-overlay'
    added = client.post(url, headers=auth, json={
        'page_index': 0, 'x': .15, 'y': .18, 'text': 'REVIEW COPY', 'font_size': 12,
        'acknowledge_signature_risk': None})
    assert added.status_code == 200, added.text
    assert added.json()['revision'] == 2
    working = client.get(f'/documents/{doc["id"]}/pdf', headers=auth)
    with pymupdf.open(stream=working.content, filetype='pdf') as pdf:
        assert 'REVIEW COPY' in pdf[0].get_text()
    assert store.sha256(store.original_path(doc)) == original_hash

    build = client.post(f'/matters/{matter["id"]}/build', headers=auth)
    assert build.status_code == 202, build.text
    for _ in range(200):
        job = client.get(f'/jobs/{build.json()["id"]}', headers=auth).json()
        if job['status'] not in ('queued', 'running'):
            break
        time.sleep(.03)
    assert job['status'] == 'completed', job
    export_id = job['result']['id']
    exported = client.get(f'/matters/{matter["id"]}/exports/{export_id}/combined.pdf', headers=auth)
    assert exported.status_code == 200, exported.text
    with pymupdf.open(stream=exported.content, filetype='pdf') as pdf:
        assert 'REVIEW COPY' in pdf[1].get_text()

    assert client.post(f'/matters/{matter["id"]}/history/undo', headers=auth).status_code == 200
    undone = client.get(f'/documents/{doc["id"]}/pdf', headers=auth)
    with pymupdf.open(stream=undone.content, filetype='pdf') as pdf:
        assert 'REVIEW COPY' not in pdf[0].get_text()


def test_invalid_overlay_does_not_publish_revision(client, auth, matter, make_pdf):
    response = client.post(f'/matters/{matter["id"]}/documents?included=true', headers=auth,
                           files=[('files', ('source.pdf', make_pdf(1), 'application/pdf'))])
    doc = response.json()[0]
    url = f'/documents/{doc["id"]}/pages/text-overlay'
    for payload in (
        {'page_index': 0, 'x': .99, 'y': .5, 'text': 'EDGE', 'font_size': 12},
        {'page_index': 0, 'x': .2, 'y': .2, 'text': 'हिंदी', 'font_size': 12},
        {'page_index': 8, 'x': .2, 'y': .2, 'text': 'TEST', 'font_size': 12},
    ):
        result = client.post(url, headers=auth, json=payload)
        assert result.status_code == 422, result.text
    assert client.app.state.store.get_document(doc['id'])['revision'] == 1
    rotated = client.post(f'/documents/{doc["id"]}/pages/rotate', headers=auth,
                          json={'page_index': 0, 'degrees': 90})
    assert rotated.status_code == 200, rotated.text
    accepted = client.post(url, headers=auth, json={
        'page_index': 0, 'x': .2, 'y': .2, 'text': 'TEST', 'font_size': 12})
    assert accepted.status_code == 200, accepted.text
    assert client.app.state.store.get_document(doc['id'])['revision'] == 3
