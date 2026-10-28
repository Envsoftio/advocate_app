import hashlib
import json
import pymupdf
import pytest
from app.metadata_review import date_candidates
from app.storage import Store
from test_document_table import upload


def preview(client, auth, matter, docs, pages=None):
    scope = {'ids': [d['id'] for d in docs], 'pages': pages or {}}
    r = client.post(f'/matters/{matter["id"]}/metadata-suggestions', headers=auth, json=scope)
    assert r.status_code == 200, r.text
    return scope, r.json()


def accept(client, auth, matter, scope, result, fields, **extra):
    return client.post(f'/matters/{matter["id"]}/metadata-suggestions/accept', headers=auth,
        json={**scope, 'expected_snapshot': result['input_snapshot'], 'fields': [
            {'document_id': row['document_id'], 'field': row['field'], 'evidence_id': row['evidence_id'], 'value': value}
            for row, value in fields], **extra})


def documents(client, auth, matter):
    return client.get(f'/matters/{matter["id"]}/documents', headers=auth).json()


def test_accept_exact_field_persists_history_and_preserves_original(client, auth, matter):
    docs = [upload(client, auth, matter['id'], n) for n in ['Order_12_Jan_2026.pdf', 'other.pdf']]
    original = client.app.state.store.sha256(client.app.state.store.original_path(docs[0]))
    scope, result = preview(client, auth, matter, docs)
    assert documents(client, auth, matter) == docs
    title, date_row = result['rows'][:2]
    assert date_row['proposed'] == '2026-01-12'
    r = accept(client, auth, matter, scope, result, [(title, 'Reviewed order')])
    assert r.status_code == 200, r.text
    changed = r.json()[0]
    assert {k for k in changed if changed[k] != docs[0][k]} == {'title', 'updated_at'}
    assert r.json()[1] == docs[1]
    assert client.post(f'/matters/{matter["id"]}/history/undo', headers=auth).status_code == 200
    assert documents(client, auth, matter)[0]['title'] == docs[0]['title']
    assert client.post(f'/matters/{matter["id"]}/history/redo', headers=auth).status_code == 200
    assert documents(client, auth, matter)[0]['title'] == 'Reviewed order'
    assert client.app.state.store.sha256(client.app.state.store.original_path(docs[0])) == original
    store = client.app.state.store
    with store.connect() as con:
        saved = con.execute("SELECT after_json FROM edit_history WHERE kind='document_batch' ORDER BY id DESC LIMIT 1").fetchone()
        assert json.loads(saved[0])['metadata_evidence'][0]['source_sha256'] == docs[0]['source_sha256']
    # New Store instance simulates reopened storage after releasing the single-engine lock.
    root = store.root
    store.close()
    reopened = Store(root)
    try:
        assert reopened.get_documents(matter['id'])[0]['title'] == 'Reviewed order'
    finally:
        reopened.close()


@pytest.mark.parametrize('text,value,ambiguous', [
    ('Dated: 12 January 2026', '2026-01-12', False),
    ('2026-02-31', '', True), ('03/04/2026', '', True),
    ('2026-01-12 and 2026-01-13', '', True), ('No date', '', False),
])
def test_conservative_date_evidence(text, value, ambiguous):
    actual, _, uncertain = date_candidates(text)
    assert (actual, uncertain) == (value, ambiguous)


def test_ambiguous_missing_unavailable_invalid_and_scope(client, auth, matter, make_pdf):
    docs = [upload(client, auth, matter['id'], '03-04-2026.pdf'), upload(client, auth, matter['id'], 'no-date.pdf')]
    scope, result = preview(client, auth, matter, docs)
    assert result['rows'][1]['proposed'] == '' and result['rows'][1]['status'] == 'uncertain'
    assert result['rows'][3]['status'] == 'unavailable'
    for value in ['2026-02-31', '03/04/2026', '', '2026-2-3']:
        r = accept(client, auth, matter, scope, result, [(result['rows'][1], value)])
        assert r.status_code == 422, r.text
    r = accept(client, auth, matter, scope, result, [(result['rows'][1], '2026-04-03')])
    assert r.status_code == 200
    assert r.json()[0]['date_status'] == 'confirmed'
    path = client.app.state.store.working_path(docs[1]); path.unlink()
    scope, result = preview(client, auth, matter, docs)
    r = accept(client, auth, matter, scope, result, [(result['rows'][3], '2026-04-03')])
    assert r.status_code == 422
    assert documents(client, auth, matter)[1]['document_date'] == ''
    assert client.post(f'/matters/{matter["id"]}/metadata-suggestions', headers=auth,
                       json={'ids': ['missing']}).status_code == 422


def test_conflict_evidence_change_and_atomic_failure_retry(client, auth, matter, monkeypatch):
    docs = [upload(client, auth, matter['id'], 'Order_2026-01-12.pdf')]
    scope, result = preview(client, auth, matter, docs)
    client.patch(f'/documents/{docs[0]["id"]}', headers=auth, json={'title': 'Newer confirmed title'})
    assert accept(client, auth, matter, scope, result, [(result['rows'][0], 'Old proposal')]).status_code == 409
    scope, result = preview(client, auth, matter, docs)
    store = client.app.state.store
    audit = store._audit
    monkeypatch.setattr(store, '_audit', lambda *a, **kw: (_ for _ in ()).throw(ValueError('Synthetic failure')))
    failed = accept(client, auth, matter, scope, result, [(result['rows'][0], 'Retry title'), (result['rows'][1], '2026-01-12')])
    assert failed.status_code == 422
    assert documents(client, auth, matter)[0]['title'] == 'Newer confirmed title'
    assert documents(client, auth, matter)[0]['document_date'] == ''
    monkeypatch.setattr(store, '_audit', audit)
    assert accept(client, auth, matter, scope, result, [(result['rows'][0], 'Retry title')]).status_code == 200
    scope, result = preview(client, auth, matter, docs)
    # Same revision but changed source bytes must fail evidence binding too.
    with pymupdf.open() as pdf:
        pdf.new_page().insert_text((72, 90), 'Different source 2025-02-01')
        pdf.save(store.working_path(docs[0]))
    assert accept(client, auth, matter, scope, result, [(result['rows'][1], '2026-01-12')]).status_code == 409


def test_explicit_group_sort_unknown_equal_and_history(client, auth, matter):
    docs = [upload(client, auth, matter['id'], f'doc_{i}.pdf') for i in range(6)]
    for i in [0,2,3,4,5]:
        client.patch(f'/documents/{docs[i]["id"]}', headers=auth, json={'group_key':'A'})
    for i, value in [(0,'2026-03-01'),(2,'2026-01-01'),(3,'2026-01-01')]:
        client.patch(f'/documents/{docs[i]["id"]}', headers=auth, json={'document_date':value,'date_status':'confirmed','date_source':'user'})
    scope, result = preview(client, auth, matter, docs)
    r = accept(client, auth, matter, scope, result, [(result['rows'][0], 'Accepted title')])
    assert [d['id'] for d in r.json()] == [d['id'] for d in docs]
    scope, result = preview(client, auth, matter, docs)
    r = accept(client, auth, matter, scope, result, [(result['rows'][0], 'Sorted title')], sort_group='A')
    assert r.status_code == 200, r.text
    assert [d['id'] for d in r.json()] == [docs[i]['id'] for i in [2,1,3,0,4,5]]
    client.post(f'/matters/{matter["id"]}/history/undo', headers=auth)
    assert [d['id'] for d in documents(client, auth, matter)] == [d['id'] for d in docs]


def test_batch_save_atomic_conflict_and_selected_page_preview(client, auth, matter, make_pdf):
    docs = [upload(client, auth, matter['id'], 'first.pdf'), upload(client, auth, matter['id'], 'second.pdf')]
    review = client.get(f'/matters/{matter["id"]}/metadata-review', headers=auth).json()
    body = {'expected_snapshot':review['input_snapshot'],'rows':[{'document_id':d['id'],'changes':{'title':f'Edited {i}'}} for i,d in enumerate(docs)]}
    assert client.put(f'/matters/{matter["id"]}/metadata-review',headers=auth,json=body).status_code == 200
    assert client.put(f'/matters/{matter["id"]}/metadata-review',headers=auth,json=body).status_code == 409
    client.post(f'/matters/{matter["id"]}/history/undo', headers=auth)
    assert [d['title'] for d in documents(client, auth, matter)] == [d['title'] for d in docs]
    source = client.get(f'/documents/{docs[0]["id"]}/metadata-source?revision=1&page=1',headers=auth)
    assert source.status_code == 200 and source.json()['image'].startswith('data:image/png;base64,')
    assert client.get(f'/documents/{docs[0]["id"]}/metadata-source?revision=2&page=1',headers=auth).status_code == 409


def test_selected_page_scanned_evidence_and_regenerated_output(client, auth, matter):
    from test_bundle_structure import build, settings, save
    with pymupdf.open() as pdf:
        pdf.new_page().insert_text((72, 90), 'Source cover without a date')
        pdf.new_page().insert_text((72, 90), 'ORDER\nDated: 18 February 2026')
        data = pdf.tobytes()
    response = client.post(f'/matters/{matter["id"]}/documents?included=true', headers=auth,
                           files=[('files', ('Court_Order.pdf', data, 'application/pdf'))])
    docs = response.json()
    store = client.app.state.store
    originals = {d['id']: store.sha256(store.original_path(d)) for d in docs}
    scope, first = preview(client, auth, matter, docs)
    assert first['rows'][1]['status'] == 'unavailable'
    scope, result = preview(client, auth, matter, docs, {docs[0]['id']: 2})
    assert result['rows'][1]['proposed'] == '2026-02-18'
    assert result['rows'][1]['source'] == 'page' and result['rows'][1]['page'] == 2
    value = settings(client, auth, matter)
    value['output_marks']['index_date'] = True
    assert save(client, auth, matter, value).status_code == 200
    prior = build(client, auth, matter)
    folder = store.exports_root(matter['id']) / prior['id']
    hashes = {p.name: store.sha256(p) for p in folder.iterdir() if p.is_file()}
    scope, result = preview(client, auth, matter, docs, {docs[0]['id']: 2})
    before_digest = store.build_input_digest(matter['id'])
    r = accept(client, auth, matter, scope, result,
               [(result['rows'][0], 'Accepted source order'), (result['rows'][1], '2026-02-18')])
    assert r.status_code == 200, r.text
    assert store.build_input_digest(matter['id']) != before_digest
    exports = client.get(f'/matters/{matter["id"]}/exports', headers=auth).json()
    assert next(e for e in exports if e['id'] == prior['id'])['stale']
    rebuilt = build(client, auth, matter)
    with pymupdf.open(store.exports_root(matter['id']) / rebuilt['id'] / 'combined.pdf') as pdf:
        assert 'Accepted source order' in pdf[0].get_text()
        assert '2026-02-18' in pdf[0].get_text()
        assert any('Accepted source order' in row[1] for row in pdf.get_toc())
    state = client.get(f'/matters/{matter["id"]}/exports/{rebuilt["id"]}/validation/state', headers=auth).json()
    assert state['can_download']
    assert hashes == {p.name: store.sha256(p) for p in folder.iterdir() if p.is_file()}
    assert originals == {d['id']: store.sha256(store.original_path(d)) for d in docs}
