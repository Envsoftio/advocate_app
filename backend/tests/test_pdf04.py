import json

import pymupdf

from test_api import upload, wait_for_job


def reviewed_link(char, title, source, *, kind, target=None, uri=''):
    return {
        'id': char * 32, 'title': title,
        'source_document_id': source['id'], 'source_page_index': 0,
        'source_revision': source['revision'],
        'rect_x': .12, 'rect_y': .18, 'rect_width': .25, 'rect_height': .05,
        'kind': kind,
        'target_document_id': target['id'] if target else None,
        'target_page_index': 0 if target else None,
        'target_revision': target['revision'] if target else None,
        'uri': uri,
    }


def test_reviewed_internal_external_links_resolve_without_retaining_source_actions(
        client, auth, matter, make_pdf):
    first = upload(client, auth, matter, make_pdf, name='source.pdf', pages=2)
    second = upload(client, auth, matter, make_pdf, name='target.pdf', pages=2)
    base = f'/matters/{matter["id"]}'
    assert client.patch(base, headers=auth,
                        json={'max_pages_per_volume': 3}).status_code == 200
    entries = [
        reviewed_link('a', 'Open target', first, kind='internal', target=second),
        reviewed_link('b', 'Official web reference', first, kind='external',
                      uri='https://example.org/reference'),
    ]
    saved = client.put(base + '/links', headers=auth, json={'entries': entries})
    assert saved.status_code == 200, saved.text
    assert all(item['resolved'] for item in saved.json())

    job = wait_for_job(client, auth, client.post(base + '/build', headers=auth).json()['id'])
    assert job['status'] == 'completed', job['error']
    folder = client.app.state.store.exports_root(matter['id']) / job['result']['id']
    with pymupdf.open(folder / 'volume-01.pdf') as pdf:
        links = pdf[1].get_links()
        assert [item['uri'] for item in links if item['kind'] == pymupdf.LINK_URI] == [
            'https://example.org/reference']
        assert not [item for item in links if item['kind'] == pymupdf.LINK_GOTO]
    with pymupdf.open(folder / 'combined.pdf') as pdf:
        links = pdf[1].get_links()
        assert [item['page'] for item in links if item['kind'] == pymupdf.LINK_GOTO] == [4]
        assert [item['uri'] for item in links if item['kind'] == pymupdf.LINK_URI] == [
            'https://example.org/reference']
    report = json.loads((folder / 'validation-report.json').read_text())
    assert report['reviewed_link_summary'] == {
        'combined_retained': 2,
        'standalone_cross_volume_omissions': 1,
        'unresolved': 0,
    }
    assert any(check['code'] == 'CROSS_VOLUME_LINKS_COMBINED_ONLY'
               for check in report['checks'])

    invalid = dict(entries[1], id='c' * 32, uri='file:///private/matter.pdf')
    rejected = client.put(base + '/links', headers=auth,
                          json={'entries': entries + [invalid]})
    assert rejected.status_code == 422
    assert 'https, http or mailto' in rejected.text


def test_reviewed_link_revision_state_undo_and_portable_restore(
        client, auth, matter, make_pdf):
    source = upload(client, auth, matter, make_pdf, name='source.pdf', pages=1)
    target = upload(client, auth, matter, make_pdf, name='target.pdf', pages=1)
    base = f'/matters/{matter["id"]}'
    entry = reviewed_link('d', 'Revision anchored', source, kind='internal', target=target)
    assert client.put(base + '/links', headers=auth,
                      json={'entries': [entry]}).status_code == 200
    assert client.post(base + '/history/undo', headers=auth).status_code == 200
    assert client.get(base + '/links', headers=auth).json() == []
    assert client.post(base + '/history/redo', headers=auth).status_code == 200
    rotated = client.post(f'/documents/{target["id"]}/pages/rotate', headers=auth,
                          json={'page_index': 0, 'degrees': 90})
    assert rotated.status_code == 200
    assert client.get(base + '/links', headers=auth).json()[0]['resolved'] is False
    job = wait_for_job(client, auth, client.post(base + '/build', headers=auth).json()['id'])
    assert job['status'] == 'completed', job['error']
    assert any(check['code'] == 'UNRESOLVED_REVIEWED_LINKS'
               for check in job['result']['checks'])

    backup = client.get(base + '/portable', headers=auth)
    restored = client.post('/matters/restore', headers=auth,
        files={'file': ('matter.cbmatter', backup.content, 'application/zip')})
    assert restored.status_code == 201, restored.text
    restored_links = client.get(
        f'/matters/{restored.json()["id"]}/links', headers=auth).json()
    assert len(restored_links) == 1 and restored_links[0]['resolved'] is False
    assert restored_links[0]['source_document_id'] != source['id']


def test_output_mark_preview_export_freshness_and_immutable_original(
        client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, name='rotated.pdf', pages=1)
    store = client.app.state.store
    original_hash = store.sha256(store.original_path(doc))
    base = f'/matters/{matter["id"]}'
    rotated = client.post(f'/documents/{doc["id"]}/pages/rotate', headers=auth,
                          json={'page_index': 0, 'degrees': 90})
    assert rotated.status_code == 200, rotated.text
    doc = rotated.json()
    first_job = wait_for_job(client, auth, client.post(base + '/build', headers=auth).json()['id'])
    assert first_job['status'] == 'completed', first_job['error']
    old_pdf = (store.exports_root(matter['id']) / first_job['result']['id'] / 'combined.pdf').read_bytes()

    settings = {
        'watermark_enabled': True, 'watermark_text': 'REVIEW COPY',
        'watermark_position': 'center', 'watermark_font_size': 28,
        'watermark_opacity': .18,
        'timestamp_enabled': True, 'timestamp_format': 'datetime',
        'timestamp_timezone': 'Asia/Kolkata', 'timestamp_position': 'bottom_right',
        'timestamp_font_size': 8, 'timestamp_prefix': 'Built',
    }
    preview = client.post(
        f'/documents/{doc["id"]}/pages/0/output-marks-preview',
        headers=auth, json=settings)
    assert preview.status_code == 200, preview.text
    marks = preview.json()['marks']
    assert [item['kind'] for item in marks] == ['watermark', 'timestamp']
    assert marks[0]['text'] == 'REVIEW COPY'
    assert 'IST' in marks[1]['text']
    assert all(0 <= value <= 1 for item in marks for value in item['relative_rect'])
    saved = client.put(base + '/output-marks', headers=auth, json=settings)
    assert saved.status_code == 200, saved.text
    assert client.get(base + '/exports', headers=auth).json()[0]['stale'] is True

    second_job = wait_for_job(client, auth, client.post(base + '/build', headers=auth).json()['id'])
    assert second_job['status'] == 'completed', second_job['error']
    folder = store.exports_root(matter['id']) / second_job['result']['id']
    with pymupdf.open(folder / 'combined.pdf') as pdf:
        text = pdf[1].get_text()
        assert 'REVIEW COPY' in text and 'Built ' in text and 'IST' in text
    report = json.loads((folder / 'validation-report.json').read_text())
    assert report['output_marks'] == saved.json()
    assert report['output_mark_summary']['inserted'] == 2
    assert store.sha256(store.original_path(doc)) == original_hash
    assert (store.exports_root(matter['id']) / first_job['result']['id'] /
            'combined.pdf').read_bytes() == old_pdf

    package = client.get(base + '/portable', headers=auth)
    restored = client.post('/matters/restore', headers=auth,
        files={'file': ('matter.cbmatter', package.content, 'application/zip')})
    assert restored.status_code == 201, restored.text
    assert client.get(f'/matters/{restored.json()["id"]}/output-marks',
                      headers=auth).json() == saved.json()

