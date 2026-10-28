import csv
import json

import pymupdf

from test_api import wait_for_job


def upload_text(client, auth, matter_id, name, text):
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=595, height=842)
        page.insert_text((70, 90), text, fontsize=12)
        content = pdf.tobytes()
    response = client.post(f'/matters/{matter_id}/documents?included=true', headers=auth,
        files=[('files', (name, content, 'application/pdf'))])
    assert response.status_code == 201, response.text
    return response.json()[0]


def test_label_proposals_are_preview_only_group_aware_and_conservative(
        client, auth, matter):
    docs = [upload_text(client, auth, matter['id'], name, 'Synthetic evidence') for name in
            ('Annexure P-7.pdf', 'second.pdf', 'third.pdf')]
    ambiguous = upload_text(client, auth, matter['id'],
                            'Annexure P-7 Exhibit P-8.pdf', 'Ambiguous synthetic evidence')
    grouped = client.patch(f'/matters/{matter["id"]}/documents/batch', headers=auth,
        json={'ids': [docs[0]['id'], docs[1]['id']], 'group_key': 'supporting-set'})
    assert grouped.status_code == 200, grouped.text

    partial = client.post(f'/matters/{matter["id"]}/labels/preview', headers=auth, json={
        'ids': [docs[0]['id']], 'mode': 'grouped_suffix', 'prefix': 'P-', 'start': 1})
    assert partial.status_code == 200, partial.text
    assert any('partly selected' in item for item in partial.json()['blocking'])

    preview = client.post(f'/matters/{matter["id"]}/labels/preview', headers=auth, json={
        'ids': [doc['id'] for doc in docs], 'mode': 'grouped_suffix',
        'prefix': 'P-', 'start': 1})
    assert preview.status_code == 200, preview.text
    body = preview.json()
    assert [row['proposed_label'] for row in body['rows']] == ['P-1a', 'P-1b', 'P-2']
    assert 'does not establish a legal relationship or court approval' in body['warnings'][0]
    assert all(doc['label'] == '' for doc in client.get(
        f'/matters/{matter["id"]}/documents', headers=auth).json())

    configured = client.post(f'/matters/{matter["id"]}/labels/preview', headers=auth, json={
        'ids': [doc['id'] for doc in docs], 'mode': 'grouped_suffix',
        'prefix': 'ANN-', 'start': 4, 'group_separator': '.',
        'group_suffix_style': 'upper_alpha'})
    assert configured.status_code == 200, configured.text
    assert [row['proposed_label'] for row in configured.json()['rows']] == [
        'ANN-4.A', 'ANN-4.B', 'ANN-5']

    parent = client.patch(f'/documents/{docs[1]["id"]}', headers=auth,
                          json={'group_role': 'parent'})
    assert parent.status_code == 200, parent.text
    hierarchy = client.post(f'/matters/{matter["id"]}/labels/preview', headers=auth, json={
        'ids': [doc['id'] for doc in docs], 'mode': 'grouped_suffix',
        'prefix': 'P-', 'start': 1})
    assert hierarchy.status_code == 200, hierarchy.text
    assert [row['proposed_label'] for row in hierarchy.json()['rows']] == [
        'P-1a', 'P-1', 'P-2']
    assert hierarchy.json()['rows'][1]['group_role'] == 'parent'
    duplicate_parent = client.patch(f'/documents/{docs[0]["id"]}', headers=auth,
                                    json={'group_role': 'parent'})
    assert duplicate_parent.status_code == 422
    assert client.patch(f'/documents/{docs[1]["id"]}', headers=auth,
                        json={'group_role': 'member'}).status_code == 200

    first_base = client.post(f'/matters/{matter["id"]}/labels/preview', headers=auth, json={
        'ids': [doc['id'] for doc in docs], 'mode': 'grouped_suffix',
        'prefix': 'P-', 'start': 1, 'group_suffix_style': 'lower_alpha_parenthesized',
        'first_member_base': True})
    assert first_base.status_code == 200, first_base.text
    assert [row['proposed_label'] for row in first_base.json()['rows']] == [
        'P-1', 'P-1(a)', 'P-2']

    filename = client.post(f'/matters/{matter["id"]}/labels/preview', headers=auth, json={
        'ids': [docs[0]['id'], docs[1]['id'], ambiguous['id']], 'mode': 'filename'})
    assert filename.status_code == 200, filename.text
    rows = filename.json()['rows']
    assert rows[0]['proposed_label'] == 'P-7'
    assert rows[1]['proposal_status'] == 'needs_manual'
    assert rows[2]['proposal_status'] == 'needs_manual'
    assert 'Multiple possible labels' in rows[2]['reason']
    assert any('Every selected document' in item for item in filename.json()['blocking'])


def test_archived_parent_cannot_restore_into_group_with_new_parent(client, auth, matter):
    docs = [upload_text(client, auth, matter['id'], name, 'Synthetic evidence')
            for name in ('parent-a.pdf', 'parent-b.pdf')]
    grouped = client.patch(f'/matters/{matter["id"]}/documents/batch', headers=auth,
        json={'ids': [doc['id'] for doc in docs], 'group_key': 'restore-group'})
    assert grouped.status_code == 200, grouped.text
    assert client.patch(f'/documents/{docs[0]["id"]}', headers=auth,
                        json={'group_role': 'parent'}).status_code == 200
    assert client.delete(f'/documents/{docs[0]["id"]}', headers=auth).status_code == 200
    assert client.patch(f'/documents/{docs[1]["id"]}', headers=auth,
                        json={'group_role': 'parent'}).status_code == 200

    restored = client.post(
        f'/matters/{matter["id"]}/archived-documents/{docs[0]["id"]}/restore', headers=auth)
    assert restored.status_code == 422
    assert 'already has a designated parent' in restored.text


def test_citation_review_acknowledgements_apply_and_atomic_undo(
        client, auth, matter):
    reference = upload_text(client, auth, matter['id'], 'petition.pdf',
                            'See Annexure P-1 and Annexure X-9. Human review required.')
    first = upload_text(client, auth, matter['id'], 'first.pdf', 'First synthetic annexure')
    second = upload_text(client, auth, matter['id'], 'second.pdf', 'Second synthetic annexure')
    store = client.app.state.store
    source_hashes = {doc['id']: store.sha256(store.original_path(doc))
                     for doc in (reference, first, second)}
    assert client.patch(f'/documents/{first["id"]}', headers=auth,
                        json={'label': 'P-1'}).status_code == 200

    preview = client.post(f'/matters/{matter["id"]}/labels/preview', headers=auth, json={
        'ids': [first['id'], second['id']], 'mode': 'sequential', 'prefix': 'P-',
        'start': 2, 'reference_document_id': reference['id']})
    assert preview.status_code == 200, preview.text
    body = preview.json()
    assert body['reference_review']['revision'] == reference['revision']
    assert body['reference_review']['searchable_pages'] == 1
    refs = body['reference_review']['references']
    assert [(item['page'], item['label'], item['status']) for item in refs] == [
        (1, 'P-1', 'old_label'), (1, 'X-9', 'unmatched')]
    mappings = [{'document_id': row['document_id'], 'old_label': row['old_label'],
                 'new_label': row['proposed_label']} for row in body['rows']]
    base = {'mappings': mappings, 'reference_document_id': reference['id'],
            'expected_reference_revision': reference['revision']}
    blocked = client.post(f'/matters/{matter["id"]}/labels/apply', headers=auth, json=base)
    assert blocked.status_code == 409
    assert 'Acknowledge' in blocked.json()['detail']

    applied = client.post(f'/matters/{matter["id"]}/labels/apply', headers=auth, json={
        **base, 'acknowledge_existing_labels': True, 'acknowledge_references': True})
    assert applied.status_code == 200, applied.text
    by_id = {doc['id']: doc for doc in applied.json()['documents']}
    assert (by_id[first['id']]['label'], by_id[second['id']]['label']) == ('P-2', 'P-3')
    assert all(by_id[item]['label_status'] == 'confirmed' and
               by_id[item]['label_source'] == 'proposal' for item in (first['id'], second['id']))
    assert client.post(f'/matters/{matter["id"]}/history/undo', headers=auth).status_code == 200
    undone = {doc['id']: doc for doc in client.get(
        f'/matters/{matter["id"]}/documents', headers=auth).json()}
    assert undone[first['id']]['label'] == 'P-1'
    assert undone[first['id']]['label_source'] == 'user'
    assert undone[second['id']]['label'] == ''
    assert {doc_id: store.sha256(store.original_path(undone[doc_id]))
            for doc_id in source_hashes} == source_hashes


def test_renumbering_warns_about_prior_build_and_never_rewrites_it(
        client, auth, matter):
    doc = upload_text(client, auth, matter['id'], 'annexure.pdf', 'Synthetic annexure')
    assert client.patch(f'/documents/{doc["id"]}', headers=auth,
        json={'label': 'P-1'}).status_code == 200
    base = f'/matters/{matter["id"]}'
    first_job = wait_for_job(client, auth, client.post(base + '/build', headers=auth).json()['id'])
    assert first_job['status'] == 'completed', first_job
    first_folder = client.app.state.store.exports_root(matter['id']) / first_job['result']['id']
    prior_csv = (first_folder / 'index.csv').read_bytes()
    prior_manifest = json.loads((first_folder / 'export_manifest.json').read_text())
    assert prior_manifest['label_inputs'][0]['label'] == 'P-1'

    preview = client.post(base + '/labels/preview', headers=auth, json={
        'ids': [doc['id']], 'mode': 'sequential', 'prefix': 'P-', 'start': 2})
    assert preview.status_code == 200, preview.text
    body = preview.json()
    assert body['export_history']['has_prior_exports'] is True
    mapping = [{'document_id': doc['id'], 'old_label': 'P-1', 'new_label': 'P-2'}]
    blocked = client.post(base + '/labels/apply', headers=auth, json={
        'mappings': mapping, 'acknowledge_existing_labels': True})
    assert blocked.status_code == 409
    applied = client.post(base + '/labels/apply', headers=auth, json={
        'mappings': mapping, 'acknowledge_existing_labels': True,
        'acknowledge_prior_exports': True})
    assert applied.status_code == 200, applied.text
    assert (first_folder / 'index.csv').read_bytes() == prior_csv
    assert json.loads((first_folder / 'export_manifest.json').read_text())['label_inputs'][0]['label'] == 'P-1'

    second_job = wait_for_job(client, auth, client.post(base + '/build', headers=auth).json()['id'])
    assert second_job['status'] == 'completed', second_job
    second_folder = client.app.state.store.exports_root(matter['id']) / second_job['result']['id']
    with (second_folder / 'index.csv').open(encoding='utf-8-sig') as stream:
        assert list(csv.DictReader(stream))[0]['label'] == 'P-2'
    with pymupdf.open(second_folder / 'combined.pdf') as pdf:
        assert any(item[1] == 'P-2 - annexure' for item in pdf.get_toc())
    assert json.loads((second_folder / 'export_manifest.json').read_text())['label_inputs'][0]['label'] == 'P-2'


def test_apply_rejects_duplicate_and_stale_label_maps(client, auth, matter):
    first = upload_text(client, auth, matter['id'], 'first.pdf', 'One')
    second = upload_text(client, auth, matter['id'], 'second.pdf', 'Two')
    assert client.patch(f'/documents/{second["id"]}', headers=auth,
                        json={'label': 'P-9'}).status_code == 200
    duplicate = client.post(f'/matters/{matter["id"]}/labels/apply', headers=auth, json={
        'mappings': [{'document_id': first['id'], 'old_label': '', 'new_label': 'p-9'}]})
    assert duplicate.status_code == 422
    stale = client.post(f'/matters/{matter["id"]}/labels/apply', headers=auth, json={
        'mappings': [{'document_id': second['id'], 'old_label': 'P-8', 'new_label': 'P-10'}],
        'acknowledge_existing_labels': True})
    assert stale.status_code == 422


def test_insertion_and_removal_renumber_policy_is_explicit(client, auth, matter):
    docs = [upload_text(client, auth, matter['id'], f'{name}.pdf', name) for name in
            ('first', 'second', 'inserted')]
    for doc, label in zip(docs, ('P-1', 'P-2', 'P-3')):
        assert client.patch(f'/documents/{doc["id"]}', headers=auth,
                            json={'label': label}).status_code == 200
    assert client.delete(f'/documents/{docs[0]["id"]}', headers=auth).status_code == 200
    remaining = docs[1:]

    preserved = client.post(f'/matters/{matter["id"]}/labels/preview', headers=auth, json={
        'ids': [doc['id'] for doc in remaining], 'mode': 'sequential',
        'prefix': 'P-', 'start': 1, 'existing_label_policy': 'preserve'})
    assert preserved.status_code == 200, preserved.text
    assert [row['proposed_label'] for row in preserved.json()['rows']] == ['P-2', 'P-3']

    compact = client.post(f'/matters/{matter["id"]}/labels/preview', headers=auth, json={
        'ids': [doc['id'] for doc in remaining], 'mode': 'sequential',
        'prefix': 'P-', 'start': 1, 'existing_label_policy': 'compact'})
    assert compact.status_code == 200, compact.text
    assert [(row['old_label'], row['proposed_label']) for row in compact.json()['rows']] == [
        ('P-2', 'P-1'), ('P-3', 'P-2')]


def test_oversized_group_follows_saved_indivisible_or_split_policy(
        client, auth, matter):
    docs = [upload_text(client, auth, matter['id'], f'{name}.pdf', name) for name in
            ('group-one', 'group-two')]
    # Add a second page to each synthetic file by importing the shared two-page fixture
    # through the existing page insertion path would obscure this policy test, so update
    # the fixture documents with two-page replacements made locally.
    import pymupdf
    for doc in docs:
        with pymupdf.open() as pdf:
            pdf.new_page().insert_text((70, 90), 'page one')
            pdf.new_page().insert_text((70, 90), 'page two')
            replacement = pdf.tobytes()
        extra = client.post(f'/matters/{matter["id"]}/documents?included=true', headers=auth,
            files=[('files', (f'replacement-{doc["id"]}.pdf', replacement, 'application/pdf'))])
        assert extra.status_code == 201
        source = extra.json()[0]
        assert client.post(f'/documents/{doc["id"]}/replace', headers=auth,
                           json={'source_document_id': source['id']}).status_code == 200
        assert client.delete(f'/documents/{source["id"]}', headers=auth).status_code == 200
    grouped = client.patch(f'/matters/{matter["id"]}/documents/batch', headers=auth,
                           json={'ids': [doc['id'] for doc in docs], 'group_key': 'large-group'})
    assert grouped.status_code == 200, grouped.text
    assert client.patch(f'/matters/{matter["id"]}', headers=auth,
                        json={'max_pages_per_volume': 4,
                              'keep_groups_intact': True}).status_code == 200
    blocked = wait_for_job(client, auth, client.post(
        f'/matters/{matter["id"]}/build', headers=auth).json()['id'])
    assert blocked['status'] == 'failed'
    assert 'group' in blocked['error'] and 'No document was split' in blocked['error']

    assert client.patch(f'/matters/{matter["id"]}', headers=auth,
                        json={'keep_groups_intact': False}).status_code == 200
    split = wait_for_job(client, auth, client.post(
        f'/matters/{matter["id"]}/build', headers=auth).json()['id'])
    assert split['status'] == 'completed', split
    assert split['result']['volumes'] == 2
