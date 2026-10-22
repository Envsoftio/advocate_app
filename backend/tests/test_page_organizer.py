"""Synthetic organizer operations are atomic, revision-checked and undoable."""
import pymupdf
import pytest

from app.pdf_ops import ProcessingError


def upload(client, auth, matter_id, content):
    result = client.post(f'/matters/{matter_id}/documents', headers=auth,
                         files=[('files', ('organizer-synthetic.pdf', content, 'application/pdf'))])
    assert result.status_code == 201, result.text
    return result.json()[0]


def pdf_state(client, auth, document_id):
    result = client.get(f'/documents/{document_id}/pdf', headers=auth)
    assert result.status_code == 200
    with pymupdf.open(stream=result.content, filetype='pdf') as pdf:
        return [(page.get_text().strip(), page.rotation) for page in pdf]


def test_batch_rotation_and_deletion_each_have_one_history_entry(client, auth, matter, make_pdf):
    content = make_pdf(5, 'ORGANIZER')
    document = upload(client, auth, matter['id'], content)
    store = client.app.state.store
    original = store.original_path(document)
    initial_state = pdf_state(client, auth, document['id'])
    initial_working = store.working_path(document)
    initial_working_bytes = initial_working.read_bytes()
    history_path = f'/matters/{matter["id"]}/history'
    history = client.get(history_path, headers=auth).json()['undo_count']
    path = f'/documents/{document["id"]}/pages/batch'

    rotated = client.post(path, headers=auth, json={
        'operation': 'rotate', 'pages': [3, 0, 2], 'degrees': 90, 'expected_revision': 1})
    assert rotated.status_code == 200, rotated.text
    assert rotated.json()['revision'] == 2
    assert rotated.json()['page_count'] == 5
    assert [rotation for _, rotation in pdf_state(client, auth, document['id'])] == [90, 0, 90, 90, 0]
    assert client.get(history_path, headers=auth).json()['undo_count'] == history + 1

    assert client.post(history_path + '/undo', headers=auth).status_code == 200
    assert pdf_state(client, auth, document['id']) == initial_state
    assert client.post(history_path + '/redo', headers=auth).status_code == 200
    before_deletion = pdf_state(client, auth, document['id'])
    revision = store.get_document(document['id'])['revision']
    before_delete_history = client.get(history_path, headers=auth).json()['undo_count']
    deleted = client.post(path, headers=auth, json={
        'operation': 'delete', 'pages': [3, 1], 'expected_revision': revision})
    assert deleted.status_code == 200, deleted.text
    assert deleted.json()['revision'] == revision + 1
    assert deleted.json()['page_count'] == 3
    assert pdf_state(client, auth, document['id']) == [before_deletion[index] for index in [0, 2, 4]]
    assert client.get(history_path, headers=auth).json()['undo_count'] == before_delete_history + 1
    assert client.post(history_path + '/undo', headers=auth).status_code == 200
    assert pdf_state(client, auth, document['id']) == before_deletion
    assert original.read_bytes() == content
    assert initial_working.read_bytes() == initial_working_bytes


@pytest.mark.parametrize('payload', [
    {'operation': 'delete', 'pages': []},
    {'operation': 'delete', 'pages': [0, 0]},
    {'operation': 'delete', 'pages': [-1]},
    {'operation': 'delete', 'pages': [0, 3]},
    {'operation': 'delete', 'pages': [0, 1, 2]},
    {'operation': 'delete', 'pages': [True]},
    {'operation': 'delete', 'pages': ['1']},
    {'operation': 'delete', 'pages': [1.0]},
    {'operation': 'delete', 'pages': [0], 'degrees': 90},
    {'operation': 'rotate', 'pages': [0]},
    {'operation': 'rotate', 'pages': [0, 2], 'degrees': 45},
    {'operation': 'rotate', 'pages': [0, 3], 'degrees': 90},
    {'operation': 'rotate', 'pages': [0], 'degrees': 90, 'expected_revision': True},
    {'operation': 'delete', 'pages': [0], 'expected_revision': None},
])
def test_invalid_batch_is_rejected_without_working_file_or_history_change(
        client, auth, matter, make_pdf, payload):
    document = upload(client, auth, matter['id'], make_pdf(3))
    store = client.app.state.store
    working = store.working_path(document)
    files = set(working.parent.iterdir())
    working_bytes = working.read_bytes()
    history = store.history_state(matter['id'])
    result = client.post(f'/documents/{document["id"]}/pages/batch', headers=auth,
                         json={'expected_revision': 1, **payload})
    assert result.status_code == 422, result.text
    assert store.get_document(document['id'])['revision'] == 1
    assert store.history_state(matter['id']) == history
    assert working.read_bytes() == working_bytes
    assert set(working.parent.iterdir()) == files


def test_batch_revision_is_required_and_stale_requests_cannot_edit(client, auth, matter, make_pdf):
    document = upload(client, auth, matter['id'], make_pdf(3))
    path = f'/documents/{document["id"]}/pages'
    payload = {'operation': 'rotate', 'pages': [0, 2], 'degrees': 270}
    assert client.post(path + '/batch', headers=auth, json=payload).status_code == 422
    changed = client.post(path + '/batch', headers=auth, json={**payload, 'expected_revision': 1})
    assert changed.status_code == 200, changed.text
    store = client.app.state.store
    state = pdf_state(client, auth, document['id'])
    history = store.history_state(matter['id'])
    for operation, body in [
        ('batch', {**payload, 'expected_revision': 1}),
        ('batch', {'operation': 'delete', 'pages': [1], 'expected_revision': 1}),
        ('reorder', {'order': [2, 1, 0], 'expected_revision': 1}),
    ]:
        stale = client.post(path + '/' + operation, headers=auth, json=body)
        assert stale.status_code == 409, stale.text
        assert 'revision changed' in stale.json()['detail']
    assert pdf_state(client, auth, document['id']) == state
    assert store.get_document(document['id'])['revision'] == 2
    assert store.history_state(matter['id']) == history
    reordered = client.post(path + '/reorder', headers=auth, json={
        'order': [2, 1, 0], 'expected_revision': 2})
    assert reordered.status_code == 200, reordered.text
    assert pdf_state(client, auth, document['id']) == list(reversed(state))


@pytest.mark.parametrize('operation,payload', [
    ('rotate', {'page_index': 0, 'degrees': 90}),
    ('delete', {'page_index': 0}),
    ('crop', {'pages': [0, 1], 'left': 0.1, 'top': 0.1, 'right': 0.9, 'bottom': 0.9}),
])
def test_single_page_tools_honor_optional_revision(client, auth, matter, make_pdf, operation, payload):
    document = upload(client, auth, matter['id'], make_pdf(3))
    path = f'/documents/{document["id"]}/pages/{operation}'
    applied = client.post(path, headers=auth, json={**payload, 'expected_revision': 1})
    assert applied.status_code == 200, applied.text
    assert applied.json()['revision'] == 2
    store = client.app.state.store
    current = store.get_document(document['id'])
    working_bytes = store.working_path(current).read_bytes()
    history = store.history_state(matter['id'])
    stale = client.post(path, headers=auth, json={**payload, 'expected_revision': 1})
    assert stale.status_code == 409, stale.text
    assert 'revision changed' in stale.json()['detail']
    assert store.get_document(document['id'])['revision'] == 2
    assert store.working_path(current).read_bytes() == working_bytes
    assert store.history_state(matter['id']) == history


@pytest.mark.parametrize('operation', ['rotate', 'delete', 'reorder'])
def test_organizer_requires_explicit_signature_acknowledgement(client, auth, matter, make_pdf, operation):
    content = make_pdf(3, 'SYNTHETIC VISIBLE SIGNATURE REVIEW')
    document = upload(client, auth, matter['id'], content)
    marked = client.patch(f'/documents/{document["id"]}', headers=auth,
                          json={'visible_signature_status': 'present'})
    assert marked.status_code == 200
    revision = marked.json()['revision']
    endpoint = 'reorder' if operation == 'reorder' else 'batch'
    payload = ({'order': [2, 1, 0]} if operation == 'reorder' else
               {'operation': operation, 'pages': [0, 2], **({'degrees': 180} if operation == 'rotate' else {})})
    path = f'/documents/{document["id"]}/pages/{endpoint}'
    store = client.app.state.store
    history = store.history_state(matter['id'])
    for acknowledgement in [False, None, 'true', 1]:
        denied = client.post(path, headers=auth, json={**payload, 'expected_revision': revision,
            'acknowledge_signature_risk': acknowledgement})
        assert denied.status_code == 409, denied.text
        assert 'Signature-risk acknowledgment required' in denied.json()['detail']
    assert store.history_state(matter['id']) == history
    assert store.get_document(document['id'])['revision'] == revision
    allowed = client.post(path, headers=auth, json={**payload, 'expected_revision': revision,
        'acknowledge_signature_risk': True})
    assert allowed.status_code == 200, allowed.text
    assert allowed.json()['revision'] == revision + 1
    assert store.original_path(document).read_bytes() == content


def test_failed_batch_save_discards_candidate_and_keeps_entire_selection_unchanged(
        client, auth, matter, make_pdf, monkeypatch):
    document = upload(client, auth, matter['id'], make_pdf(3))
    store = client.app.state.store
    original_state = pdf_state(client, auth, document['id'])
    working = store.working_path(document)
    files = set(working.parent.iterdir())
    history = store.history_state(matter['id'])

    def fail_save(*_args, **_kwargs):
        raise ProcessingError('Synthetic save failure after page changes')

    monkeypatch.setattr('app.pdf_ops.replace_pdf_atomically', fail_save)
    result = client.post(f'/documents/{document["id"]}/pages/batch', headers=auth, json={
        'operation': 'rotate', 'pages': [0, 1, 2], 'degrees': 90, 'expected_revision': 1})
    assert result.status_code == 422, result.text
    assert pdf_state(client, auth, document['id']) == original_state
    assert store.get_document(document['id'])['revision'] == 1
    assert store.history_state(matter['id']) == history
    assert set(working.parent.iterdir()) == files


@pytest.mark.parametrize('operation,body', [
    ('pages/insert', {'source_pages': [1], 'before_index': 1}),
    ('pages/replace', {'source_pages': [1], 'target_pages': [0]}),
    ('replace', {}),
])
@pytest.mark.parametrize('changed_document', ['source', 'target'])
def test_insertion_preview_rejects_changed_source_or_target(client, auth, matter, make_pdf, operation, body, changed_document):
    target = upload(client, auth, matter['id'], make_pdf(3, 'TARGET'))
    source = upload(client, auth, matter['id'], make_pdf(2, 'SOURCE'))
    changed = source if changed_document == 'source' else target
    assert client.post(f'/documents/{changed["id"]}/pages/rotate', headers=auth,
                       json={'page_index': 0, 'degrees': 90}).status_code == 200
    store = client.app.state.store
    before = pdf_state(client, auth, target['id'])
    history = store.history_state(matter['id'])
    response = client.post(f'/documents/{target["id"]}/{operation}', headers=auth, json={
        **body, 'source_document_id': source['id'], 'expected_revision': 1, 'expected_source_revision': 1})
    assert response.status_code == 409, response.text
    assert pdf_state(client, auth, target['id']) == before
    assert store.history_state(matter['id']) == history
    response = client.post(f'/documents/{target["id"]}/{operation}', headers=auth, json={
        **body, 'source_document_id': source['id'],
        'expected_revision': 2 if changed_document == 'target' else 1,
        'expected_source_revision': 2 if changed_document == 'source' else 1})
    assert response.status_code == 200, response.text
    assert store.history_state(matter['id'])['undo_count'] == history['undo_count'] + 1
