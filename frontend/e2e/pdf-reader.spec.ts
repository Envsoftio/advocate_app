import { enterLatestMatter } from './navigation'
import { closeDocumentBrowser, openReaderView, closeWorkspaceNavigation, openDocumentActions, openDocumentBrowser } from './navigation'
import { expect, test } from '@playwright/test'
import { execFileSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { resolve } from 'node:path'
const base='http://127.0.0.1:8768'
const headers={Authorization:'Bearer e2e-isolated-token'}
const backend=resolve(import.meta.dirname,'../../backend')
const python=process.env.MATTERDRAFT_TEST_PYTHON || [resolve(backend,'.venv312/bin/python'),resolve(backend,'.venv/bin/python')].find(existsSync)||'python3'
function fixture(count:number){return execFileSync(python,['-c',`import pymupdf,sys
pdf=pymupdf.open()
for i in range(${count}):
 p=pdf.new_page(width=420 if i%3 else 500,height=595)
 p.insert_text((36,65),f'Reader synthetic page {i+1}',fontsize=18)
 p.insert_text((36,110),'Needle Needle',fontsize=16)
 p.insert_text((36,150),'Alpha',fontsize=16)
 p.insert_text((36+pymupdf.get_text_length('Alpha',fontsize=16),150),'Beta',fontsize=16,fontname='hebo')
 if i==1:p.set_rotation(90)
pdf.set_toc([[1,'Beginning',1],[1,'Last page',${count}]])
pdf[0].insert_link({'kind':pymupdf.LINK_GOTO,'from':pymupdf.Rect(36,180,200,205),'page':${count}-1})
sys.stdout.buffer.write(pdf.tobytes())`])}

test('shared reader keeps numeric zoom, exact matches, continuous reading and document positions',async({page,request})=>{
  const errors:string[]=[];page.on('pageerror',error=>errors.push(error.stack || error.message))
  const created=await request.post(`${base}/matters`,{headers,data:{name:'E2E Shared Reader',court_profile:'general-demo',max_pages_per_volume:100}})
  const matter=await created.json()
  for(const [name,count] of [['reader-long.pdf',40],['reader-short.pdf',2]] as const){
    const uploaded=await request.post(`${base}/matters/${matter.id}/documents?included=true`,{headers,multipart:{files:{name,mimeType:'application/pdf',buffer:fixture(count)}}})
    expect(uploaded.ok()).toBeTruthy()
    const document=(await uploaded.json())[0]
    const annotated=await request.post(`${base}/documents/${document.id}/annotations`,{headers,data:{
      kind:'comment',page_index:1,expected_revision:1,rect_x:.1,rect_y:.2,rect_width:.1,rect_height:.05,
      body:count===40?'Long document review note':'Other document review note'}})
    expect(annotated.ok()).toBeTruthy()
  }
  await page.setViewportSize({width:1440,height:1000})
  await page.goto('/');await enterLatestMatter(page)
  await expect(page.locator('.open-workspace-tab.active .open-workspace-tab-select').filter({hasText:'E2E Shared Reader'})).toBeVisible()
  await openDocumentBrowser(page)
  await page.locator('.doc-item').filter({hasText:'reader-long'}).click()
  await expect(page.locator('.pdf-placement-surface')).not.toHaveClass(/is-stale/)
  await page.getByRole('combobox',{name:'Zoom',exact:true}).selectOption('100')
  await expect.poll(async()=> (await page.locator('.pdf-paper').boundingBox())?.width||0).toBeCloseTo(500*4/3,0)
  const numericWidth=(await page.locator('.pdf-paper').boundingBox())!.width
  await closeDocumentBrowser(page)
  await expect.poll(async()=> (await page.locator('.pdf-paper').boundingBox())!.width).toBeCloseTo(numericWidth,0)
  await page.getByRole('button',{name:'Search',exact:true}).click()
  await page.getByRole('searchbox',{name:'Find text in this PDF'}).fill('Needle')
  await expect(page.locator('.pdf-findbar')).toContainText('80 matches')
  await expect(page.locator('.pdf-search-highlight')).toHaveCount(2)
  await page.getByRole('button',{name:'Next search result',exact:true}).click()
  await expect(page.locator('.page-counter input')).toHaveValue('1')
  await expect(page.locator('.pdf-search-highlight.is-active')).toHaveAttribute('data-pdf-match','1')
  await page.getByRole('button',{name:'Next search result',exact:true}).click()
  await expect(page.locator('.page-counter input')).toHaveValue('2')
  await expect(page.locator('.pdf-text-layer')).toHaveAttribute('data-main-rotation','90')
  await expect(page.locator('.pdf-search-highlight.is-active')).toHaveAttribute('data-pdf-match','2')
  await page.getByRole('searchbox',{name:'Find text in this PDF'}).fill('AlphaBeta')
  await expect(page.locator('.pdf-findbar')).toContainText('40 matches')
  await page.getByRole('button',{name:'Close PDF search'}).click()
  await page.getByRole('combobox',{name:'Zoom',exact:true}).selectOption('fit-page')
  await expect.poll(async()=>{
    const canvas=(await page.locator('.pdf-paper').boundingBox())!, wrap=(await page.locator('.canvas-scroll').boundingBox())!
    return canvas.width<=wrap.width && canvas.height<=wrap.height
  }).toBeTruthy()
  await openReaderView(page)
  await page.getByRole('combobox',{name:'Reading mode'}).selectOption('continuous')
  await page.getByRole('combobox',{name:'Zoom',exact:true}).selectOption('100')
  await page.getByRole('spinbutton',{name:'Go to page',exact:true}).fill('30')
  await page.getByRole('spinbutton',{name:'Go to page',exact:true}).press('Enter')
  await expect(page.locator('.page-counter input')).toHaveValue('30')
  await expect(page.locator('[data-reader-page="30"] canvas')).toBeVisible()
  await expect(page.locator('.thumbnail-strip canvas[data-page="30"]')).toHaveAttribute('data-rendered','true')
  expect(await page.locator('.reader-page-frame').count()).toBeLessThanOrEqual(12)
  await page.locator('.canvas-scroll').evaluate(element=>{element.scrollTop+=220})
  await expect.poll(()=>page.locator('.canvas-scroll').evaluate(element=>element.scrollTop)).toBeGreaterThan(10000)
  const beforeZoom=await page.locator('.canvas-scroll').evaluate(element=>{
    const frame=element.querySelector('[data-reader-page="30"]')!;const a=frame.getBoundingClientRect(),b=element.getBoundingClientRect();return (b.top-a.top)/a.height
  })
  await page.getByRole('combobox',{name:'Zoom',exact:true}).selectOption('125')
  await expect.poll(()=>page.locator('.canvas-scroll').evaluate(element=>{
    const frame=element.querySelector('[data-reader-page="30"]')!;const a=frame.getBoundingClientRect(),b=element.getBoundingClientRect();return (b.top-a.top)/a.height
  })).toBeCloseTo(beforeZoom,1)
  await page.screenshot({path:test.info().outputPath('continuous-reader.png'),fullPage:true})
  await closeDocumentBrowser(page)
  await openDocumentBrowser(page)
  await page.locator('.doc-item').filter({hasText:'reader-short'}).click()
  await expect(page.locator('.page-counter')).toContainText('/ 2')
  await openDocumentBrowser(page)
  await page.locator('.doc-item').filter({hasText:'reader-long'}).click()
  await expect(page.locator('.page-counter input')).toHaveValue('30')
  await expect(page.getByRole('combobox',{name:'Zoom',exact:true})).toHaveValue('125')
  await openReaderView(page)
  await expect(page.getByRole('combobox',{name:'Reading mode'})).toHaveValue('continuous')
  await page.getByRole('button',{name:'View options'}).click()
  await page.getByRole('button',{name:'Document bookmarks',exact:true}).click()
  await page.getByRole('button',{name:/Last page · p40/}).click()
  await expect(page.locator('.page-counter input')).toHaveValue('40')
  await openReaderView(page)
  await page.getByRole('button',{name:'Back to previous reading position'}).click()
  await page.getByRole('button',{name:'View options'}).click()
  await expect(page.locator('.page-counter input')).toHaveValue('30')
  await page.getByRole('button',{name:'Document bookmarks',exact:true}).click()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', { name: 'Edit', exact: true }).click()
  await page.getByRole('button',{name:'Add text',exact:true}).click()
  await expect(page.locator('.reader-page-frame')).toHaveCount(1)
  await expect(page.locator('.pdf-placement-surface')).not.toHaveClass(/is-stale/)
  await page.getByRole('button',{name:'Select',exact:true}).click()
  await expect(page.locator('.canvas-scroll')).toHaveClass(/continuous-reader/)
  await page.getByRole('button',{name:'Search',exact:true}).click()
  await page.getByRole('searchbox',{name:'Find text in this PDF'}).fill('Needle')
  await expect(page.locator('.pdf-findbar')).toContainText('80 matches')
  await page.getByLabel('PDF search occurrences').getByRole('button').nth(2).click()
  await expect(page.locator('.page-counter input')).toHaveValue('2')
  await page.getByRole('button',{name:'Close PDF search'}).click()
  await page.getByRole('button',{name:'Comments (1)',exact:true}).click()
  await expect(page.getByLabel('Private review comments')).not.toContainText('Other document review note')
  await page.getByLabel('Private review comments').getByRole('button',{name:/Source 2.*Long document review note/}).click()
  await expect(page.locator('.page-counter input')).toHaveValue('2')
  await expect(page.getByRole('textbox',{name:'Comment text',exact:true})).toHaveValue('Long document review note')
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab',{name:'Pages',exact:true}).click()
  await page.getByRole('button',{name:'Rotate left',exact:true}).click()
  await page.getByRole('button',{name:'Comments (1)',exact:true}).click()
  await expect(page.getByLabel('Private review comments').getByRole('button',{name:/Source 2 Needs review Long document review note/})).toBeVisible()
  await page.getByLabel('Private review comments').getByRole('button',{name:/Source 2 Needs review Long document review note/}).click()
  await expect(page.getByLabel('PDF editing controls')).toContainText('Needs review after a revision change')
  await expect(page.getByRole('button',{name:'Update annotation',exact:true})).toHaveCount(0)
  expect(errors).toEqual([])
})

test('expanded editor fits saved zoom and keeps many comments out of the canvas',async({page,request})=>{
  const created=await request.post(`${base}/matters`,{headers,data:{name:'E2E Compact Editor',court_profile:'general-demo',max_pages_per_volume:100}})
  const matter=await created.json()
  const uploaded=await request.post(`${base}/matters/${matter.id}/documents?included=true`,{headers,multipart:{files:{name:'compact.pdf',mimeType:'application/pdf',buffer:fixture(2)}}})
  expect(uploaded.ok()).toBeTruthy()
  const document=(await uploaded.json())[0]
  const position=await request.put(`${base}/reader-position`,{headers,data:{key:`${document.id}:${document.revision}`,page:1,x:0,y:0,zoom:'fit-width',mode:'single'}})
  expect(position.ok()).toBeTruthy()
  for(let index=0;index<120;index++){
    const annotated=await request.post(`${base}/documents/${document.id}/annotations`,{headers,data:{kind:'comment',page_index:0,expected_revision:document.revision,rect_x:.1,rect_y:.2,rect_width:.1,rect_height:.05,body:`Review item ${index}`}})
    expect(annotated.ok()).toBeTruthy()
  }
  await page.setViewportSize({width:1440,height:900})
  await page.goto('/');await enterLatestMatter(page)
  await openDocumentBrowser(page)
  await page.locator('.doc-item').filter({hasText:'compact'}).click()
  await expect(page.getByRole('combobox',{name:'Zoom',exact:true})).toHaveValue('fit-width')
  await openDocumentActions(page)
  await page.getByRole('button',{name:'Full screen',exact:true}).click()
  await expect(page.getByRole('combobox',{name:'Zoom',exact:true})).toHaveValue('fit-page')
  await expect.poll(async()=>{
    const paper=(await page.locator('.pdf-paper').boundingBox())!
    const viewport=(await page.locator('.canvas-scroll').boundingBox())!
    return paper.width<=viewport.width && paper.height<=viewport.height
  }).toBeTruthy()
  expect((await page.locator('.viewer-toolbar').boundingBox())!.height).toBeLessThanOrEqual(64)
  await page.screenshot({path:test.info().outputPath('compact-expanded-editor.png')})
  const canvasHeight=(await page.locator('.canvas-scroll').boundingBox())!.height
  await page.getByRole('button',{name:'Comments (120)',exact:true}).click()
  await expect(page.getByLabel('Private review comments')).toBeVisible()
  await page.screenshot({path:test.info().outputPath('compact-comments-panel.png')})
  await expect.poll(async()=>Math.round((await page.locator('.canvas-scroll').boundingBox())!.height)).toBe(Math.round(canvasHeight))
  const list=page.locator('.reader-side-panel-list')
  expect(await list.evaluate(node=>node.scrollHeight>node.clientHeight)).toBeTruthy()
  await page.getByRole('searchbox',{name:'Search private comments'}).fill('Review item 119')
  await expect(page.getByLabel('Private review comments')).toContainText('1 of 120 comments')
  await expect(page.getByLabel('Private review comments').getByRole('button',{name:/Source 1.*Review item 119/})).toBeVisible()
  await page.getByRole('button',{name:'Close comments'}).click()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab',{name:'Annotate',exact:true}).click()
  const annotationCanvasHeight=(await page.locator('.canvas-scroll').boundingBox())!.height
  await page.getByRole('button',{name:'More annotation tools'}).click()
  await page.getByRole('button',{name:'Bookmark page'}).click()
  await page.getByRole('textbox',{name:'Bookmark title'}).fill('Review destination')
  await page.getByRole('button',{name:'Add bookmark'}).click()
  await expect.poll(async()=>Math.round((await page.locator('.canvas-scroll').boundingBox())!.height)).toBe(Math.round(annotationCanvasHeight))
  await page.getByRole('button',{name:/Page 1 bookmarks/}).click()
  await expect(page.getByLabel('Bookmarks on page 1')).toContainText('Review destination')
  await page.getByRole('button',{name:'Open Index & bookmarks'}).click()
  await expect(page.getByRole('navigation',{name:'Workspace navigation rail'}).getByRole('button',{name:'Index & bookmarks'})).toHaveAttribute('aria-current','page')
})
