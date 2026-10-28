"""B02B: labels are never physical destinations; independently reconcile exports."""
import base64
import csv
import io
import json
import pytest
import pymupdf
import pypdfium2 as pdfium
from app.engine import numbering_maps
from test_api import upload
from test_bundle_structure import settings, save, build


def doc(id,part=1):
    return dict(id=id,part=part,kind='petition',label='',title=id,revision=1,page_count=2)


@pytest.mark.parametrize('style,expected',[('arabic',['7','8']),('roman_lower',['vii','viii']),('roman_upper',['VII','VIII'])])
@pytest.mark.parametrize('restart',['volume','continuous','section'])
def test_numbering_policies(style,expected,restart):
    plans=[[doc('a')],[doc('b')],[doc('c',2)]]
    result=numbering_maps(plans,dict(numbering_style=style,numbering_restart=restart,
        numbering_start=7,numbering_prefix='V{volume} / P{section} · ',numbering_front_matter='roman'))
    assert [p['printed_label'] for p in result[0]['pages']]==['',*[f'V1 / P1 · {e}' for e in expected]]
    assert result[0]['pages'][0]['pdf_label']=='i'
    assert result[1]['pages'][1]['combined_page']==5
    # A Part spanning volumes retains its content counter; a new Part resets it.
    suffix=result[1]['pages'][1]['printed_label'].split(' · ')[1]
    assert suffix==('9' if style=='arabic' else 'ix' if style=='roman_lower' else 'IX') if restart!='volume' else suffix==expected[0]
    third=result[2]['pages'][1]['printed_label'].split(' · ')[1]
    assert third==('11' if style=='arabic' else 'xi' if style=='roman_lower' else 'XI') if restart=='continuous' else third==expected[0]


def test_blank_front_matter_separator_and_continuous_labels():
    a=doc('a');a.update(kind='annexure',label='A')
    result=numbering_maps([[a],[doc('b')]],dict(front_matter='cover_index',numbering_front_matter='none',annexure_mode='separator',numbering_restart='continuous'))
    assert [p['printed_label'] for p in result[0]['pages']]==['','','1','2','3']
    assert [p['pdf_label'] for p in result[1]['pages']]==['','','4','5']
    assert result[0]['rows'][0]['start_page']==4


@pytest.mark.parametrize('labels,index_links,back_links',[(True,True,True),(False,False,False),(True,False,True)])
def test_actual_preview_export_navigation_and_validation(client,auth,matter,make_pdf,labels,index_links,back_links):
    docs=[upload(client,auth,matter,make_pdf,name=f'synthetic-{i}.pdf',pages=2) for i in range(2)]
    store=client.app.state.store
    hashes={d['id']:store.sha256(store.original_path(d)) for d in docs}
    prior=build(client,auth,matter);prior_path=store.exports_root(matter['id'])/prior['id']/'combined.pdf';prior_hash=store.sha256(prior_path)
    cfg=settings(client,auth,matter);cfg['max_pages_per_volume']=4
    cfg['output_marks'].update(front_matter='cover_index',numbering_style='roman_upper',numbering_front_matter='roman',numbering_prefix='V{volume} · ',numbering_position='bottom_right',numbering_restart='continuous',numbering_pdf_labels=labels,numbering_linked_index=index_links,numbering_back_link=back_links,part_extracts_enabled=True,bates_enabled=True,bates_prefix='CB-')
    assert save(client,auth,matter,cfg).status_code==200
    body={k:cfg[k] for k in ('max_pages_per_volume','keep_groups_intact','output_marks')}
    plan=client.post(f'/matters/{matter["id"]}/front-matter-preview',headers=auth,json=body).json()
    assert [v['rows'][0]['printed_start'] for v in plan['volumes']]==['V1 · I','V2 · III']
    response=client.post(f'/matters/{matter["id"]}/numbering-preview',headers=auth,json={**body,'document_id':docs[1]['id'],'page':2,'expected_revision':docs[1]['revision']})
    assert response.status_code==200,response.text
    preview=response.json();assert preview['printed_label']=='V2 · IV';assert preview['combined_page']==8;assert preview['bates']=='CB-000004'
    result=build(client,auth,matter);folder=store.exports_root(matter['id'])/result['id']
    with pymupdf.open(folder/'combined.pdf') as pdf:
        assert pdf.page_count==8
        with pdfium.PdfDocument(str(folder/'combined.pdf')) as independent:
            assert [independent.get_page_label(i) for i in range(8)]==(['i','ii','V1 · I','V1 · II','i','ii','V2 · III','V2 · IV'] if labels else ['']*8)
        assert [l['page'] for l in pdf[1].get_links()]==([2] if index_links else [])
        assert [l['page'] for l in pdf[5].get_links()]==([6] if index_links else [])
        assert [l['page'] for l in pdf[7].get_links()]==([5] if back_links else [])
        assert all(1<=item[2]<=8 for item in pdf.get_toc())
        # Rendering the real source with identical marks must match the exported page.
        assert pdf[7].get_text()==preview['text']
        assert pdf[7].get_pixmap(matrix=pymupdf.Matrix(1.2,1.2),alpha=False).tobytes('png')==base64.b64decode(preview['image'].split(',')[1])
        for v in [1,2]:
            with pymupdf.open(folder/f'volume-{v:02}.pdf') as volume:
                assert [l['page'] for l in volume[3].get_links()]==([1] if back_links else [])
                assert volume[3].get_text()==pdf[(v-1)*4+3].get_text()
    rows=list(csv.DictReader(io.StringIO((folder/'index.csv').read_text(encoding='utf-8-sig'))))
    assert [r['printed_start'] for r in rows]==['V1 · I','V2 · III']
    assert [r['combined_start_page'] for r in rows]==['3','7']
    qa=client.get(f'/matters/{matter["id"]}/exports/{result["id"]}/validation/state',headers=auth).json()
    assert qa['can_download'],json.dumps(qa,indent=2)
    assert all(store.sha256(store.original_path(d))==hashes[d['id']] for d in docs)
    assert store.sha256(prior_path)==prior_hash


def test_numbering_atomic_reopen_conflict_and_bookmark_resolution(client,auth,matter,make_pdf):
    d=upload(client,auth,matter,make_pdf,pages=2)
    cfg=settings(client,auth,matter);old=json.loads(json.dumps(cfg));cfg['output_marks'].update(numbering_front_matter='roman',numbering_style='roman_lower')
    assert save(client,auth,matter,cfg).status_code==200
    assert save(client,auth,matter,old).status_code==409
    base=f'/matters/{matter["id"]}'
    assert client.post(base+'/history/undo',headers=auth).status_code==200
    assert settings(client,auth,matter)['output_marks']==old['output_marks']
    assert client.post(base+'/history/redo',headers=auth).status_code==200
    assert settings(client,auth,matter)['output_marks']['numbering_style']=='roman_lower'
    bookmark=dict(id='a'*32,title='Explicit target',document_id=d['id'],page_index=1,target_revision=d['revision'],parent_id=None)
    assert client.put(base+'/bookmarks',headers=auth,json={'entries':[bookmark],'expected_snapshot':old['input_snapshot']}).status_code==409
    assert client.put(base+'/bookmarks',headers=auth,json={'entries':[bookmark]}).status_code==200
    # A fabricated newer revision cannot be silently mapped to a current page.
    cfg=settings(client,auth,matter)
    body={k:cfg[k] for k in ('max_pages_per_volume','keep_groups_intact','output_marks')}
    assert client.post(base+'/numbering-preview',headers=auth,json={**body,'document_id':d['id'],'page':2,'expected_revision':d['revision']+1}).status_code==422
    assert client.post(base+'/numbering-preview',headers=auth,json={**body,'document_id':d['id'],'page':3,'expected_revision':d['revision']}).status_code==422

@pytest.mark.parametrize('position',['bottom_left','bottom_center','bottom_right','top_left','top_right'])
def test_stamp_positions_and_literal_prefixes(client,auth,matter,make_pdf,position):
    d=upload(client,auth,matter,make_pdf,pages=1)
    cfg=settings(client,auth,matter)
    cfg['output_marks'].update(numbering_prefix='A(é)\\ · ',numbering_position=position)
    assert save(client,auth,matter,cfg).status_code==200
    output=build(client,auth,matter)
    assert output['page_map_snapshot'][0]['printed_labels']==['A(é)\\ · 2']
    folder=client.app.state.store.exports_root(matter['id'])/output['id']
    with pdfium.PdfDocument(str(folder/'combined.pdf')) as pdf:
        assert pdf.get_page_label(1)=='A(é)\\ · 2'
    with pymupdf.open(folder/'combined.pdf') as pdf:
        rect=pdf[1].search_for('A(é)\\ · 2')[0]
        assert (rect.y0<pdf[1].rect.height/2)==position.startswith('top')
        if position.endswith('left'):assert rect.x0<pdf[1].rect.width/3
        if position.endswith('right'):assert rect.x0>pdf[1].rect.width*2/3
    state=client.get(f'/matters/{matter["id"]}/exports/{output["id"]}/validation/state',headers=auth).json()
    assert state['can_download'],state


def test_reordered_map_and_unresolved_bookmarks_are_explicit(client,auth,matter,make_pdf):
    docs=[upload(client,auth,matter,make_pdf,name=f'ordered-{i}.pdf',pages=2) for i in range(2)]
    base=f'/matters/{matter["id"]}'
    entries=[dict(id='a'*32,title='Resolved parent',document_id=docs[0]['id'],page_index=1,target_revision=docs[0]['revision'],parent_id=None),
             dict(id='b'*32,title='Missing revision',document_id=docs[1]['id'],page_index=0,target_revision=docs[1]['revision'],parent_id='a'*32)]
    assert client.put(base+'/bookmarks',headers=auth,json={'entries':entries}).status_code==200
    assert client.post(f'/documents/{docs[1]["id"]}/pages/rotate',headers=auth,json={'page_index':0,'degrees':180}).status_code==200
    before=client.post(base+'/bundle-preview',headers=auth,json={'max_pages_per_volume':10,'keep_groups_intact':True}).json()
    assert client.post(base+'/documents/reorder',headers=auth,json={'ids':[d['id'] for d in reversed(docs)]}).status_code==200
    after=client.post(base+'/bundle-preview',headers=auth,json={'max_pages_per_volume':10,'keep_groups_intact':True}).json()
    assert before['input_snapshot']!=after['input_snapshot']
    assert after['manual_bookmarks'][0]['combined_page']==5
    assert after['manual_bookmarks'][1]['combined_page'] is None
    output=build(client,auth,matter)
    with pymupdf.open(client.app.state.store.exports_root(matter['id'])/output['id']/'combined.pdf') as pdf:
        assert [t[2] for t in pdf.get_toc() if t[1]=='Resolved parent']==[5]
        assert not any(t[1]=='Missing revision' for t in pdf.get_toc())
