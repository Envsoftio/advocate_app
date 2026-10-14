"""Synthetic failure injection and PDF-content regression coverage."""
import hashlib
import errno
import random
import threading
import time

import pymupdf
import pytest

from app import pdf_ops
from app.engine import validate_navigation, volume_plan, index_pages_for
from app.jobs import JobManager
from test_api import upload, wait_for_job


def navigation_pdf(path):
    with pymupdf.open() as pdf:
        for number in range(3):
            page = pdf.new_page()
            page.insert_text((70, 90), f'PAGE {number + 1}')
        pdf[0].insert_link({'kind': pymupdf.LINK_GOTO, 'from': pymupdf.Rect(60, 70, 160, 100),
                            'page': 2, 'to': pymupdf.Point(0, 0)})
        pdf[0].insert_link({'kind': pymupdf.LINK_URI, 'from': pymupdf.Rect(60, 110, 160, 140),
                            'uri': 'https://example.com'})
        pdf[1].add_text_annot((100, 140), 'Retained note')
        pdf.set_toc([[1, 'पहला दस्तावेज़', 1], [2, 'Last page', 3]])
        pdf.set_metadata({'title': 'Synthetic navigation'})
        pdf.save(path)


def test_reorder_extract_and_split_remap_navigation(tmp_path):
    path = tmp_path / 'source.pdf'
    navigation_pdf(path)
    original = path.read_bytes()
    pdf_ops.reorder_pages(path, [2, 1, 0])
    with pymupdf.open(path) as pdf:
        assert pdf[2].get_links()[0]['page'] == 0
        assert pdf[2].get_links()[1]['uri'] == 'https://example.com'
        assert pdf.get_toc() == [[1, 'पहला दस्तावेज़', 3], [2, 'Last page', 1]]
        assert len(list(pdf[1].annots())) == 1
        assert pdf.metadata['title'] == 'Synthetic navigation'
    path.write_bytes(original)
    with pymupdf.open(stream=pdf_ops.extract_pages(path, [2, 0, 2]), filetype='pdf') as pdf:
        assert pdf[1].get_links()[0]['page'] == 0
        assert pdf.get_toc()[1][2] == 1
    with pymupdf.open(stream=pdf_ops.extract_pages(path, [0]), filetype='pdf') as pdf:
        assert [link['kind'] for link in pdf[0].get_links()] == [pymupdf.LINK_URI]
    assert path.read_bytes() == original


def test_reordered_rotated_links_keep_click_area_and_destination_point(tmp_path):
    path = tmp_path / 'rotated-links.pdf'
    with pymupdf.open() as pdf:
        pdf.new_page()
        pdf.new_page()
        pdf[0].insert_link({'kind': pymupdf.LINK_GOTO, 'from': pymupdf.Rect(60, 70, 160, 100),
                           'page': 1, 'to': pymupdf.Point(40, 50)})
        pdf[0].set_rotation(90)
        pdf[1].set_rotation(180)
        pdf.save(path)
    with pymupdf.open(path) as pdf:
        before = pdf[0].get_links()[0]
    pdf_ops.reorder_pages(path, [1, 0])
    with pymupdf.open(path) as pdf:
        after = pdf[1].get_links()[0]
        assert after['page'] == 0
        assert after['from'] == before['from']
        assert after['to'] == before['to']


@pytest.mark.parametrize('failure', ['save', 'validate', 'sync', 'replace'])
def test_atomic_write_failure_keeps_previous_bytes(tmp_path, monkeypatch, failure):
    path = tmp_path / 'source.pdf'
    navigation_pdf(path)
    original = path.read_bytes()
    def fail(*args, **kwargs):
        raise OSError('Synthetic disk failure')
    target, name = {'save': (pymupdf.Document, 'save'),
                    'validate': (pdf_ops, 'validate_pdf'),
                    'sync': (pdf_ops.os, 'fsync'),
                    'replace': (pdf_ops.os, 'replace')}[failure]
    monkeypatch.setattr(target, name, fail)
    with pytest.raises(OSError, match='Synthetic disk failure'):
        pdf_ops.rotate_page(path, 0, 90)
    assert path.read_bytes() == original
    assert not list(tmp_path.glob('.pdf-edit-*'))


def test_partial_revision_copy_is_removed(client, auth, matter, make_pdf, monkeypatch):
    import app.main as main
    doc = upload(client, auth, matter, make_pdf)
    store = client.app.state.store
    directory = store.working_path(doc).parent
    before = set(directory.iterdir())
    def partial_copy(source, target):
        target.write_bytes(b'partial copy')
        raise OSError(errno.ENOSPC, 'Synthetic disk full')
    monkeypatch.setattr(main.shutil, 'copy2', partial_copy)
    response = client.post(f'/documents/{doc["id"]}/pages/rotate', headers=auth,
                           json={'page_index': 0, 'degrees': 90})
    assert response.status_code == 507
    assert 'disk space' in response.json()['detail']
    assert set(directory.iterdir()) == before
    assert store.get_document(doc['id'])['revision'] == 1


def test_exception_after_database_commit_cannot_delete_revision(client, auth, matter, make_pdf, monkeypatch):
    doc = upload(client, auth, matter, make_pdf)
    store = client.app.state.store
    original_update = store.update_document
    def commit_then_fail(*args, **kwargs):
        original_update(*args, **kwargs)
        raise OSError(errno.ENOSPC, 'Synthetic failure after commit')
    monkeypatch.setattr(store, 'update_document', commit_then_fail)
    response = client.post(f'/documents/{doc["id"]}/pages/rotate', headers=auth,
                           json={'page_index': 0, 'degrees': 90})
    assert response.status_code == 507
    current = store.get_document(doc['id'])
    assert current['revision'] == 2
    with pymupdf.open(store.working_path(current)) as pdf:
        assert pdf[0].rotation == 90
    assert store.working_path(doc).is_file()


def test_text_replacement_does_not_apply_unrelated_redactions(tmp_path):
    path = tmp_path / 'redactions.pdf'
    with pymupdf.open() as pdf:
        page = pdf.new_page()
        page.insert_text((70, 90), 'Editable title')
        page.insert_text((70, 150), 'Unrelated evidence')
        page.add_redact_annot(pymupdf.Rect(60, 130, 250, 160))
        pdf.save(path)
    original = path.read_bytes()
    with pytest.raises(pdf_ops.ProcessingError, match='pending redaction'):
        pdf_ops.replace_text_run(path, 0, 0, 'Editable title', 'New title')
    assert path.read_bytes() == original


@pytest.mark.parametrize('rotation', [0, 90, 180, 270])
def test_mixed_ocr_preserves_visible_content_geometry_and_navigation(tmp_path, rotation):
    path = tmp_path / 'mixed.pdf'
    with pymupdf.open() as scan:
        page = scan.new_page(width=500, height=140)
        page.insert_text((30, 80), 'SCANNED BODY EVIDENCE 2026', fontsize=22)
        pixels = page.get_pixmap(dpi=240)
    with pymupdf.open() as pdf:
        page = pdf.new_page()
        page.insert_text((60, 60), 'Native header')
        page.insert_image(pymupdf.Rect(40, 150, 540, 290), pixmap=pixels)
        page.insert_link({'kind': pymupdf.LINK_URI, 'from': pymupdf.Rect(40, 30, 180, 70),
                          'uri': 'https://example.com'})
        page.add_text_annot((200, 400), 'Review note')
        page.set_rotation(rotation)
        pdf.set_toc([[1, 'Mixed evidence', 1]])
        pdf.save(path)
    with pymupdf.open(path) as pdf:
        before = pdf[0].get_pixmap().samples
        dimensions = pdf[0].rect
    assert pdf_ops.ocr_pdf(path) == (1, 1)
    with pymupdf.open(path) as pdf:
        assert 'SCANNED BODY EVIDENCE' in pdf[0].get_text()
        assert pdf[0].get_text().count('Native header') == 1
        assert pdf[0].get_pixmap().samples == before
        assert pdf[0].rotation == rotation and pdf[0].rect == dimensions
        assert len(pdf[0].get_links()) == 1 and len(list(pdf[0].annots())) == 1
        assert pdf.get_toc() == [[1, 'Mixed evidence', 1]]
    digest = hashlib.sha256(path.read_bytes()).digest()
    assert pdf_ops.ocr_pdf(path) == (1, 0)
    assert hashlib.sha256(path.read_bytes()).digest() == digest


def test_cancelled_ocr_and_oversized_raster_leave_source_unchanged(tmp_path):
    path = tmp_path / 'huge.pdf'
    with pymupdf.open() as pdf:
        pdf.new_page(width=10000, height=10000)
        pdf.save(path)
    before = path.read_bytes()
    with pytest.raises(pdf_ops.ProcessingError, match='cancelled'):
        pdf_ops.ocr_pdf(path, cancelled=lambda: True)
    with pytest.raises(pdf_ops.ProcessingError, match='megapixel'):
        pdf_ops.ocr_pdf(path)
    assert path.read_bytes() == before


def test_late_cancel_does_not_hide_a_committed_job():
    jobs = JobManager()
    committed = threading.Event()
    release = threading.Event()
    def task(*args):
        committed.set()
        assert release.wait(5)
        return {'published': True}
    try:
        job = jobs.submit('synthetic', 'build', task)
        assert committed.wait(5)
        jobs.cancel(job['id'])
        release.set()
        deadline = time.monotonic() + 5
        while jobs.get(job['id'])['status'] == 'running' and time.monotonic() < deadline:
            time.sleep(.01)
        result = jobs.get(job['id'])
        assert result['status'] == 'completed'
        assert result['result'] == {'published': True}
    finally:
        release.set()
        jobs.shutdown()


def test_wrong_link_destination_rejected_even_when_count_matches(tmp_path):
    path = tmp_path / 'wrong.pdf'
    navigation_pdf(path)
    with pymupdf.open(path) as pdf:
        toc = pdf.get_toc()
    with pytest.raises(pdf_ops.ProcessingError, match='destinations differ'):
        validate_navigation(path, 3, toc, {0: [1]})


@pytest.mark.parametrize('count,cap', [(23, 24), (24, 25), (24, 26), (46, 48), (47, 49)])
def test_index_growth_never_overflows_volume(count, cap):
    docs = [dict(id=str(i), title=str(i), page_count=1, group_key='') for i in range(count)]
    plans = volume_plan(docs, cap, True)
    assert [doc['id'] for plan in plans for doc in plan] == [doc['id'] for doc in docs]
    assert all(len(plan) + index_pages_for(len(plan)) <= cap for plan in plans)


def test_compression_keeps_text_navigation_and_is_undoable(client, auth, matter):
    with pymupdf.open() as pdf:
        page = pdf.new_page()
        page.insert_text((70, 90), 'Searchable evidence')
        pix = pymupdf.Pixmap(pymupdf.csRGB, 1200, 1200, random.Random(7).randbytes(1200 * 1200 * 3), False)
        page.insert_image(pymupdf.Rect(70, 150, 370, 450), pixmap=pix)
        page.insert_link({'kind': pymupdf.LINK_GOTO, 'from': pymupdf.Rect(60, 70, 250, 100),
                         'page': 0, 'to': pymupdf.Point(0, 0)})
        pdf.set_toc([[1, 'Evidence', 1]])
        blob = pdf.tobytes()
    response = client.post(f'/matters/{matter["id"]}/documents', headers=auth,
                           files=[('files', ('compression.pdf', blob, 'application/pdf'))])
    doc = response.json()[0]
    store = client.app.state.store
    before = store.working_path(doc).read_bytes()
    job = client.post(f'/documents/{doc["id"]}/compress', headers=auth,
                      json={'preset': 'small', 'expected_revision': 1}).json()
    done = wait_for_job(client, auth, job['id'])
    assert done['status'] == 'completed', done
    assert done['result']['after_bytes'] < done['result']['before_bytes']
    current = store.get_document(doc['id'])
    assert current['revision'] == 2
    with pymupdf.open(store.working_path(current)) as pdf:
        assert 'Searchable evidence' in pdf[0].get_text()
        assert pdf.get_toc() == [[1, 'Evidence', 1]]
        assert pdf[0].get_links()[0]['page'] == 0
        assert pdf[0].get_images()[0][2] < 1200
    assert store.original_path(doc).read_bytes() == blob
    client.post(f'/matters/{matter["id"]}/history/undo', headers=auth)
    assert store.working_path(store.get_document(doc['id'])).read_bytes() == before


def test_compression_cancellation_does_not_write(tmp_path):
    path = tmp_path / 'source.pdf'
    navigation_pdf(path)
    before = path.read_bytes()
    with pytest.raises(pdf_ops.ProcessingError, match='cancelled'):
        pdf_ops.compress_pdf(path, 'small', cancelled=lambda: True)
    assert path.read_bytes() == before
    assert not list(tmp_path.glob('.pdf-edit-*'))


@pytest.mark.parametrize('rotation', [90, 180, 270])
def test_overlay_on_rotated_pages_uses_display_coordinates(tmp_path, rotation):
    path = tmp_path / 'rotated.pdf'
    with pymupdf.open() as pdf:
        page = pdf.new_page()
        page.set_rotation(rotation)
        pdf.save(path)
    pdf_ops.add_text_overlay(path, 0, .2, .3, 'REVIEW COPY', 12)
    with pymupdf.open(path) as pdf:
        page = pdf[0]
        box = page.search_for('REVIEW COPY')[0] * page.rotation_matrix
        assert abs(box.x0 / page.rect.width - .2) < .01
        assert abs(box.y0 / page.rect.height - .3) < .01
        assert page.rotation == rotation


@pytest.mark.parametrize('corrupt', [False, True])
def test_restart_recovers_only_verified_committed_export(tmp_path, make_pdf, corrupt):
    from fastapi.testclient import TestClient
    from app.main import create_app
    root = tmp_path / 'store'
    headers = {'Authorization': 'Bearer recovery-test'}
    with TestClient(create_app(root, 'recovery-test')) as client:
        matter = client.post('/matters', headers=headers, json={'name': 'Recovery'}).json()
        upload(client, headers, matter, make_pdf)
        started = client.post(f'/matters/{matter["id"]}/build', headers=headers).json()
        job = wait_for_job(client, headers, started['id'])
        assert job['status'] == 'completed'
        store = client.app.state.store
        job.update(status='running', result=None, progress=95)
        store.save_job(job)  # Simulate death between atomic rename and terminal DB update.
        if corrupt:
            path = store.exports_root(matter['id']) / job['id'] / 'combined.pdf'
            data = path.read_bytes()
            path.write_bytes(data[:-1] + bytes([data[-1] ^ 1]))
    with TestClient(create_app(root, 'recovery-test')) as client:
        recovered = client.get(f'/jobs/{job["id"]}', headers=headers).json()
        assert recovered['status'] == ('interrupted' if corrupt else 'completed')
        response = client.get(f'/matters/{matter["id"]}/exports/{job["id"]}/combined.pdf', headers=headers)
        assert response.status_code == (404 if corrupt else 200)


def test_unicode_bookmarks_are_not_truncated(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, pages=1)
    title = 'दस्तावेज़ ' + 'A' * 150
    base = f'/matters/{matter["id"]}'
    response = client.put(base + '/bookmarks', headers=auth, json={'entries': [{
        'id': 'a' * 32, 'title': title, 'document_id': doc['id'],
        'page_index': 0, 'target_revision': 1, 'parent_id': None}]})
    assert response.status_code == 200
    job = wait_for_job(client, auth, client.post(base + '/build', headers=auth).json()['id'])
    assert job['status'] == 'completed', job
    path = client.app.state.store.exports_root(matter['id']) / job['id'] / 'combined.pdf'
    with pymupdf.open(path) as pdf:
        assert title in [entry[1] for entry in pdf.get_toc()]


def test_scattered_bookmark_children_keep_their_intended_parent(client, auth, matter, make_pdf):
    doc = upload(client, auth, matter, make_pdf, pages=1)
    entries = [{'id': letter * 32, 'title': letter, 'document_id': doc['id'],
                'page_index': 0, 'target_revision': 1, 'parent_id': parent}
               for letter, parent in [('a', None), ('b', None), ('c', 'a' * 32)]]
    base = f'/matters/{matter["id"]}'
    response = client.put(base + '/bookmarks', headers=auth, json={'entries': entries})
    assert response.status_code == 200
    assert [entry['title'] for entry in response.json()] == ['a', 'c', 'b']
    job = wait_for_job(client, auth, client.post(base + '/build', headers=auth).json()['id'])
    assert job['status'] == 'completed', job
    path = client.app.state.store.exports_root(matter['id']) / job['id'] / 'combined.pdf'
    with pymupdf.open(path) as pdf:
        assert [entry[:2] for entry in pdf.get_toc() if entry[1] in ('a', 'b', 'c')] == [
            [2, 'a'], [3, 'c'], [2, 'b']]
