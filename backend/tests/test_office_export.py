from io import BytesIO
from xml.etree import ElementTree as ET
from zipfile import ZipFile

from test_api import upload, wait_for_job


def test_completed_build_exports_editable_office_review_files(client, auth, matter, make_pdf):
    source = make_pdf(2, text='Synthetic page content')
    document = upload(client, auth, matter, lambda _: source, pages=2)
    started = client.post(f'/matters/{matter["id"]}/build', headers=auth)
    assert started.status_code == 202, started.text
    completed = wait_for_job(client, auth, started.json()['id'])
    assert completed['status'] == 'completed', completed
    export_id = completed['result']['id']
    base = f'/matters/{matter["id"]}/exports/{export_id}'
    original_hash = client.app.state.store.sha256(client.app.state.store.original_path(document))

    word = client.get(base + '/combined.pdf/office/docx', headers=auth)
    assert word.status_code == 200, word.text
    assert word.headers['cache-control'] == 'no-store'
    assert 'combined-review.docx' in word.headers['content-disposition']
    with ZipFile(BytesIO(word.content)) as archive:
        text = ''.join(ET.fromstring(archive.read('word/document.xml')).itertext())
    assert 'Synthetic page content page 1' in text
    assert 'Synthetic page content page 2' in text
    assert 'editable review draft' in text

    sheet = client.get(base + '/index.csv/office/xlsx', headers=auth)
    assert sheet.status_code == 200, sheet.text
    csv_text = client.get(base + '/index.csv', headers=auth).text.lstrip('\ufeff')
    with ZipFile(BytesIO(sheet.content)) as archive:
        cells = ''.join(ET.fromstring(archive.read('xl/worksheets/sheet1.xml')).itertext())
    assert 'start_page' in csv_text and 'start_page' in cells
    assert 'Synthetic page content' not in cells  # This workbook is the shared index, not a PDF transcript.

    slides = client.get(base + '/combined.pdf/office/pptx', headers=auth)
    assert slides.status_code == 200, slides.text
    with ZipFile(BytesIO(slides.content)) as archive:
        names = [name for name in archive.namelist() if name.startswith('ppt/slides/slide') and name.endswith('.xml')]
        slide_text = ' '.join(''.join(ET.fromstring(archive.read(name)).itertext()) for name in names)
    assert len(names) >= 2
    assert 'Synthetic page content page 1' in slide_text
    assert 'Synthetic page content page 2' in slide_text
    assert client.app.state.store.sha256(client.app.state.store.original_path(document)) == original_hash

    assert client.get(base + '/combined.pdf/office/xlsx', headers=auth).status_code == 422
    assert client.get(base + '/index.csv/office/docx', headers=auth).status_code == 422
    assert client.get(base + '/combined.pdf/office/docx').status_code == 401

    printed = client.post(base + '/combined.pdf/print', headers=auth,
                          json={'pages': '2-3', 'orientation': 'landscape', 'scale': 'fit'})
    assert printed.status_code == 200, printed.text
    import pymupdf
    with pymupdf.open(stream=printed.content, filetype='pdf') as copy:
        assert copy.page_count == 2
        assert all(page.rect.width > page.rect.height for page in copy)
        assert 'Synthetic page content page 2' in copy[1].get_text()
    assert client.post(base + '/combined.pdf/print', headers=auth,
                       json={'pages': '2,2', 'orientation': 'source'}).status_code == 422

    working = client.post(f'/documents/{document["id"]}/print', headers=auth,
                          json={'expected_revision': document['revision'], 'pages': '2',
                                'orientation': 'portrait', 'scale': 'fit'})
    assert working.status_code == 200, working.text
    with pymupdf.open(stream=working.content, filetype='pdf') as copy:
        assert copy.page_count == 1
        assert 'Synthetic page content page 2' in copy[0].get_text()
    assert client.post(f'/documents/{document["id"]}/print', headers=auth,
                       json={'expected_revision': 0, 'pages': 'all'}).status_code == 409
    mark = client.post(f'/documents/{document["id"]}/annotations', headers=auth, json={
        'kind': 'box', 'page_index': 0, 'expected_revision': document['revision'],
        'rect_x': .18, 'rect_y': .22, 'rect_width': .24, 'rect_height': .08,
        'body': '', 'color': 'yellow', 'output_scope': 'pdf',
    })
    assert mark.status_code == 201, mark.text
    working_hash = client.app.state.store.sha256(client.app.state.store.working_path(document))
    marked = client.post(f'/documents/{document["id"]}/print', headers=auth,
                         json={'expected_revision': document['revision'], 'pages': '1',
                               'orientation': 'source', 'scale': 'fit'})
    assert marked.status_code == 200, marked.text
    with pymupdf.open(stream=marked.content, filetype='pdf') as copy:
        assert copy[0].get_drawings()
    assert client.app.state.store.sha256(client.app.state.store.working_path(document)) == working_hash
    assert client.app.state.store.sha256(client.app.state.store.original_path(document)) == original_hash


def test_word_draft_refuses_image_only_pdf(tmp_path):
    from app.office_export import word_draft
    import pymupdf
    with pymupdf.open() as pdf:
        pdf.new_page()
        path = tmp_path / 'blank.pdf'
        pdf.save(path)
    try:
        word_draft(path)
    except ValueError as exc:
        assert 'no extractable text' in str(exc)
    else:
        raise AssertionError('Blank PDF should not silently become an editable draft')


def test_print_copy_keeps_annotation_appearance_on_source_and_a4(tmp_path):
    import pymupdf
    from app.print_export import print_copy

    path = tmp_path / 'annotated.pdf'
    with pymupdf.open() as pdf:
        page = pdf.new_page()
        page.add_freetext_annot(pymupdf.Rect(80, 80, 300, 130), 'PRINT THIS NOTE',
                               fontsize=14)
        pdf.save(path)
    before = path.read_bytes()
    for orientation in ('source', 'portrait'):
        content = print_copy(path, 'all', orientation, 'fit')
        with pymupdf.open(stream=content, filetype='pdf') as printed:
            assert 'PRINT THIS NOTE' in printed[0].get_text()
    assert path.read_bytes() == before
