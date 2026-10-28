import io
import json

import pymupdf
import pytest

from test_api import upload, wait_for_job


def image_bytes():
    pix = pymupdf.Pixmap(pymupdf.csRGB, pymupdf.IRect(0, 0, 40, 24), False)
    pix.clear_with(0xD22B2B)
    return pix.tobytes('png')


def region(**extra):
    return {
        'page_index': 0, 'expected_revision': 1,
        'rect_x': .18, 'rect_y': .22, 'rect_width': .24, 'rect_height': .08,
        **extra,
    }


def assert_image_aspect(page):
    xref = page.get_images(full=True)[0][0]
    rect = page.get_image_rects(xref)[0]
    assert rect.width / rect.height == pytest.approx(40 / 24, rel=.02)


def test_working_image_insert_revisions_and_undo(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, name='working-image.pdf', pages=1)
    store = client.app.state.store
    original_hash = store.sha256(store.original_path(doc))
    endpoint = f'/documents/{doc["id"]}/pages/image-overlay'
    fields = {key: str(value) for key, value in region().items()}
    files = {'file': ('stamp.png', image_bytes(), 'image/png')}
    inserted = client.post(endpoint, headers=auth, data=fields, files=files)
    assert inserted.status_code == 200, inserted.text
    assert inserted.json()['revision'] == doc['revision'] + 1
    with pymupdf.open(store.working_path(inserted.json())) as pdf:
        assert len(pdf[0].get_images(full=True)) == 1
        assert_image_aspect(pdf[0])
    assert store.sha256(store.original_path(doc)) == original_hash
    job = wait_for_job(client, auth, client.post(
        f'/matters/{matter["id"]}/build', headers=auth).json()['id'])
    assert job['status'] == 'completed', job['error']
    combined = store.exports_root(matter['id']) / job['result']['id'] / 'combined.pdf'
    with pymupdf.open(combined) as pdf:
        assert any(page.get_images(full=True) for page in pdf)
        assert_image_aspect(next(page for page in pdf if page.get_images(full=True)))
    stale = client.post(endpoint, headers=auth, data=fields, files=files)
    assert stale.status_code == 409
    undone = client.post(f'/matters/{matter["id"]}/history/undo', headers=auth)
    assert undone.status_code == 200
    current = next(item for item in client.get(
        f'/matters/{matter["id"]}/documents', headers=auth).json() if item['id'] == doc['id'])
    with pymupdf.open(store.working_path(current)) as pdf:
        assert not pdf[0].get_images(full=True)


def test_editable_pdf_marks_move_and_save_with_real_redaction(client, auth, matter):
    with pymupdf.open() as source:
        page = source.new_page(width=600, height=800)
        page.insert_text((100, 105), 'SECRET_EDITABLE_7319', fontsize=15)
        page.insert_text((100, 420), 'PUBLIC_EDITABLE_2681', fontsize=15)
        source.set_metadata({'title': 'SECRET_META_5524'})
        source.embfile_add('secret.txt', b'SECRET_ATTACHMENT_5524')
        original = source.tobytes()
    response = client.post(f'/matters/{matter["id"]}/documents?included=true', headers=auth,
        files=[('files', ('editable.pdf', original, 'application/pdf'))])
    assert response.status_code == 201, response.text
    doc = response.json()[0]
    base = f'/documents/{doc["id"]}'
    original_hash = client.app.state.store.sha256(client.app.state.store.original_path(doc))
    entries = [
        dict(kind='box', rect_x=.1, rect_y=.25, rect_width=.2, rect_height=.1,
             body='', color='blue', output_scope='pdf'),
        dict(kind='ellipse', rect_x=.4, rect_y=.25, rect_width=.2, rect_height=.1,
             body='', color='green', output_scope='pdf'),
        dict(kind='text', rect_x=.1, rect_y=.5, rect_width=.3, rect_height=.07,
             body='MOVEABLE TEXT', color='blue', output_scope='pdf'),
        dict(kind='draw', rect_x=.62, rect_y=.2, rect_width=.2, rect_height=.1,
             body='{"v":1,"points":[[0,0],[0.5,1],[1,0]]}', color='pink', output_scope='pdf'),
        dict(kind='line', rect_x=.62, rect_y=.32, rect_width=.2, rect_height=.08,
             body='', color='blue', output_scope='pdf'),
        dict(kind='arrow', rect_x=.62, rect_y=.42, rect_width=.2, rect_height=.08,
             body='', color='blue', output_scope='pdf'),
        dict(kind='check', rect_x=.62, rect_y=.52, rect_width=.1, rect_height=.07,
             body='', color='green', output_scope='pdf'),
        dict(kind='cross', rect_x=.74, rect_y=.52, rect_width=.1, rect_height=.07,
             body='', color='pink', output_scope='pdf'),
        dict(kind='date', rect_x=.62, rect_y=.62, rect_width=.22, rect_height=.08,
             body='2026-10-22', color='blue', output_scope='pdf'),
        dict(kind='text', rect_x=.17, rect_y=.095, rect_width=.3, rect_height=.04,
             body='SECRET_OVERLAY_9821', color='blue', output_scope='pdf'),
        dict(kind='redaction', rect_x=.15, rect_y=.09, rect_width=.37, rect_height=.055,
             body='', color='yellow', output_scope='pdf'),
    ]
    saved = []
    for entry in entries:
        result = client.post(base + '/annotations', headers=auth,
            json={'page_index': 0, 'expected_revision': 1, **entry})
        assert result.status_code == 201, result.text
        saved = result.json()
    box = next(item for item in saved if item['kind'] == 'box')
    moved = client.patch(f'/annotations/{box["id"]}', headers=auth, json={
        'expected_revision': 1, 'rect_x': .2, 'rect_y': .28,
        'rect_width': .2, 'rect_height': .1, 'body': '', 'color': 'blue'})
    assert moved.status_code == 200, moved.text
    assert next(item for item in moved.json() if item['id'] == box['id'])['rect_x'] == .2
    text_mark = next(item for item in moved.json() if item['kind'] == 'text' and item['body'] == 'MOVEABLE TEXT')
    moved_text = client.patch(f'/annotations/{text_mark["id"]}', headers=auth, json={
        'expected_revision': 1, 'rect_x': .35, 'rect_y': .5,
        'rect_width': .3, 'rect_height': .07, 'body': 'MOVEABLE TEXT', 'color': 'blue'})
    assert moved_text.status_code == 200, moved_text.text
    raw = client.get(base + '/pdf?revision=1', headers=auth)
    with pymupdf.open(stream=raw.content, filetype='pdf') as pdf:
        assert 'SECRET_EDITABLE_7319' in pdf[0].get_text()
    snapshot = client.get(base + '/pdf?revision=1&editable=true', headers=auth)
    assert snapshot.status_code == 200, snapshot.text
    with pymupdf.open(stream=snapshot.content, filetype='pdf') as pdf:
        assert 'SECRET_EDITABLE_7319' not in pdf[0].get_text()
        assert 'SECRET_OVERLAY_9821' not in pdf[0].get_text()
        assert 'PUBLIC_EDITABLE_2681' in pdf[0].get_text()
        assert 'MOVEABLE TEXT' in pdf[0].get_text()
        assert pdf[0].search_for('MOVEABLE TEXT')[0].x0 == pytest.approx(210, abs=3)
        assert '2026-10-22' in pdf[0].get_text()
        assert not pdf.embfile_names()
        assert 'SECRET_META_5524' not in str(pdf.metadata)
        assert any(abs(path['rect'].x0 - 120) < 3 and abs(path['rect'].y0 - 224) < 3
                   for path in pdf[0].get_drawings())
        assert pdf[0].get_pixmap().pixel(180, 100)[:3] == (0, 0, 0)
    job = wait_for_job(client, auth, client.post(
        f'/matters/{matter["id"]}/build', headers=auth).json()['id'])
    assert job['status'] == 'completed', job['error']
    folder = client.app.state.store.exports_root(matter['id']) / job['result']['id']
    with pymupdf.open(folder / 'combined.pdf') as pdf:
        assert 'SECRET_EDITABLE_7319' not in pdf[1].get_text()
        assert 'SECRET_OVERLAY_9821' not in pdf[1].get_text()
        assert 'MOVEABLE TEXT' in pdf[1].get_text()
    assert client.app.state.store.sha256(client.app.state.store.original_path(doc)) == original_hash


def test_image_highlight_comment_edit_delete_undo_export_and_portable(
        client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, name='annotated.pdf', pages=1)
    store = client.app.state.store
    original_hash = store.sha256(store.original_path(doc))
    base = f'/matters/{matter["id"]}'

    image = client.post(f'/documents/{doc["id"]}/annotations/image', headers=auth,
        data={key: str(value) for key, value in region().items()},
        files={'file': ('synthetic-mark.png', image_bytes(), 'image/png')})
    assert image.status_code == 201, image.text
    image_item = next(item for item in image.json() if item['kind'] == 'image')
    assert image_item['resolved'] is True
    assert client.get(f'/annotations/{image_item["id"]}/asset', headers=auth).content.startswith(b'\x89PNG')

    highlight = client.post(f'/documents/{doc["id"]}/annotations', headers=auth,
        json=region(kind='highlight', body='Key passage', color='green'))
    assert highlight.status_code == 201, highlight.text
    comment = client.post(f'/documents/{doc["id"]}/annotations', headers=auth,
        json=region(kind='comment', body='Check this proposition', color='pink'))
    assert comment.status_code == 201, comment.text
    comment_item = next(item for item in comment.json() if item['kind'] == 'comment')

    edited = client.patch(f'/annotations/{comment_item["id"]}', headers=auth, json={
        'expected_revision': 1, 'rect_x': .55, 'rect_y': .3,
        'rect_width': .08, 'rect_height': .06,
        'body': 'Reviewed note text', 'color': 'blue'})
    assert edited.status_code == 200, edited.text
    assert next(item for item in edited.json() if item['id'] == comment_item['id'])['body'] == 'Reviewed note text'

    highlight_item = next(item for item in edited.json() if item['kind'] == 'highlight')
    removed = client.delete(f'/annotations/{highlight_item["id"]}', headers=auth)
    assert removed.status_code == 200
    assert all(item['kind'] != 'highlight' for item in removed.json())
    assert client.post(base + '/history/undo', headers=auth).status_code == 200
    assert any(item['kind'] == 'highlight' for item in client.get(base + '/annotations', headers=auth).json())

    job = wait_for_job(client, auth, client.post(base + '/build', headers=auth).json()['id'])
    assert job['status'] == 'completed', job['error']
    folder = store.exports_root(matter['id']) / job['result']['id']
    with pymupdf.open(folder / 'combined.pdf') as pdf:
        source_page = pdf[1]
        assert source_page.get_images(full=True)
        assert_image_aspect(source_page)
        assert list(source_page.annots() or ()) == []
    report = json.loads((folder / 'validation-report.json').read_text())
    assert report['page_annotation_summary'] == {
        'image_overlays_inserted': 1,
        'unresolved_image_overlays': 0,
        'private_review_annotations_omitted': 2,
        'editable_marks_inserted': 0,
        'redactions_applied': 0,
    }
    assert report['interactive_content_policy']['courtbundle_highlights_comments'].startswith('private')
    report_text = json.dumps(report)
    assert 'Reviewed note text' not in report_text
    assert 'Key passage' not in report_text
    assert all('asset_relpath' not in item and 'body' not in item
               for item in report['page_annotation_inputs'])
    manifest_text = (folder / 'export_manifest.json').read_text()
    assert 'Reviewed note text' not in manifest_text
    assert 'Key passage' not in manifest_text
    assert store.sha256(store.original_path(doc)) == original_hash

    package = client.get(base + '/portable', headers=auth)
    restored = client.post('/matters/restore', headers=auth,
        files={'file': ('annotated.cbmatter', package.content, 'application/zip')})
    assert restored.status_code == 201, restored.text
    restored_base = f'/matters/{restored.json()["id"]}'
    restored_items = client.get(restored_base + '/annotations', headers=auth).json()
    assert sorted(item['kind'] for item in restored_items) == ['comment', 'highlight', 'image']
    restored_image = next(item for item in restored_items if item['kind'] == 'image')
    assert client.get(f'/annotations/{restored_image["id"]}/asset', headers=auth).content == image_bytes()


def test_annotation_validation_and_revision_invalidation(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, name='source.pdf', pages=1)
    empty_comment = client.post(f'/documents/{doc["id"]}/annotations', headers=auth,
        json=region(kind='comment', body='   ', color='yellow'))
    assert empty_comment.status_code == 422
    outside = client.post(f'/documents/{doc["id"]}/annotations', headers=auth,
        json=region(kind='highlight', body='', color='yellow', rect_x=.9, rect_width=.2))
    assert outside.status_code == 422
    bad_image = client.post(f'/documents/{doc["id"]}/annotations/image', headers=auth,
        data={key: str(value) for key, value in region().items()},
        files={'file': ('bad.gif', b'not an image', 'image/gif')})
    assert bad_image.status_code == 422
    renamed_image = client.post(f'/documents/{doc["id"]}/annotations/image', headers=auth,
        data={key: str(value) for key, value in region().items()},
        files={'file': ('renamed.jpg', image_bytes(), 'image/jpeg')})
    assert renamed_image.status_code == 422

    created = client.post(f'/documents/{doc["id"]}/annotations/image', headers=auth,
        data={key: str(value) for key, value in region().items()},
        files={'file': ('mark.png', image_bytes(), 'image/png')})
    assert created.status_code == 201, created.text
    image_item = created.json()[0]
    rotated = client.post(f'/documents/{doc["id"]}/pages/rotate', headers=auth,
                          json={'page_index': 0, 'degrees': 90})
    assert rotated.status_code == 200
    stale = client.get(f'/matters/{matter["id"]}/annotations', headers=auth).json()[0]
    assert stale['resolved'] is False
    update = client.patch(f'/annotations/{image_item["id"]}', headers=auth, json={
        'expected_revision': 1, 'rect_x': .2, 'rect_y': .2,
        'rect_width': .2, 'rect_height': .1, 'body': '', 'color': 'yellow'})
    assert update.status_code == 409

    job = wait_for_job(client, auth,
        client.post(f'/matters/{matter["id"]}/build', headers=auth).json()['id'])
    assert job['status'] == 'completed', job['error']
    report = json.loads((client.app.state.store.exports_root(matter['id']) /
                         job['result']['id'] / 'validation-report.json').read_text())
    assert report['page_annotation_summary']['image_overlays_inserted'] == 0
    assert report['page_annotation_summary']['unresolved_image_overlays'] == 1
    assert any(item['code'] == 'UNRESOLVED_IMAGE_OVERLAYS' for item in report['checks'])


def test_stale_editable_redaction_blocks_saved_pdf_and_bundle(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, name='stale-redaction.pdf', pages=1)
    created = client.post(f'/documents/{doc["id"]}/annotations', headers=auth,
        json=region(kind='redaction', body='', color='yellow', output_scope='pdf'))
    assert created.status_code == 201, created.text
    rotated = client.post(f'/documents/{doc["id"]}/pages/rotate', headers=auth,
                          json={'page_index': 0, 'degrees': 90})
    assert rotated.status_code == 200, rotated.text
    snapshot = client.get(f'/documents/{doc["id"]}/pdf?revision=2&editable=true', headers=auth)
    assert snapshot.status_code == 409
    checks = client.get(f'/matters/{matter["id"]}/validate', headers=auth)
    assert checks.status_code == 200
    assert any(item['code'] == 'UNRESOLVED_EDITABLE_PDF_MARK' for item in checks.json())


def test_existing_private_shape_can_be_explicitly_included_in_pdf(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, name='old-private-box.pdf', pages=1)
    created = client.post(f'/documents/{doc["id"]}/annotations', headers=auth,
        json=region(kind='box', body='', color='blue'))
    assert created.status_code == 201, created.text
    item = created.json()[0]
    assert item['output_scope'] == 'private'
    changed = client.patch(f'/annotations/{item["id"]}', headers=auth, json={
        'expected_revision': 1, 'rect_x': .18, 'rect_y': .22,
        'rect_width': .24, 'rect_height': .08, 'body': '',
        'color': 'blue', 'output_scope': 'pdf'})
    assert changed.status_code == 200, changed.text
    assert changed.json()[0]['output_scope'] == 'pdf'
    snapshot = client.get(f'/documents/{doc["id"]}/pdf?revision=1&editable=true', headers=auth)
    assert snapshot.status_code == 200, snapshot.text
    with pymupdf.open(stream=snapshot.content, filetype='pdf') as pdf:
        assert pdf[0].get_drawings()


def test_pdf_marks_follow_same_geometry_working_edits(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, name='same-geometry-edit.pdf', pages=1)
    for kind, scope in (('box', 'pdf'), ('highlight', 'private')):
        result = client.post(f'/documents/{doc["id"]}/annotations', headers=auth,
            json=region(kind=kind, body='', color='blue', output_scope=scope))
        assert result.status_code == 201, result.text
    fields = {key: str(value) for key, value in region(
        rect_x=.65, rect_y=.65, rect_width=.1, rect_height=.1).items()}
    inserted = client.post(f'/documents/{doc["id"]}/pages/image-overlay', headers=auth,
        data=fields, files={'file': ('stamp.png', image_bytes(), 'image/png')})
    assert inserted.status_code == 200, inserted.text
    annotations = client.get(f'/matters/{matter["id"]}/annotations', headers=auth).json()
    assert next(item for item in annotations if item['kind'] == 'box')['resolved']
    assert not next(item for item in annotations if item['kind'] == 'highlight')['resolved']
    snapshot = client.get(f'/documents/{doc["id"]}/pdf?revision=2&editable=true', headers=auth)
    assert snapshot.status_code == 200, snapshot.text


def test_editable_redaction_save_uses_rotated_display_coordinates(client, auth, matter):
    with pymupdf.open() as source:
        page = source.new_page(width=600, height=800)
        page.insert_text((100, 100), 'ROTATED_SECRET_3702', fontsize=13)
        page.set_rotation(90)
        displayed = pymupdf.Rect(85, 80, 265, 115) * page.rotation_matrix
        screen = page.rect
        region = dict(rect_x=displayed.x0 / screen.width,
            rect_y=displayed.y0 / screen.height,
            rect_width=displayed.width / screen.width,
            rect_height=displayed.height / screen.height)
        original = source.tobytes()
    uploaded = client.post(f'/matters/{matter["id"]}/documents?included=true', headers=auth,
        files=[('files', ('rotated-editable.pdf', original, 'application/pdf'))])
    assert uploaded.status_code == 201, uploaded.text
    doc = uploaded.json()[0]
    created = client.post(f'/documents/{doc["id"]}/annotations', headers=auth,
        json=dict(kind='redaction', page_index=0, expected_revision=1,
                  body='', color='yellow', output_scope='pdf', **region))
    assert created.status_code == 201, created.text
    snapshot = client.get(f'/documents/{doc["id"]}/pdf?revision=1&editable=true', headers=auth)
    assert snapshot.status_code == 200, snapshot.text
    with pymupdf.open(stream=snapshot.content, filetype='pdf') as pdf:
        assert pdf[0].rotation == 90
        assert 'ROTATED_SECRET_3702' not in pdf[0].get_text()
        center = displayed.tl + (displayed.br - displayed.tl) * .5
        assert pdf[0].get_pixmap().pixel(int(center.x), int(center.y))[:3] == (0, 0, 0)


def test_independent_private_shapes_round_trip_without_entering_export(
        client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, name='review-shapes.pdf', pages=1)
    store = client.app.state.store
    original_hash = store.sha256(store.original_path(doc))
    kinds = ('draw', 'box', 'ellipse', 'line', 'arrow', 'check', 'cross', 'date')
    entries = []
    for kind in kinds:
        response = client.post(f'/documents/{doc["id"]}/annotations', headers=auth,
            json=region(kind=kind, body=('2026-10-22' if kind == 'date' else
                '{"v":1,"points":[[0,0],[1,1]]}' if kind == 'draw' else ''), color='blue'))
        assert response.status_code == 201, response.text
        entries = response.json()
    assert {item['kind'] for item in entries} == set(kinds)
    box = next(item for item in entries if item['kind'] == 'box')
    moved = client.patch(f'/annotations/{box["id"]}', headers=auth, json={
        'expected_revision': doc['revision'], 'rect_x': .4, 'rect_y': .3,
        'rect_width': .24, 'rect_height': .08, 'body': '', 'color': 'blue'})
    assert moved.status_code == 200, moved.text
    assert next(item for item in moved.json() if item['id'] == box['id'])['rect_x'] == .4
    assert client.post(f'/matters/{matter["id"]}/history/undo', headers=auth).status_code == 200
    assert next(item for item in client.get(f'/matters/{matter["id"]}/annotations', headers=auth).json()
                if item['id'] == box['id'])['rect_x'] == .18
    invalid = client.post(f'/documents/{doc["id"]}/annotations', headers=auth,
        json=region(kind='date', body='not a date'))
    assert invalid.status_code == 422
    impossible_date = client.post(f'/documents/{doc["id"]}/annotations', headers=auth,
        json=region(kind='date', body='2026-13-99'))
    assert impossible_date.status_code == 422
    bad_draw = client.post(f'/documents/{doc["id"]}/annotations', headers=auth,
        json=region(kind='draw', body='{"v":1,"points":[[0,0],[4,1]]}'))
    assert bad_draw.status_code == 422
    drawing = next(item for item in entries if item['kind'] == 'draw')
    invalid_path_update = client.patch(f'/annotations/{drawing["id"]}', headers=auth, json={
        'expected_revision': doc['revision'], 'rect_x': .18, 'rect_y': .22,
        'rect_width': .24, 'rect_height': .08, 'body': '[]', 'color': 'blue'})
    assert invalid_path_update.status_code == 422
    job = wait_for_job(client, auth, client.post(
        f'/matters/{matter["id"]}/build', headers=auth).json()['id'])
    assert job['status'] == 'completed', job['error']
    folder = store.exports_root(matter['id']) / job['result']['id']
    report = json.loads((folder / 'validation-report.json').read_text())
    assert report['page_annotation_summary']['private_review_annotations_omitted'] == len(kinds)
    with pymupdf.open(folder / 'combined.pdf') as pdf:
        assert len(list(pdf[1].annots() or ())) == 0
    assert store.sha256(store.original_path(doc)) == original_hash
    package = client.get(f'/matters/{matter["id"]}/portable', headers=auth)
    restored = client.post('/matters/restore', headers=auth,
        files={'file': ('shapes.cbmatter', package.content, 'application/zip')})
    assert restored.status_code == 201, restored.text
    saved = client.get(f'/matters/{restored.json()["id"]}/annotations', headers=auth).json()
    assert {item['kind'] for item in saved} == set(kinds)


def test_missing_image_overlay_asset_is_a_preflight_error(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, name='missing-asset.pdf', pages=1)
    created = client.post(f'/documents/{doc["id"]}/annotations/image', headers=auth,
        data={key: str(value) for key, value in region().items()},
        files={'file': ('mark.png', image_bytes(), 'image/png')})
    assert created.status_code == 201, created.text
    item = created.json()[0]
    asset = client.app.state.store.matter_dir(matter['id']) / item['asset_relpath']
    asset.unlink()

    checks = client.get(f'/matters/{matter["id"]}/validate', headers=auth).json()
    assert any(check['code'] == 'BAD_IMAGE_OVERLAY_ASSET' and
               check['severity'] == 'error' for check in checks)
    job = wait_for_job(client, auth,
        client.post(f'/matters/{matter["id"]}/build', headers=auth).json()['id'])
    assert job['status'] == 'failed'
    assert 'Preflight failed' in job['error']
