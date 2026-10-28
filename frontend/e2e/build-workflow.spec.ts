import {test,expect} from '@playwright/test'
import {openWorkspaceNavigation} from './navigation'
import {readFile} from 'node:fs/promises'
import {resolve} from 'node:path'
import {createHash} from 'node:crypto'
const base=`http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT||8768}`
const headers={Authorization:'Bearer e2e-isolated-token'}
const hash=(b:Buffer)=>createHash('sha256').update(b).digest('hex')
async function fixture(request:any){
 const matter=await(await request.post(`${base}/matters`,{headers,data:{name:'Synthetic build review',case_number:'DEMO 2481/2026',court_profile:'general-demo'}})).json()
 const buffer=await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'))
 const docs=[]
 for(const name of ['Petition.pdf','Reference notes.pdf']){const r=await request.post(`${base}/matters/${matter.id}/documents`,{headers,multipart:{files:{name,mimeType:'application/pdf',buffer}}});expect(r.ok()).toBeTruthy();docs.push((await r.json())[0])}
 const membership=await(await request.get(`${base}/matters/${matter.id}/membership`,{headers})).json()
 await request.put(`${base}/matters/${matter.id}/membership`,{headers,data:{included_ids:[docs[0].id],expected_snapshot:membership.input_snapshot}})
 return {matter,docs,buffer}
}
async function enter(page:any){await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click();await expect(page.locator('.build-workspace')).toBeVisible();await expect(page.locator('.section-enter-active')).toHaveCount(0);await page.evaluate(()=>document.fonts.ready)}
async function capture(page:any,info:any,name:string){
 await page.screenshot({path:info.outputPath(name+'.png'),animations:'disabled'})
 if(name.includes('15A-') || name.includes('15A-settings'))return
 for(const size of [{width:1440,height:900},{width:1024,height:768}])for(const theme of ['light','dark'])for(const scale of [90,100,110,125]){
  await page.setViewportSize(size)
  await page.evaluate(({theme,scale})=>{document.documentElement.dataset.theme=theme;document.documentElement.dataset.effectiveTheme=theme;document.documentElement.style.setProperty('--ui-scale',String(scale/100));document.documentElement.style.zoom=scale+'%'},{theme,scale})
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBeTruthy()
  const dialog=page.locator('.generated-review[open]')
  if(await dialog.count()){
   const paper=dialog.locator('.pdf-paper').first();await expect(paper).toBeVisible()
   const box=(await paper.boundingBox())!;expect(box.width).toBeGreaterThan(200)
   const area=(await dialog.locator('.shared-reader-viewport').boundingBox())!;expect(area.height).toBeGreaterThan(180)
  }
  await page.screenshot({path:info.outputPath(`${name}-${size.width}-${theme}-${scale}.png`),animations:'disabled'})
 }
 await page.setViewportSize({width:1440,height:900})
 await page.evaluate(()=>{document.documentElement.dataset.theme='light';document.documentElement.dataset.effectiveTheme='light';document.documentElement.style.setProperty('--ui-scale','1');document.documentElement.style.zoom='100%'})
}
test('B01B real failure/retry, persisted output, generated review and matching export',async({page,request},info)=>{
 test.setTimeout(240_000)
 const {matter,docs,buffer}=await fixture(request)
 await page.setViewportSize({width:1440,height:900});await page.goto('/');await expect(page.locator('.pdf-paper')).toBeVisible();await enter(page)
 const workspace=page.locator('.build-workspace')
 await expect(workspace).toContainText('1 included documents · 1 reference-only')
 for(const [selector,x,y,width,height] of [['.build-settings-card',208,272,882,518],['.build-summary',1106,272,310,518]] as const){const box=(await page.locator(selector).boundingBox())!;expect(Math.round(box.x)).toBe(x);expect(Math.round(box.y)).toBe(y);expect(Math.round(box.width)).toBe(width);expect(Math.round(box.height)).toBe(height)}
 await capture(page,info,'15A-settings-1440-light')
 await workspace.getByRole('button',{name:'Plan volumes',exact:true}).click()
 const tool=page.getByRole('dialog',{name:'Plan volumes',exact:true})
 await tool.getByLabel('Maximum pages per volume').fill('2')
 await tool.getByRole('button',{name:'Save bundle settings',exact:true}).click();await expect(tool).toContainText('Bundle settings saved')
 await page.keyboard.press('Escape');await expect(workspace.getByRole('button',{name:'Plan volumes',exact:true})).toBeFocused()
 await workspace.getByRole('button',{name:'Build bundle',exact:true}).click()
 await expect(workspace).toContainText('Build failed');await capture(page,info,'16A-failure')
 await workspace.getByRole('button',{name:'Build settings',exact:true}).click()
 await workspace.getByRole('button',{name:'Plan volumes',exact:true}).click();await tool.getByLabel('Maximum pages per volume').fill('300');await tool.getByRole('button',{name:'Save bundle settings',exact:true}).click();await expect(tool).toContainText('Bundle settings saved');await page.keyboard.press('Escape')
 // Delay delivery of a real job result so the actual queued/running UI is observable.
 let release!:()=>void;const held=new Promise<void>(r=>release=r)
 await page.route('**/jobs/*',async route=>{if(route.request().method()==='GET'){const response=await route.fetch();await held;await route.fulfill({response})}else await route.continue()})
 await workspace.getByRole('button',{name:'Build bundle',exact:true}).click();await expect(workspace).toContainText('Building bundle');await capture(page,info,'15B-progress');await workspace.getByRole('button',{name:'Cancel build',exact:true}).click();await expect(workspace).toContainText('Cancellation requested');await expect(workspace).not.toContainText('Build cancelled');release()
 await expect(workspace.getByRole('heading',{name:'Bundle ready for review'})).toBeVisible()
 await page.unroute('**/jobs/*')
 const builds=await(await request.get(`${base}/matters/${matter.id}/exports`,{headers})).json();const build=builds[0]
 await expect(workspace).toContainText(build.id);await capture(page,info,'17A-complete')
 await workspace.getByRole('button',{name:'Review bundle',exact:true}).click();const review=page.getByRole('dialog',{name:'Review generated PDF'})
 await expect(review.locator('.pdf-paper').first()).toBeVisible();await expect(review).toContainText(build.id.slice(0,8));await expect(review.getByRole('button',{name:'Output checks',exact:true})).toBeEnabled();await capture(page,info,'18A-review')
 await review.getByRole('button',{name:'Export files',exact:true}).click();const exp=page.getByRole('dialog',{name:'Export bundle files',exact:true})
 await exp.getByLabel('File name').selectOption('combined.pdf');await capture(page,info,'18B-export')
 const exportPath=`**/matters/${matter.id}/exports/${build.id}/combined.pdf`
 await page.route(exportPath,route=>route.fulfill({status:503,contentType:'application/json',body:JSON.stringify({detail:'Synthetic export temporarily unavailable'})}))
 await exp.getByRole('button',{name:'Export files',exact:true}).click();await expect(exp).toContainText('Synthetic export temporarily unavailable');await expect(exp.getByRole('button',{name:'Retry export',exact:true})).toBeEnabled();await page.unroute(exportPath)
 const downloaded=page.waitForEvent('download');await exp.getByRole('button',{name:'Retry export',exact:true}).click();const download=await downloaded
 const bytes=await readFile((await download.path())!);const source=await(await request.get(`${base}/matters/${matter.id}/exports/${build.id}/combined.pdf`,{headers})).body()
 expect(hash(bytes)).toBe(hash(source));expect(hash(bytes)).toBe(build.files.find((f:any)=>f.name==='combined.pdf').sha256)
 await expect(exp).toContainText('Download requested');await exp.getByRole('button',{name:'Close',exact:true}).click();await expect(review.getByRole('button',{name:'Export files',exact:true})).toBeFocused();await review.getByRole('button',{name:'Close review',exact:true}).click()
 await page.reload();await enter(page);await expect(workspace).toContainText(build.id)
 const original=await(await request.get(`${base}/documents/${docs[0].id}/original`,{headers})).body();expect(hash(original)).toBe(hash(buffer))
 await request.patch(`${base}/documents/${docs[0].id}`,{headers,data:{title:'Petition — updated working title'}})
 await page.reload();await enter(page);await expect(workspace.getByRole('heading',{name:'Bundle is out of date'})).toBeVisible();await capture(page,info,'19A-stale')
 await workspace.getByRole('button',{name:'Review changes',exact:true}).click();await expect(workspace).toContainText('title changed');await capture(page,info,'19B-changes')
 await workspace.getByRole('button',{name:'Open earlier bundle',exact:true}).click();await expect(review.locator('.pdf-paper').first()).toBeVisible();await expect(review).toContainText('Earlier output remains unchanged');await capture(page,info,'18C-stale-review');await review.getByRole('button',{name:'Close review',exact:true}).click()
 await page.locator('.matter-rail').getByRole('button',{name:'Storage',exact:true}).click();await expect(workspace).toContainText(build.id);await capture(page,info,'20A-storage')
 expect(hash(await(await request.get(`${base}/matters/${matter.id}/exports/${build.id}/combined.pdf`,{headers})).body())).toBe(hash(source))
})

test('B01B settings geometry, keyboard, theme and scale matrix',async({page,request},info)=>{
 await fixture(request);await page.goto('/');await expect(page.locator('.pdf-paper')).toBeVisible();await enter(page)
 for(const size of [{width:1440,height:900},{width:1024,height:768}])for(const theme of ['light','dark'])for(const scale of [90,100,110,125]){
  await page.setViewportSize(size)
  await page.evaluate(({theme,scale})=>{document.documentElement.dataset.theme=theme;document.documentElement.dataset.effectiveTheme=theme;document.documentElement.style.setProperty('--ui-scale',String(scale/100));document.documentElement.style.zoom=scale+'%'},{theme,scale})
  await expect(page.locator('.build-settings-card')).toBeVisible()
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBeTruthy()
  await page.locator('.build-summary').getByRole('button',{name:'Plan volumes',exact:true}).click();await expect(page.getByRole('dialog',{name:'Plan volumes',exact:true})).toBeVisible();await page.keyboard.press('Escape');await expect(page.locator('.build-summary').getByRole('button',{name:'Plan volumes',exact:true})).toBeFocused()
  await page.locator('.workspace-page').evaluate((el:HTMLElement)=>el.scrollTop=0)
  await capture(page,info,`15A-${size.width}-${theme}-${scale}`)
 }
})


test('B01B late build response cannot replace another matter workspace',async({page,request})=>{
 const second=await fixture(request)
 const name=`Synthetic late-result destination ${second.matter.id.slice(0,8)}`
 await request.patch(`${base}/matters/${second.matter.id}`,{headers,data:{name}})
 const first=await fixture(request)
 await page.goto('/');await expect(page.locator('.pdf-paper')).toBeVisible();await enter(page)
 let release!:()=>void;const held=new Promise<void>(r=>release=r)
 let fetched=0
 await page.route('**/jobs/*',async route=>{
  if(route.request().method()!=='GET'){await route.continue();return}
  const response=await route.fetch();fetched++;await held;await route.fulfill({response})
 })
 try{
  await page.locator('.build-workspace').getByRole('button',{name:'Build bundle',exact:true}).click()
  await expect.poll(()=>fetched).toBeGreaterThan(0)
  const builds=await(await request.get(`${base}/matters/${first.matter.id}/exports`,{headers})).json()
  expect(builds).toHaveLength(1)
  await openWorkspaceNavigation(page)
  await page.locator('.matter-link').filter({hasText:name}).click()
  await enter(page)
  await expect(page.locator('.build-workspace')).toHaveAttribute('data-screen','settings')
  const lateResponse=page.waitForResponse(response=>response.url().includes('/jobs/')&&response.request().method()==='GET')
  release();await lateResponse;await page.unroute('**/jobs/*')
  await expect(page.locator('.build-workspace')).toHaveAttribute('data-screen','settings')
  await expect(page.locator('.build-workspace')).not.toContainText(builds[0].id)
  await expect(page.locator('.toast')).not.toContainText('Filing bundle generated successfully')
  expect(await(await request.get(`${base}/matters/${second.matter.id}/exports`,{headers})).json()).toEqual([])
 }finally{release()}
})
