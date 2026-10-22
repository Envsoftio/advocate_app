import hashlib
import io
import json
import time
import zipfile

import pymupdf
import pytest
from PIL import Image


def test_selected_redaction_can_be_removed_after_later_edits_without_restoring_other_secrets(
        client, auth, matter):
    with pymupdf.open() as source:
        page = source.new_page(width=600, height=800)
        page.insert_text((80, 100), 'RESTORE_THIS_7312', fontsize=14)
        page.insert_text((80, 250), 'OTHER_SECRET_8631', fontsize=14)
        document = upload(client, auth, matter, source.tobytes())
    first = redact(client, auth, document)
    assert first.status_code == 200, first.text
    records = client.get(f'/documents/{document["id"]}/pages/redactions', headers=auth).json()
    assert len(records) == 1
    second = redact(client, auth, document, expected_revision=2,
                    rect_y=.28, rect_height=.06)
    assert second.status_code == 200, second.text
    later = client.post(f'/documents/{document["id"]}/pages/text-overlay', headers=auth,
                        json={'page_index': 0, 'x': .2, 'y': .6,
                              'text': 'LATER_OUTSIDE_5297', 'font_size': 12})
    assert later.status_code == 200, later.text
    inside = client.post(f'/documents/{document["id"]}/pages/text-overlay', headers=auth,
                         json={'page_index': 0, 'x': .2, 'y': .12,
                               'text': 'LATER_INSIDE_9823', 'font_size': 12})
    assert inside.status_code == 200, inside.text
    stale = client.post(f'/documents/{document["id"]}/pages/redactions/{records[0]["id"]}/remove',
                        headers=auth, json={'expected_revision': 3})
    assert stale.status_code == 409
    restored = client.post(f'/documents/{document["id"]}/pages/redactions/{records[0]["id"]}/remove',
                           headers=auth, json={'expected_revision': inside.json()['revision']})
    assert restored.status_code == 200, restored.text
    assert restored.json()['revision'] == 6
    active = client.get(f'/documents/{document["id"]}/pages/redactions', headers=auth).json()
    assert len(active) == 1 and active[0]['id'] != records[0]['id']
    data = client.get(f'/documents/{document["id"]}/pdf', headers=auth).content
    with pymupdf.open(stream=data, filetype='pdf') as pdf:
        text = pdf[0].get_text()
        assert 'RESTORE_THIS_7312' in text
        assert 'LATER_OUTSIDE_5297' in text
        assert 'LATER_INSIDE_9823' not in text
        assert 'OTHER_SECRET_8631' not in text
    assert b'OTHER_SECRET_8631' not in data
    build = client.post(f'/matters/{matter["id"]}/build', headers=auth)
    assert build.status_code == 202, build.text
    for _ in range(200):
        job = client.get(f'/jobs/{build.json()["id"]}', headers=auth).json()
        if job['status'] not in ('queued', 'running'):
            break
        time.sleep(.03)
    assert job['status'] == 'completed', job
    exported = client.get(f'/matters/{matter["id"]}/exports/{job["result"]["id"]}/combined.pdf',
                          headers=auth).content
    with pymupdf.open(stream=exported, filetype='pdf') as pdf:
        content = ''.join(page.get_text() for page in pdf)
        assert 'RESTORE_THIS_7312' in content
        assert 'LATER_OUTSIDE_5297' in content
        assert 'OTHER_SECRET_8631' not in content
    assert b'OTHER_SECRET_8631' not in exported


def test_redaction_selection_tracks_undo_and_redo(client, auth, matter, make_pdf):
    document = upload(client, auth, matter, make_pdf(1, 'PRIVATE_1832'))
    applied = redact(client, auth, document)
    assert applied.status_code == 200
    base = f'/documents/{document["id"]}/pages/redactions'
    record = client.get(base, headers=auth).json()[0]
    history = f'/matters/{matter["id"]}/history'
    assert client.post(history + '/undo', headers=auth).status_code == 200
    assert client.get(base, headers=auth).json() == []
    assert client.post(history + '/redo', headers=auth).status_code == 200
    assert len(client.get(base, headers=auth).json()) == 1
    current = client.app.state.store.get_document(document['id'])
    removed = client.post(base + f'/{record["id"]}/remove', headers=auth,
                          json={'expected_revision': current['revision']})
    assert removed.status_code == 200, removed.text
    assert client.get(base, headers=auth).json() == []
    assert client.post(history + '/undo', headers=auth).status_code == 200
    assert len(client.get(base, headers=auth).json()) == 1
    assert client.post(history + '/redo', headers=auth).status_code == 200
    assert client.get(base, headers=auth).json() == []


def test_applied_redaction_remains_selectable_after_portable_restore(
        client, auth, matter, make_pdf):
    document = upload(client, auth, matter, make_pdf(1, 'RESTORE_AFTER_BACKUP_6123'))
    assert redact(client, auth, document).status_code == 200
    backup = client.get(f'/matters/{matter["id"]}/portable', headers=auth)
    assert backup.status_code == 200, backup.text
    restored = client.post('/matters/restore', headers=auth,
                           files={'file': ('synthetic.cbmatter', backup.content, 'application/zip')})
    assert restored.status_code == 201, restored.text
    new_doc = client.get(f'/matters/{restored.json()["id"]}/documents', headers=auth).json()[0]
    base = f'/documents/{new_doc["id"]}/pages/redactions'
    records = client.get(base, headers=auth).json()
    assert len(records) == 1
    removed = client.post(base + f'/{records[0]["id"]}/remove', headers=auth,
                          json={'expected_revision': new_doc['revision']})
    assert removed.status_code == 200, removed.text
    with pymupdf.open(stream=client.get(f'/documents/{new_doc["id"]}/pdf', headers=auth).content,
                      filetype='pdf') as pdf:
        assert 'RESTORE_AFTER_BACKUP_6123' in pdf[0].get_text()


def test_selective_restore_refuses_reordered_pages(client, auth, matter, make_pdf):
    document = upload(client, auth, matter, make_pdf(2, 'PAGE_SECRET_7123'))
    applied = redact(client, auth, document)
    assert applied.status_code == 200
    record = client.get(f'/documents/{document["id"]}/pages/redactions', headers=auth).json()[0]
    reordered = client.post(f'/documents/{document["id"]}/pages/reorder', headers=auth,
                            json={'order': [1, 0], 'expected_revision': 2})
    assert reordered.status_code == 200, reordered.text
    listed = client.get(f'/documents/{document["id"]}/pages/redactions', headers=auth).json()
    assert listed[0]['structure_changed'] == 1
    refused = client.post(f'/documents/{document["id"]}/pages/redactions/{record["id"]}/remove',
                          headers=auth, json={'expected_revision': reordered.json()['revision']})
    assert refused.status_code == 422
    assert client.app.state.store.get_document(document['id'])['revision'] == 3


def upload(client, auth, matter, content):
    result = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
                         files=[('files', ('redaction.pdf', content, 'application/pdf'))])
    assert result.status_code == 201, result.text
    return result.json()[0]


def redact(client, auth, document, **changes):
    return client.post(f'/documents/{document["id"]}/pages/redact', headers=auth, json={
        'page_index': 0, 'expected_revision': 1,
        'rect_x': .1, 'rect_y': .08, 'rect_width': .65, 'rect_height': .08,
        **changes})


def test_redaction_removes_text_metadata_attachments_and_annotations_from_working_and_export(
        client, auth, matter):
    with pymupdf.open() as source:
        page = source.new_page(width=600, height=800)
        page.insert_text((80, 100), 'SECRET_NATIVE_7351', fontsize=18)
        page.insert_text((80, 250), 'PUBLIC_TEXT_2452', fontsize=18)
        page.add_text_annot((90, 100), 'SECRET_NOTE_9183')
        page.insert_link({'kind': pymupdf.LINK_URI, 'from': pymupdf.Rect(80, 80, 200, 110),
                          'uri': 'https://example.test/SECRET_LINK_1845'})
        source.set_metadata({'title': 'SECRET_META_5561'})
        source.embfile_add('secret.txt', b'SECRET_ATTACHMENT_4472')
        source.set_toc([[1, 'SECRET_BOOKMARK_3241', 1]])
        original = source.tobytes()
    document = upload(client, auth, matter, original)
    store = client.app.state.store
    original_hash = hashlib.sha256(original).hexdigest()
    assert store.sha256(store.original_path(document)) == original_hash
    changed = redact(client, auth, document)
    assert changed.status_code == 200, changed.text
    assert changed.json()['revision'] == 2
    working = client.get(f'/documents/{document["id"]}/pdf', headers=auth).content
    for marker in (b'SECRET_NATIVE_7351', b'SECRET_NOTE_9183', b'SECRET_LINK_1845',
                   b'SECRET_META_5561', b'SECRET_ATTACHMENT_4472', b'SECRET_BOOKMARK_3241'):
        assert marker not in working
    with pymupdf.open(stream=working, filetype='pdf') as pdf:
        assert 'SECRET_NATIVE_7351' not in pdf[0].get_text()
        assert 'PUBLIC_TEXT_2452' in pdf[0].get_text()
        assert not list(pdf[0].annots() or ())
        assert not pdf[0].get_links()
        assert not pdf.embfile_names()
        assert not pdf.get_toc()
        assert not pdf.metadata.get('title')
        pix = pdf[0].get_pixmap()
        # The text's former baseline lies inside the reviewed black region.
        assert pix.pixel(100, 95)[:3] == (0, 0, 0)
    assert store.sha256(store.original_path(document)) == original_hash

    build = client.post(f'/matters/{matter["id"]}/build', headers=auth)
    assert build.status_code == 202, build.text
    for _ in range(200):
        job = client.get(f'/jobs/{build.json()["id"]}', headers=auth).json()
        if job['status'] not in ('queued', 'running'):
            break
        time.sleep(.03)
    assert job['status'] == 'completed', job
    exported = client.get(f'/matters/{matter["id"]}/exports/{job["result"]["id"]}/combined.pdf', headers=auth).content
    with pymupdf.open(stream=exported, filetype='pdf') as pdf:
        assert 'SECRET_NATIVE_7351' not in ''.join(page.get_text() for page in pdf)
        assert 'PUBLIC_TEXT_2452' in ''.join(page.get_text() for page in pdf)
    assert b'SECRET_ATTACHMENT_4472' not in exported
    archive_bytes = client.get(
        f'/matters/{matter["id"]}/exports/{job["result"]["id"]}/filing-package.zip',
        headers=auth).content
    with zipfile.ZipFile(io.BytesIO(archive_bytes)) as archive:
        for name in archive.namelist():
            content = archive.read(name)
            for marker in (b'SECRET_NATIVE_7351', b'SECRET_NOTE_9183',
                           b'SECRET_META_5561', b'SECRET_ATTACHMENT_4472'):
                assert marker not in content


@pytest.mark.parametrize('rotation', [0, 90, 180, 270])
def test_redaction_removes_pixels_and_text_on_rotated_pages(client, auth, matter, rotation):
    image = Image.new('RGB', (200, 100), (255, 0, 0))
    buffer = io.BytesIO()
    image.save(buffer, format='PNG')
    with pymupdf.open() as source:
        page = source.new_page(width=600, height=800)
        page.insert_image(pymupdf.Rect(60, 60, 260, 160), stream=buffer.getvalue())
        page.insert_text((80, 100), 'SECRET_PIXELS_7532', fontsize=12)
        page.insert_text((80, 270), 'PUBLIC_2981', fontsize=12)
        page.set_rotation(rotation)
        display = pymupdf.Rect(80, 70, 200, 145) * page.rotation_matrix
        rect = page.rect
        region = {'rect_x': display.x0 / rect.width, 'rect_y': display.y0 / rect.height,
                  'rect_width': display.width / rect.width, 'rect_height': display.height / rect.height}
        original = source.tobytes()
    document = upload(client, auth, matter, original)
    changed = redact(client, auth, document, **region)
    assert changed.status_code == 200, changed.text
    working = client.get(f'/documents/{document["id"]}/pdf', headers=auth).content
    with pymupdf.open(stream=working, filetype='pdf') as pdf:
        page = pdf[0]
        assert page.rotation == rotation
        assert 'SECRET_PIXELS_7532' not in page.get_text()
        assert 'PUBLIC_2981' in page.get_text()
        pix = page.get_pixmap()
        cx, cy = int(display.x0 + display.width / 2), int(display.y0 + display.height / 2)
        assert pix.pixel(cx, cy)[:3] == (0, 0, 0)
        outside = pymupdf.Point(70, 110) * page.rotation_matrix
        assert pix.pixel(int(outside.x), int(outside.y))[:3] == (255, 0, 0)
        # The source image must not be extractable as an intact red image.
        for image_info in page.get_images(full=True):
            extracted = pdf.extract_image(image_info[0])['image']
            with Image.open(io.BytesIO(extracted)) as extracted_image:
                assert extracted_image.convert('RGB').getpixel((extracted_image.width // 2,
                                                                extracted_image.height // 2)) != (255, 0, 0)
    record = client.get(f'/documents/{document["id"]}/pages/redactions', headers=auth).json()[0]
    restored = client.post(f'/documents/{document["id"]}/pages/redactions/{record["id"]}/remove',
                           headers=auth, json={'expected_revision': changed.json()['revision']})
    assert restored.status_code == 200, restored.text
    with pymupdf.open(stream=client.get(f'/documents/{document["id"]}/pdf', headers=auth).content,
                      filetype='pdf') as pdf:
        assert 'SECRET_PIXELS_7532' in pdf[0].get_text()
        assert pdf[0].rotation == rotation
        for image_info in pdf[0].get_images(full=True):
            with Image.open(io.BytesIO(pdf.extract_image(image_info[0])['image'])) as extracted:
                if extracted.size == (200, 100):
                    assert extracted.convert('RGB').getpixel((100, 50)) != (255, 0, 0)


def test_redaction_rejects_stale_outside_and_pending_marks(client, auth, matter, make_pdf):
    document = upload(client, auth, matter, make_pdf(1, 'SECRET'))
    base = f'/documents/{document["id"]}'
    assert redact(client, auth, document, rect_x=.9, rect_width=.2).status_code == 422
    store = client.app.state.store
    with pymupdf.open(store.working_path(document)) as source:
        source[0].add_redact_annot(pymupdf.Rect(80, 200, 200, 220))
        source.saveIncr()
    pending = redact(client, auth, document)
    assert pending.status_code == 422
    assert 'pending redaction' in pending.text
    assert store.get_document(document['id'])['revision'] == 1

    other = upload(client, auth, matter, make_pdf(1, 'ANOTHER'))
    first = redact(client, auth, other)
    assert first.status_code == 200, first.text
    stale = redact(client, auth, other)
    assert stale.status_code == 409


def test_redaction_signature_review_is_not_a_console_conflict(client, auth, matter, make_pdf):
    document = upload(client, auth, matter, make_pdf(1, 'SYNTHETIC SECRET'))
    store = client.app.state.store
    original_hash = store.sha256(store.original_path(document))
    marked = client.patch(f'/documents/{document["id"]}', headers=auth,
                          json={'visible_signature_status': 'present'})
    assert marked.status_code == 200, marked.text
    challenge_headers = {**auth, 'X-CourtBundle-Review-Protocol': 'challenge'}
    challenge = client.post(f'/documents/{document["id"]}/pages/redact',
                            headers=challenge_headers, json={
        'page_index': 0, 'expected_revision': 1,
        'rect_x': .1, 'rect_y': .08, 'rect_width': .65, 'rect_height': .08,
        'acknowledge_signature_risk': False})
    assert challenge.status_code == 200, challenge.text
    assert challenge.json()['review_required'] is True
    assert store.get_document(document['id'])['revision'] == 1
    approved = client.post(f'/documents/{document["id"]}/pages/redact',
                           headers=challenge_headers, json={
        'page_index': 0, 'expected_revision': 1,
        'rect_x': .1, 'rect_y': .08, 'rect_width': .65, 'rect_height': .08,
        'acknowledge_signature_risk': True})
    assert approved.status_code == 200, approved.text
    assert approved.json()['revision'] == 2
    assert store.sha256(store.original_path(document)) == original_hash
    stale = client.post(f'/documents/{document["id"]}/pages/redact',
                        headers=challenge_headers, json={
        'page_index': 0, 'expected_revision': 1,
        'rect_x': .1, 'rect_y': .08, 'rect_width': .65, 'rect_height': .08,
        'acknowledge_signature_risk': False})
    assert stale.status_code == 409
    assert 'revision changed' in stale.json()['detail']


def test_redaction_is_area_scoped_and_omits_stale_link_and_bookmark_text_from_report(
        client, auth, matter):
    with pymupdf.open() as source:
        page = source.new_page(width=600, height=800)
        page.insert_text((80, 100), 'REPEATED_7312', fontsize=16)
        page.insert_text((80, 250), 'REPEATED_7312', fontsize=16)
        page.insert_text((80, 120), 'HIDDEN_8429', fontsize=12, render_mode=3)
        document = upload(client, auth, matter, source.tobytes())
    base = f'/matters/{matter["id"]}'
    bookmark = {'id': 'a' * 32, 'title': 'STALE_SECRET_BOOKMARK_7351',
                'document_id': document['id'], 'page_index': 0,
                'target_revision': document['revision'], 'parent_id': None}
    link = {'id': 'b' * 32, 'title': 'STALE_SECRET_LINK_7351',
            'source_document_id': document['id'], 'source_page_index': 0,
            'source_revision': document['revision'], 'rect_x': .1, 'rect_y': .1,
            'rect_width': .2, 'rect_height': .04, 'kind': 'external',
            'target_document_id': None, 'target_page_index': None,
            'target_revision': None, 'uri': 'https://example.org/STALE_SECRET_URI_7351'}
    assert client.put(base + '/bookmarks', headers=auth, json={'entries': [bookmark]}).status_code == 200
    assert client.put(base + '/links', headers=auth, json={'entries': [link]}).status_code == 200
    changed = redact(client, auth, document, rect_height=.1)
    assert changed.status_code == 200, changed.text
    working = client.get(f'/documents/{document["id"]}/pdf', headers=auth).content
    with pymupdf.open(stream=working, filetype='pdf') as pdf:
        text = pdf[0].get_text()
        assert text.count('REPEATED_7312') == 1
        assert 'HIDDEN_8429' not in text
    assert b'HIDDEN_8429' not in working
    build = client.post(base + '/build', headers=auth)
    assert build.status_code == 202, build.text
    for _ in range(200):
        job = client.get(f'/jobs/{build.json()["id"]}', headers=auth).json()
        if job['status'] not in ('queued', 'running'):
            break
        time.sleep(.03)
    assert job['status'] == 'completed', job
    folder = client.app.state.store.exports_root(matter['id']) / job['result']['id']
    report_bytes = (folder / 'validation-report.json').read_bytes()
    for marker in (b'STALE_SECRET_BOOKMARK_7351', b'STALE_SECRET_LINK_7351',
                   b'STALE_SECRET_URI_7351', b'HIDDEN_8429'):
        assert marker not in report_bytes
    report = json.loads(report_bytes)
    assert report['manual_bookmark_inputs'][0]['title'] is None
    assert report['reviewed_link_inputs'][0]['title'] is None
    assert report['reviewed_link_inputs'][0]['uri'] is None
