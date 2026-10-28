"""B02A generated front matter, persistence, override and immutable output contract."""
import base64
import csv
import io
import json
import pymupdf
import pytest
from app.engine import volume_plan, volume_rows, bates_page_labels
from app.pdf_ops import ProcessingError
from app.storage import Store
from test_api import upload, wait_for_job


def settings(client, auth, matter):
    return client.get(f'/matters/{matter["id"]}/bundle-settings', headers=auth).json()


def save(client, auth, matter, value):
    return client.put(f'/matters/{matter["id"]}/bundle-settings', headers=auth, json={
        'max_pages_per_volume': value['max_pages_per_volume'],
        'keep_groups_intact': value['keep_groups_intact'], 'output_marks': value['output_marks'],
        'expected_snapshot': value['input_snapshot']})


def preview(client, auth, matter, value, **extra):
    return client.post(f'/matters/{matter["id"]}/front-matter-preview', headers=auth, json={
        'max_pages_per_volume': value['max_pages_per_volume'],
        'keep_groups_intact': value['keep_groups_intact'], 'output_marks': value['output_marks'], **extra})


def build(client, auth, matter):
    job = wait_for_job(client, auth, client.post(f'/matters/{matter["id"]}/build', headers=auth).json()['id'])
    assert job['status'] == 'completed', job
    return job['result']


def test_cover_preview_matches_export_map_links_labels_and_preserves_sources(client, auth, matter, make_pdf):
    docs = [upload(client, auth, matter, make_pdf, name=f'doc-{i}.pdf', pages=2) for i in range(2)]
    client.patch(f'/documents/{docs[0]["id"]}', headers=auth, json={'document_date':'2026-10-01', 'date_status':'confirmed', 'date_source':'user'})
    store = client.app.state.store
    hashes = {d['id']:store.sha256(store.original_path(d)) for d in docs}
    prior = build(client, auth, matter)
    prior_hash = store.sha256(store.exports_root(matter['id']) / prior['id'] / 'combined.pdf')
    value = settings(client, auth, matter)
    value['max_pages_per_volume'] = 4
    value['output_marks'].update(front_matter='cover_index', cover_heading='SYNTHETIC FORUM',
        cover_reference='DEMO 01/2026', cover_title='REVIEW BUNDLE', index_date=True, index_style='review',
        bates_enabled=True, bates_include_index=True, bates_prefix='CB-')
    before = settings(client, auth, matter)
    draft = preview(client, auth, matter, value).json()
    assert settings(client, auth, matter) == before  # read-only draft preview
    assert draft['combined_pages'] == 8 and draft['generated_pages'] == 4
    assert [v['cover_pages'] for v in draft['volumes']] == [1, 1]
    assert all(v['rows'][0]['start_page'] == 3 for v in draft['volumes'])
    assert 'SYNTHETIC FORUM' in draft['pages'][0]['text']
    assert '2026-10-01' in draft['pages'][1]['text']
    assert 'INDEX - VOLUME 1' in draft['pages'][1]['text']
    assert save(client, auth, matter, value).status_code == 200
    result = build(client, auth, matter)
    folder = store.exports_root(matter['id']) / result['id']
    with pymupdf.open(folder/'combined.pdf') as pdf:
        assert pdf.page_count == draft['combined_pages']
        assert [pdf[i].get_label() for i in range(8)] == ['1','2','3','4']*2
        assert [x['page'] for x in pdf[1].get_links()] == [2]
        assert [x['page'] for x in pdf[5].get_links()] == [6]
        assert any(t[2] == 7 for t in pdf.get_toc())
        for volume in [1,2]:
            check = preview(client, auth, matter, settings(client,auth,matter), volume=volume).json()
            for sheet in check['pages']:
                page = pdf[(volume-1)*4+sheet['physical_page']-1]
                assert page.get_text() == sheet['text']
                # Independently rasterize published PDF and compare generated preview pixels.
                png = base64.b64decode(sheet['image'].split(',')[1])
                assert page.get_pixmap(matrix=pymupdf.Matrix(1.2,1.2),alpha=False).tobytes('png') == png
    rows = list(csv.DictReader(io.StringIO((folder/'index.csv').read_text(encoding='utf-8-sig'))))
    assert [r['start_page'] for r in rows] == ['3','3']
    qa = client.get(f'/matters/{matter["id"]}/exports/{result["id"]}/validation/state',headers=auth).json()
    assert qa['can_download'], qa
    assert all(store.sha256(store.original_path(d)) == hashes[d['id']] for d in docs)
    assert store.sha256(store.exports_root(matter['id']) / prior['id'] / 'combined.pdf') == prior_hash


def test_atomic_settings_conflicts_undo_redo_and_reopen(client, auth, matter, make_pdf):
    upload(client, auth, matter, make_pdf)
    base = f'/matters/{matter["id"]}'
    before = settings(client,auth,matter)
    changed = json.loads(json.dumps(before))
    changed['max_pages_per_volume'] = 7
    changed['output_marks'].update(front_matter='cover_index',cover_title='Synthetic layout')
    assert save(client,auth,matter,changed).status_code == 200
    assert save(client,auth,matter,before).status_code == 409
    assert client.app.state.store.get_output_marks(matter['id'])['cover_title'] == 'Synthetic layout'
    assert client.post(base+'/history/undo', headers=auth).status_code == 200
    restored=settings(client,auth,matter)
    assert restored['output_marks'] == before['output_marks']
    assert restored['max_pages_per_volume'] == before['max_pages_per_volume']
    assert client.post(base+'/history/redo',headers=auth).status_code == 200
    assert settings(client,auth,matter)['output_marks']['front_matter']=='cover_index'
    bad=settings(client,auth,matter);bad['max_pages_per_volume']=1
    assert save(client,auth,matter,bad).status_code==422
    assert settings(client,auth,matter)['max_pages_per_volume']==7


def test_preset_preserves_explicit_overrides_and_matter_only_identity(client,auth,matter):
    base=f'/matters/{matter["id"]}'
    current=settings(client,auth,matter)
    def preset(name, cap, **marks):
        value={'name':name,'max_pages_per_volume':cap,'keep_groups_intact':True,
               'output_marks':{**current['output_marks'],**marks}}
        response=client.post('/output-presets',headers=auth,json=value)
        assert response.status_code==201,response.text
        return response.json()
    first=preset('Synthetic layout',10)
    second=preset('Synthetic layout',20,index_style='readable')
    assert second['version']==2
    assert client.post(base+'/output-preset/'+first['id'],headers=auth).status_code==200
    changed=settings(client,auth,matter);changed['max_pages_per_volume']=15
    changed['output_marks'].update(index_date=True,cover_title='Private matter title')
    assert save(client,auth,matter,changed).status_code==200
    snap=settings(client,auth,matter)['input_snapshot']
    assert client.post(base+'/output-preset/'+second['id'],headers=auth,json={
        'preserve_overrides':True,'expected_snapshot':snap}).status_code==200
    after=settings(client,auth,matter)
    assert after['max_pages_per_volume']==15 and after['output_marks']['index_date']
    assert after['output_marks']['index_style']=='readable'
    assert after['output_marks']['cover_title']=='Private matter title'
    assert after['applied']['settings']['max_pages_per_volume']==20
    assert after['applied']['provenance']=='Local user preset'
    assert client.post(base+'/output-preset/'+second['id'],headers=auth,json={
        'preserve_overrides':False,'expected_snapshot':after['input_snapshot']}).status_code==200
    after=settings(client,auth,matter)
    assert after['max_pages_per_volume']==20 and not after['output_marks']['index_date']
    assert after['output_marks']['cover_title']=='Private matter title'
    assert client.post('/output-presets',headers=auth,json={'name':'Not reusable',
        'max_pages_per_volume':10,'output_marks':after['output_marks']}).status_code==422


def test_cover_and_date_rules_drive_volume_capacity_and_bates():
    docs=[dict(id=str(i),title=f'Source {i}',part=1,kind='other',label='',page_count=1,group_key='') for i in range(17)]
    marks={'front_matter':'cover_index','index_date':True,'bates_enabled':True,'bates_include_index':True}
    plans=volume_plan(docs,19,False,marks)
    assert [len(p) for p in plans]==[16,1]  # seventeenth row needs a second index page
    rows,index_count=volume_rows(docs,1,marks)
    assert index_count==2 and rows[0]['start_page']==4 and rows[-1]['end_page']==20
    labels=bates_page_labels(plans,marks)
    assert labels[0][0] is None and labels[0][1]=='000001' and labels[1][0] is None
    with pytest.raises(ProcessingError):volume_plan(docs[:1],2,False,marks)


def test_spacious_review_index_counts_drive_page_map():
    docs=[dict(id=str(i),title=f'Source {i}',part=1,kind='other',label='',page_count=1,group_key='') for i in range(9)]
    marks={'front_matter':'cover_index','index_style':'review'}
    rows, count=volume_rows(docs,1,marks)
    assert count==2 and rows[0]['start_page']==4 and rows[-1]['end_page']==12
    assert [len(p) for p in volume_plan(docs,11,False,marks)]==[8,1]
