import { closeWorkspaceNavigation, openDocumentActions } from './navigation'
import {expect,test} from '@playwright/test'
import {readFile} from 'node:fs/promises'
import {resolve} from 'node:path'

const base=`http://127.0.0.1:${process.env.COURTBUNDLE_E2E_BACKEND_PORT || 8768}`
const headers={Authorization:'Bearer e2e-isolated-token'}

test('select, drag, and Delete affect only authored overlays',async({page,request})=>{
  await page.setViewportSize({width:1440,height:1000})
  const matter=await (await request.post(`${base}/matters`,{headers,data:{
    name:'E2E Overlay Selection',court_profile:'general-demo',max_pages_per_volume:10,
  }})).json()
  const uploaded=await request.post(`${base}/matters/${matter.id}/documents?included=true`,{headers,multipart:{
    files:{name:'01_petition.pdf',mimeType:'application/pdf',buffer:await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'))},
  }})
  expect(uploaded.ok()).toBeTruthy()
  const document=(await uploaded.json())[0]
  const created=await request.post(`${base}/documents/${document.id}/annotations`,{headers,data:{
    kind:'highlight',page_index:0,expected_revision:document.revision,
    rect_x:.1,rect_y:.1,rect_width:.18,rect_height:.05,body:'Move me',color:'yellow',
  }})
  expect(created.ok()).toBeTruthy()
  const link=await request.put(`${base}/matters/${matter.id}/links`,{headers,data:{entries:[{
    id:'a1b2c3d4e5f60718293a4b5c6d7e8f90',title:'Review link',source_document_id:document.id,
    source_page_index:0,source_revision:document.revision,rect_x:.4,rect_y:.1,
    rect_width:.18,rect_height:.05,kind:'external',target_document_id:null,
    target_page_index:null,target_revision:null,uri:'https://example.com',
  }]}})
  expect(link.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Overlay Selection'})).toBeVisible()
  const highlight=page.locator('.pdf-review-highlight')
  const reviewedLink=page.locator('.pdf-reviewed-link')
  await expect(highlight).toBeVisible()
  await highlight.click()
  await expect(highlight).toHaveClass(/selected/)
  await expect(page.locator('.pdf-placement-surface')).not.toHaveClass(/is-stale/)
  const before=await highlight.boundingBox()
  expect(before).toBeTruthy()
  await page.mouse.move(before!.x+before!.width/2,before!.y+before!.height/2)
  await page.mouse.down()
  await page.mouse.move(before!.x+before!.width/2+55,before!.y+before!.height/2+35,{steps:6})
  await page.mouse.up()
  await expect.poll(async()=>{
    const entries=await (await request.get(`${base}/matters/${matter.id}/annotations`,{headers})).json()
    return entries[0].rect_x
  }).toBeGreaterThan(.1)
  await reviewedLink.click()
  await expect(reviewedLink).toHaveClass(/selected/)
  await expect(page.locator('.pdf-placement-surface')).not.toHaveClass(/is-stale/)
  const linkBefore=await reviewedLink.boundingBox()
  expect(linkBefore).toBeTruthy()
  await page.mouse.move(linkBefore!.x+linkBefore!.width/2,linkBefore!.y+linkBefore!.height/2)
  await page.mouse.down()
  await page.mouse.move(linkBefore!.x+linkBefore!.width/2+38,linkBefore!.y+linkBefore!.height/2+24,{steps:6})
  await page.mouse.up()
  await expect.poll(async()=>{
    const entries=await (await request.get(`${base}/matters/${matter.id}/links`,{headers})).json()
    return entries[0].rect_x
  }).toBeGreaterThan(.4)
  const handle=page.getByRole('button',{name:'Resize selected Reviewed link'})
  await expect(handle).toBeVisible()
  const corner=await handle.boundingBox()
  expect(corner).toBeTruthy()
  await page.mouse.move(corner!.x+corner!.width/2,corner!.y+corner!.height/2)
  await page.mouse.down()
  await page.mouse.move(corner!.x+corner!.width/2+30,corner!.y+corner!.height/2+20,{steps:6})
  await page.mouse.up()
  await expect.poll(async()=>{
    const entries=await (await request.get(`${base}/matters/${matter.id}/links`,{headers})).json()
    return entries[0].rect_width
  }).toBeGreaterThan(.18)
  await reviewedLink.press('Delete')
  await expect(reviewedLink).toHaveCount(0)
  await expect.poll(async()=> (await (await request.get(`${base}/matters/${matter.id}/links`,{headers})).json()).length).toBe(0)
  await highlight.click()
  await highlight.press('Delete')
  await expect(highlight).toHaveCount(0)
  await expect.poll(async()=> (await (await request.get(`${base}/matters/${matter.id}/annotations`,{headers})).json()).length).toBe(0)
  const refreshed=await (await request.get(`${base}/matters/${matter.id}/documents`,{headers})).json()
  expect(refreshed[0].revision).toBe(document.revision)
})

test('Annotate and Stamps retain independent editable marks',async({page,request})=>{
  await page.setViewportSize({width:1440,height:1000})
  const matter=await (await request.post(`${base}/matters`,{headers,data:{
    name:'E2E Independent Marks',court_profile:'general-demo',max_pages_per_volume:10,
  }})).json()
  const uploaded=await request.post(`${base}/matters/${matter.id}/documents?included=true`,{headers,multipart:{
    files:{name:'01_petition.pdf',mimeType:'application/pdf',buffer:await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'))},
  }})
  expect(uploaded.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Independent Marks'})).toBeVisible()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab',{name:'Annotate'}).click()
  for (const name of ['Highlight','Comment','Draw','Box','Ellipse','Line','Arrow'])
    await expect(page.getByRole('button',{name,exact:true})).toBeVisible()
  await page.screenshot({path:test.info().outputPath('annotation-toolbar.png')})
  await page.getByRole('button',{name:'Box',exact:true}).click()
  await expect(page.getByText('This editable mark is included in Save copy and new bundle exports.')).toBeVisible()
  await expect(page.locator('.pdf-placement-surface')).not.toHaveClass(/is-stale/)
  const paper=await page.locator('.pdf-paper').boundingBox()
  expect(paper).toBeTruthy()
  await page.mouse.move(paper!.x+80,paper!.y+60)
  await page.mouse.down()
  await page.mouse.move(paper!.x+180,paper!.y+115,{steps:5})
  await page.mouse.up()
  await page.getByRole('button',{name:'Add mark'}).click()
  const box=page.locator('.pdf-review-shape.box:not(.draft)')
  await expect(box).toBeVisible()
  const boxWidth=(await (await request.get(`${base}/matters/${matter.id}/annotations`,{headers})).json())[0].rect_width as number
  await box.click()
  const boxHandle=page.getByRole('button',{name:'Resize selected Box'})
  const boxCorner=await boxHandle.boundingBox()
  expect(boxCorner).toBeTruthy()
  await page.mouse.move(boxCorner!.x+boxCorner!.width/2,boxCorner!.y+boxCorner!.height/2)
  await page.mouse.down()
  await page.mouse.move(boxCorner!.x+boxCorner!.width/2+25,boxCorner!.y+boxCorner!.height/2+10,{steps:5})
  await page.mouse.up()
  await expect.poll(async()=> (await (await request.get(`${base}/matters/${matter.id}/annotations`,{headers})).json())[0].rect_width as number).toBeGreaterThan(boxWidth)
  await page.getByRole('button',{name:'Draw',exact:true}).click()
  const drawPaper=await page.locator('.pdf-paper').boundingBox()
  expect(drawPaper).toBeTruthy()
  await page.mouse.move(drawPaper!.x+100,drawPaper!.y+160)
  await page.mouse.down()
  await page.mouse.move(drawPaper!.x+145,drawPaper!.y+185,{steps:8})
  await page.mouse.move(drawPaper!.x+205,drawPaper!.y+165,{steps:8})
  await page.mouse.up()
  await page.getByRole('button',{name:'Add mark'}).click()
  const drawing=page.locator('.pdf-review-shape.draw:not(.draft)')
  await expect(drawing).toBeVisible()
  await openDocumentActions(page)
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab',{name:'Stamps'}).click()
  await expect(page.getByText('no verified signature or form filling')).toBeVisible()
  await expect(page.getByRole('button',{name:'Check',exact:true})).toBeVisible()
  await expect(page.getByRole('button',{name:'Date',exact:true})).toBeVisible()
  await page.reload()
  await expect(box).toBeVisible()
  await expect(drawing).toBeVisible()
  await box.click()
  await box.press('Delete')
  await expect(box).toHaveCount(0)
  await drawing.click()
  await drawing.press('Delete')
  await expect(drawing).toHaveCount(0)
  await expect.poll(async()=> (await (await request.get(`${base}/matters/${matter.id}/annotations`,{headers})).json()).length).toBe(0)
})
