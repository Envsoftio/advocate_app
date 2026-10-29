import { completeQueuedImport, enterLatestMatter } from './navigation'
import { openBuildTool, submitBundleBuild, openOutputTools } from './navigation'
import {test,expect} from '@playwright/test'
import {readFile} from 'node:fs/promises'
import {resolve} from 'node:path'
const base=`http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT||8768}`
const headers={Authorization:'Bearer e2e-isolated-token'}
async function fixture(request:any,count=10){
 const matter=await(await request.post(`${base}/matters`,{headers,data:{name:'Synthetic membership review',case_number:'DEMO 2481/2026',court_profile:'general-demo'}})).json()
 const buffer=await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'))
 const docs=[]
 for(let i=0;i<count;i++){
  const response=await request.post(`${base}/matters/${matter.id}/documents`,{headers,multipart:{files:{name:`${i<8?'Source record':'Reference notes'} ${i+1}.pdf`,mimeType:'application/pdf',buffer}}})
  expect(response.ok()).toBeTruthy();docs.push((await response.json())[0])
 }
 const snapshot=await(await request.get(`${base}/matters/${matter.id}/membership`,{headers})).json()
 expect((await request.put(`${base}/matters/${matter.id}/membership`,{headers,data:{included_ids:docs.slice(0,8).map(d=>d.id),expected_snapshot:snapshot.input_snapshot}})).ok()).toBeTruthy()
 return {matter,docs}
}
async function enter(page:any){await page.locator('.application-status').getByRole('button',{name:'Bundle membership',exact:true}).click();await expect(page.getByRole('heading',{name:'Bundle membership',exact:true})).toBeVisible();await expect(page.getByRole('button',{name:'Apply membership',exact:true})).toBeEnabled()}
test('BU05 reference geometry, membership apply/discard/failure/stale and caller focus',async({page,request},info)=>{
 const {matter,docs}=await fixture(request)
 await page.setViewportSize({width:1440,height:900});await page.goto('/');await enterLatestMatter(page);await expect(page.locator('.pdf-paper')).toBeVisible();await enter(page)
 const panel=page.locator('.bundle-membership')
 await expect(panel.locator('input:checked')).toHaveCount(8)
 await expect(panel).toContainText('2 reference-only')
 for(const [selector,x,y,width] of [['.matter-rail',0,116,184],['.bundle-membership',184,116,1256],['.membership-card',208,266,1208]] as const){
  const box=(await page.locator(selector).boundingBox())!;expect(Math.round(box.x)).toBe(x);expect(Math.round(box.y)).toBe(y);expect(Math.round(box.width)).toBe(width)
 }
 await page.screenshot({path:info.outputPath('BU05-1440-light.png'),animations:'disabled'})
 const first=panel.getByRole('checkbox',{name:`Include ${docs[0].title}`,exact:true});await first.uncheck()
 await expect(panel).toContainText('7 included')
 await page.getByRole('button',{name:'Return to caller'}).click()
 await expect(page.getByRole('alertdialog')).toContainText('membership')
 await page.getByRole('button',{name:'Keep editing',exact:true}).click();await expect(panel).toBeVisible()
 await panel.getByRole('button',{name:'Cancel changes',exact:true}).click();await page.getByRole('button',{name:'Discard changes',exact:true}).click()
 await expect(panel).toBeHidden();await expect(page.locator('.application-status').getByRole('button',{name:'Bundle membership',exact:true})).toBeFocused()
 await enter(page);await expect(first).toBeChecked();await first.uncheck()
 await page.route('**/membership',route=>route.request().method()==='PUT'?route.fulfill({status:500,contentType:'application/json',body:JSON.stringify({detail:'Synthetic membership save failure'})}):route.continue())
 await panel.getByRole('button',{name:'Apply membership',exact:true}).click()
 await expect(panel.getByRole('alert')).toContainText('Synthetic membership save failure');await expect(first).not.toBeChecked()
 await page.screenshot({path:info.outputPath('BU05-save-error.png')})
 await page.unroute('**/membership')
 await panel.getByRole('button',{name:'Apply membership',exact:true}).click();await expect(panel).toBeHidden()
 let saved=await(await request.get(`${base}/matters/${matter.id}/membership`,{headers})).json();expect(saved.included_documents).toBe(7)
 await page.reload();await enterLatestMatter(page);await expect(page.locator('.pdf-paper')).toBeVisible();await enter(page);await expect(first).not.toBeChecked()
 await first.check()
 await request.patch(`${base}/documents/${docs[0].id}`,{headers,data:{title:'Changed by another session'}})
 await panel.getByRole('button',{name:'Apply membership',exact:true}).click();await expect(panel.getByRole('alert')).toContainText('inputs changed')
 await expect(panel.getByRole('button',{name:'Apply membership',exact:true})).toBeDisabled()
 await page.screenshot({path:info.outputPath('BU05-stale.png')})
 await panel.getByRole('button',{name:'Discard draft and reload'}).click();await expect(panel.getByRole('checkbox',{name:'Include Changed by another session'})).not.toBeChecked()
 await page.getByRole('button',{name:'Return to caller'}).click()
 saved=await(await request.get(`${base}/matters/${matter.id}/membership`,{headers})).json();expect(saved.included_documents).toBe(7)
})
test('BU05 themes/scales, scrolling, keyboard and loading/empty states',async({page,request},info)=>{
 const {docs}=await fixture(request,14)
 await page.goto('/');await enterLatestMatter(page);await expect(page.locator('.pdf-paper')).toBeVisible()
 for(const width of [1440,1024])for(const theme of ['light','dark'])for(const scale of [90,100,110,125]){
  await page.setViewportSize({width,height:width===1440?900:768})
  await page.evaluate(({theme,scale})=>localStorage.setItem('courtbundle.appearance.v1',JSON.stringify({theme,scale})),{theme,scale});await page.reload();await enterLatestMatter(page);await expect(page.locator('.pdf-paper')).toBeVisible();await enter(page)
  const panel=page.locator('.bundle-membership')
  await expect(panel.getByRole('button',{name:'Apply membership',exact:true})).toBeInViewport()
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth&&document.documentElement.scrollHeight<=innerHeight+1)).toBe(true)
  const last=panel.getByRole('checkbox',{name:`Include ${docs[13].title}`,exact:true});await last.scrollIntoViewIfNeeded();await last.focus();await page.keyboard.press('Space');await expect(last).toBeChecked()
  await panel.getByRole('button',{name:/Reference only ·/}).click();await expect(last).toBeHidden()
  if(scale===100){await panel.getByRole('button',{name:/All documents ·/}).click();await panel.locator('.membership-table-scroll').evaluate(el=>{el.scrollTop=0;el.scrollLeft=0});await page.screenshot({path:info.outputPath(`BU05-${width}-${theme}.png`),animations:'disabled'})}
  await page.getByRole('button',{name:'Return to caller'}).click();await page.getByRole('button',{name:'Discard changes',exact:true}).click()
 }
 await page.setViewportSize({width:1440,height:900})
 await page.route('**/membership',async route=>{await new Promise(resolve=>setTimeout(resolve,600));await route.continue()})
 await page.locator('.application-status').getByRole('button',{name:'Bundle membership',exact:true}).click();await expect(page.getByText('Loading bundle membership…')).toBeVisible();await expect(page.getByRole('button',{name:'Apply membership',exact:true})).toBeEnabled();await page.unroute('**/membership');await page.getByRole('button',{name:'Return to caller'}).click()
 const empty=await(await request.post(`${base}/matters`,{headers,data:{name:'Empty membership test',court_profile:'general-demo'}})).json()
 await page.reload();await enterLatestMatter(page);await page.locator('.application-status').getByRole('button',{name:'Bundle membership',exact:true}).click()
 await expect(page.getByText(/No documents yet. Import documents/)).toBeVisible();await page.screenshot({path:info.outputPath('BU05-empty.png')})
})

test('BU05 selection drives a real build/download and reference import stays readable',async({page,request})=>{
 const {matter,docs}=await fixture(request,3)
 await page.goto('/');await enterLatestMatter(page);await expect(page.locator('.pdf-paper')).toBeVisible()
 await page.locator('input[accept=".pdf,.jpg,.jpeg,.png"]').setInputFiles(resolve(import.meta.dirname,'../../samples/03_affidavit.pdf'))
 await completeQueuedImport(page)
 await expect.poll(async()=>(await(await request.get(`${base}/matters/${matter.id}/membership`,{headers})).json()).reference_documents).toBe(1)
 await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click()
 await page.locator('.build-workspace').getByRole('button',{name:'Bundle membership',exact:true}).click()
 const picker=page.locator('.bundle-membership');await expect(picker.getByRole('button',{name:'Apply membership',exact:true})).toBeEnabled()
 await picker.getByRole('checkbox',{name:`Include ${docs[1].title}`,exact:true}).uncheck()
 await picker.getByRole('button',{name:'Apply membership',exact:true}).click();await expect(picker).toBeHidden()
 await expect(page.locator('.build-heading')).toContainText('2 included documents · 2 reference-only')
 await submitBundleBuild(page)
 await expect(page.getByRole('heading',{name:'Bundle ready for review'})).toBeVisible({timeout:90000})
 const exported=(await(await request.get(`${base}/matters/${matter.id}/exports`,{headers})).json())[0]
 expect(exported.pages).toBe(7)
 expect(exported.input_revisions.map((d:any)=>d.document_id)).toEqual([docs[0].id,docs[2].id])
 const stored=await(await request.get(`${base}/matters/${matter.id}/exports/${exported.id}/filing-package.zip`,{headers})).body()
 await openOutputTools(page)
 const pending=page.waitForEvent('download');await page.getByRole('button',{name:'Download filing-package.zip',exact:true}).click()
 const downloaded=await pending;expect(await readFile((await downloaded.path())!)).toEqual(stored)
 await page.locator('.application-status').getByRole('button',{name:'Bundle membership',exact:true}).click()
 await picker.getByRole('button',{name:'Source record 2',exact:true}).click()
 await expect(page.locator('.reader-document strong')).toHaveText('Source record 2')
 await expect(page.locator('.pdf-paper')).toBeVisible()
 const source=await(await request.get(`${base}/documents/${docs[1].id}/original`,{headers})).body()
 expect(source).toEqual(await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf')))
})

test('BU05 keyboard commands and disconnected saves retain the membership draft',async({page,request})=>{
 const {matter,docs}=await fixture(request,3)
 await page.goto('/');await enterLatestMatter(page);await expect(page.locator('.pdf-paper')).toBeVisible();await enter(page)
 const picker=page.locator('.bundle-membership');const first=picker.getByRole('checkbox',{name:`Include ${docs[0].title}`,exact:true})
 await first.uncheck();await page.getByRole('heading',{name:'Bundle membership',exact:true}).focus()
 const before=await(await request.get(`${base}/matters/${matter.id}/membership`,{headers})).json()
 await page.keyboard.press('Control+z')
 expect((await(await request.get(`${base}/matters/${matter.id}/membership`,{headers})).json()).input_snapshot).toBe(before.input_snapshot)
 await expect(first).not.toBeChecked()
 await page.keyboard.press('Control+k');await expect(page.getByRole('alertdialog')).toBeVisible();await page.getByRole('button',{name:'Keep editing',exact:true}).click();await expect(picker).toBeVisible()
 await page.route('**/membership',route=>route.request().method()==='PUT'?route.abort():route.continue())
 await picker.getByRole('button',{name:'Apply membership',exact:true}).click()
 await expect(picker.locator('.engine-notice')).toBeVisible();await expect(first).not.toBeChecked();await expect(picker.getByRole('button',{name:'Apply membership',exact:true})).toBeDisabled()
 await page.unroute('**/membership');await picker.getByRole('button',{name:'Retry connection',exact:true}).click()
 await expect(picker.getByRole('button',{name:'Apply membership',exact:true})).toBeEnabled()
 expect((await(await request.get(`${base}/matters/${matter.id}/membership`,{headers})).json()).included_documents).toBe(3)
 await page.keyboard.press('Control+s');await expect(picker).toBeHidden()
 expect((await(await request.get(`${base}/matters/${matter.id}/membership`,{headers})).json()).included_documents).toBe(2)
 await enter(page);await page.keyboard.press('Control+k');await expect(picker).toBeHidden();await expect(page.getByRole('textbox',{name:'Search documents',exact:true})).toBeFocused()
})
