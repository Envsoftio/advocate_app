import { enterLatestMatter } from './navigation'
import { closeWorkspaceNavigation, openDocumentActions, openDocumentBrowser } from './navigation'
import { test, expect } from '@playwright/test'
import { execFileSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { resolve } from 'node:path'
const base='http://127.0.0.1:8768', headers={Authorization:'Bearer e2e-isolated-token'}
const backend=resolve(import.meta.dirname,'../../backend')
const python=[resolve(backend,'.venv312/bin/python'),resolve(backend,'.venv/bin/python')].find(existsSync)||'python3'
test('new reader follows ED36 shell geometry and keeps its real navigation usable', async ({page,request}, testInfo)=>{
  page.setDefaultTimeout(15000)
  const errors:string[]=[];page.on('pageerror',e=>errors.push(e.message))
  const matter=await (await request.post(`${base}/matters`,{headers,data:{name:'Sharma v. State of Maharashtra',court_profile:'general-demo'}})).json()
  const pdf=execFileSync(python,['-c',`import pymupdf,sys
pdf=pymupdf.open()
for i in range(32):
 p=pdf.new_page(width=595,height=842)
 p.insert_textbox((45,40,550,76),'ANNEXURE B',fontsize=18,fontname='tibo',align=1)
 p.insert_textbox((45,70,550,103),'EMAIL CORRESPONDENCE',fontsize=14,fontname='tibo',align=1)
 for y in [110,225]:p.draw_line((45,y),(550,y),color=(0.8,0.85,0.9),width=.6)
 for y,label,value in [(135,'From','legal.records@example.invalid'),(158,'To','matter.team@example.invalid'),(181,'Date','14 January 2026, 10:42'),(204,'Subject','Notice and supporting correspondence')]:
  p.insert_text((45,y),label,fontsize=11,fontname='hebo');p.insert_text((115,y),value,fontsize=11)
 p.insert_text((45,258),'Dear Counsel,',fontsize=16,fontname='tiro')
 p.insert_textbox((45,280,550,360),'Please find attached the correspondence exchanged between\\nthe parties concerning the notice dated 12 January 2026.',fontsize=16,fontname='tiro')
 p.insert_textbox((45,365,550,460),'The enclosed records should be read with the agreement and\\nthe earlier notice. Please confirm the date of receipt before\\nthe bundle is finalized.',fontsize=16,fontname='tiro')
sys.stdout.buffer.write(pdf.tobytes())`])
  const upload=await request.post(`${base}/matters/${matter.id}/documents?included=true`,{headers,multipart:{files:{name:'Annexure B — Email Correspondence.pdf',mimeType:'application/pdf',buffer:pdf}}});expect(upload.ok()).toBeTruthy()
  await page.setViewportSize({width:1024,height:768});await page.goto('/');await enterLatestMatter(page);if(await page.getByRole('button',{name:'Collapse sidebar',exact:true}).isVisible())await page.getByRole('button',{name:'Collapse sidebar',exact:true}).click()
  await openDocumentBrowser(page)
  await page.locator('.doc-item').filter({hasText:'Email Correspondence'}).click()
  await expect(page.locator('.pdf-paper')).toBeVisible()
  await page.locator('.page-counter input').fill('14');await page.locator('.page-counter input').press('Enter')
  await expect(page.locator('.pdf-text-layer')).toContainText('ANNEXURE B')
  await expect(page.locator('.docs-panel')).toBeHidden()
  for(const [selector,y,height] of [['.application-bar',0,32],['.document-header',116,64],['.reader-tool-navigation',180,44],['.read-workspace>.reader-navigation',224,44]] as const){
    await expect.poll(async()=>Math.round((await page.locator(selector).boundingBox())!.y)).toBe(y)
    expect(Math.round((await page.locator(selector).boundingBox())!.height)).toBe(height)
  }
  expect(Math.round((await page.locator('.viewer-page-column').boundingBox())!.x)).toBe(264)
  await page.locator('.reader-document strong').click()
  await expect(page.locator('.thumbnail-button.selected canvas')).toHaveAttribute('data-rendered','true')
  await page.screenshot({animations:'disabled',path:testInfo.outputPath('ED36-1024-light.png')})
  await page.getByRole('button',{name:'Search',exact:true}).click()
  await page.getByRole('searchbox',{name:'Find text in this PDF'}).fill('agreement')
  await expect(page.locator('.pdf-findbar')).toContainText('32 matches')
  await page.getByRole('button',{name:'Close PDF search'}).click()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab',{name:'OCR',exact:true}).click()
  await expect(page.getByRole('button',{name:'Run English OCR'})).toBeVisible()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab',{name:'Read',exact:true}).click()
  await page.getByRole('button',{name:'More document actions'}).click()
  await openDocumentActions(page)
  await expect(page.getByRole('button',{name:'Save working copy'})).toBeVisible()
  await page.getByRole('button',{name:'More document actions'}).click()
  for(const width of [1440,1024])for(const theme of ['light','dark'])for(const scale of [90,100,110,125]){
    await page.setViewportSize({width,height:width===1440?900:768})
    await page.evaluate(({theme,scale})=>localStorage.setItem('courtbundle.appearance.v1',JSON.stringify({theme,scale})),{theme,scale})
    await page.reload();await enterLatestMatter(page);if(await page.getByRole('button',{name:'Collapse sidebar',exact:true}).isVisible())await page.getByRole('button',{name:'Collapse sidebar',exact:true}).click()
    // Browsing is an explicit drawer. Close it if restoring the matter instead of its document tab.
    const close=page.getByRole('button',{name:'Close document browser'})
    if(await close.isVisible())await close.click()
    await expect(page.locator('.pdf-paper')).toBeVisible()
    await expect.poll(async()=>(await page.locator('.viewer-page-column').boundingBox())!.width).toBeGreaterThan(400)
    await expect.poll(async()=>(await page.locator('.viewer-page-column').boundingBox())!.height).toBeGreaterThan(380)
    expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth && document.documentElement.scrollHeight<=innerHeight+1)).toBe(true)
    if(scale===100)await expect(page.locator('.thumbnail-button.selected canvas')).toHaveAttribute('data-rendered','true')
    if(scale===100)await page.screenshot({animations:'disabled',path:testInfo.outputPath(`reader-${width}-${theme}.png`)})
  }
  await page.setViewportSize({width:1440,height:900})
  await page.evaluate(()=>localStorage.setItem('courtbundle.appearance.v1',JSON.stringify({theme:'light',scale:100})));await page.reload();await enterLatestMatter(page);if(await page.getByRole('button',{name:'Collapse sidebar',exact:true}).isVisible())await page.getByRole('button',{name:'Collapse sidebar',exact:true}).click()
  await page.getByRole('combobox',{name:'Document version'}).selectOption('original')
  const original=page.getByRole('dialog',{name:'Review imported original'})
  await expect(original.locator('.pdf-text-layer')).toContainText('ANNEXURE B')
  await original.getByRole('button',{name:'Artifact information'}).click()
  await expect(original.getByRole('complementary',{name:'Artifact information'})).toContainText('Source file unchanged')
  await page.screenshot({animations:'disabled',path:testInfo.outputPath('original-1440-light.png')})
  expect(errors).toEqual([])
})
