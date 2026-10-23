import csv
import io
import zipfile
import pymupdf

from app.engine import index_pages_for
from test_api import upload, wait_for_job


def test_readable_index_growth_and_destinations_share_page_map(client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}'
    ids = []
    for number in range(17):
        doc = upload(client, auth, matter, make_pdf, name=f'doc-{number}.pdf', pages=1)
        ids.append(doc['id'])
    marks = client.get(base + '/output-marks', headers=auth).json()
    marks['index_style'] = 'readable'
    assert client.put(base + '/output-marks', headers=auth, json=marks).status_code == 200
    assert index_pages_for(16, marks) == 1
    assert index_pages_for(17, marks) == 2
    preview = client.post(base + '/bundle-preview', headers=auth,
                          json={'max_pages_per_volume': 300,
                                'keep_groups_intact': True}).json()
    volume = preview['volumes'][0]
    assert volume['index_pages'] == 2 and volume['page_count'] == 19
    assert volume['rows'][0]['start_page'] == 3
    assert volume['rows'][-1]['start_page'] == 19
    started = client.post(base + '/build', headers=auth)
    assert wait_for_job(client, auth, started.json()['id'])['status'] == 'completed'
    export = client.get(base + '/exports', headers=auth).json()[0]
    store = client.app.state.store
    pdf_path = store.get_export_file(matter['id'], export['id'], 'combined.pdf')
    with pymupdf.open(pdf_path) as pdf:
        assert pdf.page_count == 19
        assert [link['page'] for link in pdf[0].get_links()] == list(range(2, 18))
        assert [link['page'] for link in pdf[1].get_links()] == [18]
        assert pdf.get_toc()[-1][2] == 19
    csv_path = store.get_export_file(matter['id'], export['id'], 'index.csv')
    rows = list(csv.DictReader(io.StringIO(csv_path.read_bytes().decode('utf-8-sig'))))
    assert rows[0]['start_page'] == '3'
    assert rows[-1]['start_page'] == '19'
    assert [item['id'] for item in export['page_map_snapshot']] == ids


def test_unicode_index_title_is_flagged_for_visual_review(client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}'
    doc = upload(client, auth, matter, make_pdf, pages=1)
    assert client.patch(f'/documents/{doc["id"]}', headers=auth,
                        json={'title': 'प्रार्थना पत्र — दिल्ली'}).status_code == 200
    marks = client.get(base + '/output-marks', headers=auth).json()
    marks['index_style'] = 'readable'
    assert client.put(base + '/output-marks', headers=auth, json=marks).status_code == 200
    started = client.post(base + '/build', headers=auth)
    assert wait_for_job(client, auth, started.json()['id'])['status'] == 'completed'
    export = client.get(base + '/exports', headers=auth).json()[0]
    assert any(check['code'] == 'NON_LATIN_INDEX' for check in export['checks'])
    assert export['page_map_snapshot'][0]['title'] == 'प्रार्थना पत्र — दिल्ली'
    with pymupdf.open(client.app.state.store.get_export_file(matter['id'], export['id'], 'combined.pdf')) as pdf:
        assert pdf.get_toc()[-1][1].endswith('प्रार्थना पत्र — दिल्ली')
        assert pdf[0].get_links()[0]['page'] == 1


def test_optional_part_extracts_use_source_ranges_across_volumes(client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}'
    first = upload(client, auth, matter, lambda pages: make_pdf(pages, 'Part one'),
                   name='part-one.pdf', pages=2)
    second = upload(client, auth, matter, lambda pages: make_pdf(pages, 'Part three'),
                    name='part-three.pdf', pages=1)
    assert client.patch(f'/documents/{second["id"]}', headers=auth,
                        json={'part': 3, 'kind': 'annexure', 'label': 'A-1',
                              'label_status': 'confirmed', 'label_source': 'user'}).status_code == 200
    assert client.patch(base, headers=auth, json={'max_pages_per_volume': 3}).status_code == 200
    marks = client.get(base + '/output-marks', headers=auth).json()
    marks.update(part_extracts_enabled=True, annexure_mode='separator')
    assert client.put(base + '/output-marks', headers=auth, json=marks).status_code == 200
    # Separator plus source page needs index + separator + source = 3 pages.
    preview = client.post(base + '/bundle-preview', headers=auth,
                          json={'max_pages_per_volume': 3,
                                'keep_groups_intact': True}).json()
    assert len(preview['volumes']) == 2 and preview['combined_pages'] == 6
    original = client.app.state.store.original_path(first).read_bytes()
    started = client.post(base + '/build', headers=auth)
    assert wait_for_job(client, auth, started.json()['id'])['status'] == 'completed'
    export = client.get(base + '/exports', headers=auth).json()[0]
    assert [(item['part'], item['pages']) for item in export['part_extracts']] == [(1, 2), (3, 1)]
    store = client.app.state.store
    with pymupdf.open(store.get_export_file(matter['id'], export['id'], 'part-01.pdf')) as pdf:
        assert pdf.page_count == 2 and 'Part one' in pdf[0].get_text()
        assert len(pdf.get_toc()) == 1
    with pymupdf.open(store.get_export_file(matter['id'], export['id'], 'part-03.pdf')) as pdf:
        assert pdf.page_count == 1 and 'Part three' in pdf[0].get_text()
        assert 'TECHNICAL SEPARATOR' not in pdf[0].get_text()
    with zipfile.ZipFile(store.get_export_file(matter['id'], export['id'], 'filing-package.zip')) as archive:
        assert {'part-01.pdf', 'part-03.pdf'} <= set(archive.namelist())
    assert store.original_path(first).read_bytes() == original


def test_unqualified_script_blocks_generated_index_instead_of_losing_glyphs(client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}'
    doc = upload(client, auth, matter, make_pdf, pages=1)
    assert client.patch(f'/documents/{doc["id"]}', headers=auth,
                        json={'title': 'தமிழ் ஆவணம்'}).status_code == 200
    checks = client.get(base + '/validate', headers=auth).json()
    assert any(item['code'] == 'UNSUPPORTED_INDEX_SCRIPT' and item['severity'] == 'error'
               for item in checks)
    started = client.post(base + '/build', headers=auth)
    job = wait_for_job(client, auth, started.json()['id'])
    assert job['status'] == 'failed'
    assert client.get(base + '/exports', headers=auth).json() == []
