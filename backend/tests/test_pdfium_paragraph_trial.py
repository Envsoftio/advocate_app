"""Synthetic PDFium object-edit checks; native device acceptance remains separate."""
import time

import pymupdf


def test_pdfium_paragraph_trial_revision_original_and_undo(client, auth, matter):
    with pymupdf.open() as source:
        page = source.new_page()
        page.insert_text((72, 100), 'First paragraph line', fontsize=12)
        page.insert_text((72, 115), 'Second paragraph line', fontsize=12)
        page.insert_text((72, 250), 'UNTOUCHED', fontsize=12)
        original = source.tobytes()
    uploaded = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
                           files=[('files', ('paragraph.pdf', original, 'application/pdf'))])
    assert uploaded.status_code == 201, uploaded.text
    doc = uploaded.json()[0]
    base = f'/documents/{doc["id"]}'
    store = client.app.state.store
    original_hash = store.sha256(store.original_path(doc))
    status = client.get('/pdfium-paragraph-trial/status', headers=auth)
    assert status.status_code == 200 and status.json()['available']
    run = client.get(base + '/pages/0/text-runs', headers=auth).json()['runs'][0]
    selection = {'page_index': 0, 'span_index': run['index'],
                 'expected_revision': 1, 'expected_text': run['text']}
    preview = client.post(base + '/pages/paragraph-trial-preview', headers=auth,
                          json=selection)
    assert preview.status_code == 200, preview.text
    paragraph = preview.json()['text']
    assert paragraph == 'First paragraph line Second paragraph line'
    payload = {**selection, 'expected_paragraph': paragraph,
               'replacement': 'A changed paragraph\nthat should reflow into several readable lines'}
    changed = client.post(base + '/pages/paragraph-trial', headers=auth, json=payload)
    assert changed.status_code == 200, changed.text
    assert changed.json()['revision'] == 2
    assert changed.json()['edit_layout']['edit_method'] == 'native-content-edit'
    with pymupdf.open(stream=client.get(base + '/pdf', headers=auth).content,
                      filetype='pdf') as current:
        text = current[0].get_text()
        assert 'A changed paragraph' in text
        assert 'First paragraph line' not in text
        assert 'Second paragraph line' not in text
        assert 'UNTOUCHED' in text
    assert store.sha256(store.original_path(doc)) == original_hash
    build = client.post(f'/matters/{matter["id"]}/build', headers=auth)
    assert build.status_code == 202, build.text
    for _ in range(200):
        job = client.get(f'/jobs/{build.json()["id"]}', headers=auth).json()
        if job['status'] not in ('queued', 'running'):
            break
        time.sleep(.03)
    assert job['status'] == 'completed', job
    combined = client.get(f'/matters/{matter["id"]}/exports/{job["result"]["id"]}/combined.pdf',
                          headers=auth)
    with pymupdf.open(stream=combined.content, filetype='pdf') as exported:
        assert 'A changed paragraph' in exported[1].get_text()
        assert 'UNTOUCHED' in exported[1].get_text()
    stale = client.post(base + '/pages/paragraph-trial', headers=auth, json=payload)
    assert stale.status_code == 409
    undone = client.post(f'/matters/{matter["id"]}/history/undo', headers=auth)
    assert undone.status_code == 200
    with pymupdf.open(stream=client.get(base + '/pdf', headers=auth).content,
                      filetype='pdf') as restored:
        assert 'First paragraph line' in restored[0].get_text()


def test_pdfium_trial_rejects_unfittable_edit_without_revision(client, auth, matter, make_pdf):
    uploaded = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
                           files=[('files', ('short.pdf', make_pdf(1, 'SHORT'), 'application/pdf'))])
    doc = uploaded.json()[0]
    base = f'/documents/{doc["id"]}'
    run = client.get(base + '/pages/0/text-runs', headers=auth).json()['runs'][0]
    selection = {'page_index': 0, 'span_index': run['index'],
                 'expected_revision': 1, 'expected_text': run['text']}
    paragraph = client.post(base + '/pages/paragraph-trial-preview', headers=auth,
                            json=selection).json()['text']
    response = client.post(base + '/pages/paragraph-trial', headers=auth, json={
        **selection, 'expected_paragraph': paragraph, 'replacement': 'W' * 1000})
    assert response.status_code == 422
    assert client.app.state.store.get_document(doc['id'])['revision'] == 1
    unchanged = client.post(base + '/pages/paragraph-trial', headers=auth, json={
        **selection, 'expected_paragraph': paragraph, 'replacement': paragraph})
    assert unchanged.status_code == 422
    assert client.app.state.store.get_document(doc['id'])['revision'] == 1


def test_pdfium_trial_allows_expanding_a_paragraph_that_contains_old_text(client, auth, matter):
    with pymupdf.open() as source:
        page = source.new_page()
        page.insert_text((72, 100), 'Original wording', fontsize=12)
        original = source.tobytes()
    uploaded = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
                           files=[('files', ('expand.pdf', original, 'application/pdf'))])
    doc = uploaded.json()[0]
    base = f'/documents/{doc["id"]}'
    run = client.get(base + '/pages/0/text-runs', headers=auth).json()['runs'][0]
    selection = {'page_index': 0, 'span_index': run['index'],
                 'expected_revision': 1, 'expected_text': run['text']}
    paragraph = client.post(base + '/pages/paragraph-trial-preview', headers=auth,
                            json=selection).json()['text']
    replacement = 'Original wording with correction'
    response = client.post(base + '/pages/paragraph-trial', headers=auth, json={
        **selection, 'expected_paragraph': paragraph, 'replacement': replacement})
    assert response.status_code == 200, response.text
    with pymupdf.open(stream=client.get(base + '/pdf', headers=auth).content,
                      filetype='pdf') as edited:
        assert replacement in edited[0].get_text().replace('\n', ' ')


def test_pdfium_trial_rejects_a_link_over_the_paragraph(client, auth, matter):
    with pymupdf.open() as source:
        page = source.new_page()
        page.insert_text((72, 100), 'Linked wording', fontsize=12)
        page.insert_link({'kind': pymupdf.LINK_URI, 'from': pymupdf.Rect(70, 85, 170, 105),
                          'uri': 'https://example.invalid/'})
        original = source.tobytes()
    uploaded = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
                           files=[('files', ('linked.pdf', original, 'application/pdf'))])
    doc = uploaded.json()[0]
    base = f'/documents/{doc["id"]}'
    run = client.get(base + '/pages/0/text-runs', headers=auth).json()['runs'][0]
    selection = {'page_index': 0, 'span_index': run['index'],
                 'expected_revision': 1, 'expected_text': run['text']}
    paragraph = client.post(base + '/pages/paragraph-trial-preview', headers=auth,
                            json=selection).json()['text']
    response = client.post(base + '/pages/paragraph-trial', headers=auth, json={
        **selection, 'expected_paragraph': paragraph, 'replacement': 'Different linked wording'})
    assert response.status_code == 422
    assert 'overlaps an image, graphic, link or annotation' in response.json()['detail']
    assert client.app.state.store.get_document(doc['id'])['revision'] == 1


def test_pdfium_trial_preserves_same_words_elsewhere_on_page(client, auth, matter):
    with pymupdf.open() as source:
        page = source.new_page()
        page.insert_text((72, 100), 'Repeated wording', fontsize=12)
        page.insert_text((72, 250), 'Repeated wording', fontsize=12)
        original = source.tobytes()
    uploaded = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
                           files=[('files', ('repeated.pdf', original, 'application/pdf'))])
    doc = uploaded.json()[0]
    base = f'/documents/{doc["id"]}'
    run = client.get(base + '/pages/0/text-runs', headers=auth).json()['runs'][0]
    selection = {'page_index': 0, 'span_index': run['index'],
                 'expected_revision': 1, 'expected_text': run['text']}
    paragraph = client.post(base + '/pages/paragraph-trial-preview', headers=auth,
                            json=selection).json()['text']
    changed = client.post(base + '/pages/paragraph-trial', headers=auth, json={
        **selection, 'expected_paragraph': paragraph, 'replacement': 'Changed wording'})
    assert changed.status_code == 200, changed.text
    with pymupdf.open(stream=client.get(base + '/pdf', headers=auth).content,
                      filetype='pdf') as edited:
        assert 'Changed wording' in edited[0].get_text()
        assert 'Repeated wording' in edited[0].get_text()


def test_pdfium_trial_moves_following_text_to_fit_added_lines(client, auth, matter):
    with pymupdf.open() as source:
        page = source.new_page()
        page.insert_text((72, 100), 'Short first paragraph', fontsize=12)
        page.insert_text((72, 127), 'NEXT PARAGRAPH', fontsize=12)
        page.insert_text((72, 145), 'STAYS IN ORDER', fontsize=12)
        original = source.tobytes()
    uploaded = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
                           files=[('files', ('flow.pdf', original, 'application/pdf'))])
    assert uploaded.status_code == 201, uploaded.text
    doc = uploaded.json()[0]
    base = f'/documents/{doc["id"]}'
    run = client.get(base + '/pages/0/text-runs', headers=auth).json()['runs'][0]
    selection = {'page_index': 0, 'span_index': run['index'],
                 'expected_revision': 1, 'expected_text': run['text']}
    paragraph = client.post(base + '/pages/paragraph-trial-preview', headers=auth,
                            json=selection).json()['text']
    response = client.post(base + '/pages/paragraph-trial', headers=auth, json={
        **selection, 'expected_paragraph': paragraph,
        'replacement': 'A longer first paragraph\nwith a second line\nand a third line'})
    assert response.status_code == 200, response.text
    assert response.json()['edit_layout']['following_text_objects_moved'] == 2
    assert response.json()['edit_layout']['following_text_shift'] > 0
    with pymupdf.open(stream=client.get(base + '/pdf', headers=auth).content,
                      filetype='pdf') as edited:
        words = edited[0].get_text('words')
        first_next = next(word for word in words if word[4] == 'NEXT')
        third = next(word for word in words if word[4] == 'third')
        assert first_next[1] > third[3]
        assert 'STAYS IN ORDER' in edited[0].get_text()


def test_pdfium_trial_keeps_revision_when_following_text_cannot_fit(client, auth, matter):
    with pymupdf.open() as source:
        page = source.new_page()
        page.insert_text((72, 795), 'BOTTOM PARAGRAPH', fontsize=12)
        page.insert_text((72, 825), 'BOTTOM FOLLOWER', fontsize=12)
        original = source.tobytes()
    uploaded = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
                           files=[('files', ('bottom.pdf', original, 'application/pdf'))])
    doc = uploaded.json()[0]
    base = f'/documents/{doc["id"]}'
    run = client.get(base + '/pages/0/text-runs', headers=auth).json()['runs'][0]
    selection = {'page_index': 0, 'span_index': run['index'],
                 'expected_revision': 1, 'expected_text': run['text']}
    paragraph = client.post(base + '/pages/paragraph-trial-preview', headers=auth,
                            json=selection).json()['text']
    response = client.post(base + '/pages/paragraph-trial', headers=auth, json={
        **selection, 'expected_paragraph': paragraph,
        'replacement': 'A longer bottom paragraph\nsecond line\nthird line'})
    assert response.status_code == 422
    assert 'cannot fit on this page' in response.json()['detail']
    assert client.app.state.store.get_document(doc['id'])['revision'] == 1
