import io
import json

import pymupdf

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
        assert list(source_page.annots() or ()) == []
    report = json.loads((folder / 'validation-report.json').read_text())
    assert report['page_annotation_summary'] == {
        'image_overlays_inserted': 1,
        'unresolved_image_overlays': 0,
        'private_review_annotations_omitted': 2,
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
