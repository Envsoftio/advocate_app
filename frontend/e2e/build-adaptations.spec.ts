import {test,expect,type Page} from '@playwright/test'
import {readFile,mkdir,writeFile} from 'node:fs/promises'
import {resolve} from 'node:path'
import {openWorkspaceNavigation,closeWorkspaceNavigation} from './navigation'
const base=`http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT||8768}`,headers={Authorization:'Bearer e2e-isolated-token'}
const evidence=resolve(import.meta.dirname,'../../docs/ui-evidence/b03c')
async function seed(request:any){
 const matter=await(await request.post(`${base}/matters`,{headers,data:{name:'B03C Synthetic build settings',court_profile:'general-demo'}})).json()
 const buffer=await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'))
 const docs=[]
 for(const name of ['Synthetic petition.pdf','Reference notes.pdf'])docs.push(...await(await request.post(`${base}/matters/${matter.id}/documents`,{headers,multipart:{files:{name,mimeType:'application/pdf',buffer}}})).json())
 const m=await(await request.get(`${base}/matters/${matter.id}/membership`,{headers})).json()
 expect((await request.put(`${base}/matters/${matter.id}/membership`,{headers,data:{included_ids:[docs[0].id],expected_snapshot:m.input_snapshot}})).ok()).toBeTruthy()
 return {matter,docs}
}
async function enter(page:Page){await page.setViewportSize({width:1440,height:900});await page.goto('/');await expect(page.locator('.pdf-paper')).toBeVisible();await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click();await expect(page.locator('.section-enter-active')).toHaveCount(0);await expect(page.locator('.build-settings-card')).toContainText('Custom bundle settings');await page.evaluate(()=>document.fonts.ready)}
async function appearance(page:Page,theme:string,scale:number){
 await openWorkspaceNavigation(page);await page.getByLabel('Theme',{exact:true}).selectOption(theme);await page.getByLabel('Interface size',{exact:true}).selectOption(String(scale));await closeWorkspaceNavigation(page)
 await expect(page.locator('html')).toHaveAttribute('data-effective-theme',theme)
}
test('B03C reference geometry and real appearance matrix with compact keyboard return',async({page,request})=>{
 await mkdir(evidence,{recursive:true});const {matter}=await seed(request);await enter(page)
 const w=page.locator('.build-workspace'),card=w.locator('.build-settings-card'),summary=w.locator('.build-summary')
 for(const [size,theme,expectedX,expectedWidth,summaryWidth,height] of [[1440,'light',208,882,310,518],[1024,'light',88,652,244,386],[1440,'dark',208,882,310,518]] as const){
  await page.setViewportSize({width:size,height:size===1440?900:768});await appearance(page,theme,100)
  const b=(await card.boundingBox())!,s=(await summary.boundingBox())!
  expect(Math.round(b.x)).toBe(expectedX);expect(Math.round(b.y)).toBe(272);expect(Math.round(b.width)).toBe(expectedWidth);expect(Math.round(s.width)).toBe(summaryWidth);expect(Math.round(b.height)).toBe(height);expect(Math.round(s.height)).toBe(height)
 }
 const before=await(await request.get(`${base}/matters/${matter.id}/bundle-settings`,{headers})).json()
 const matrix=[]
 for(const size of [{width:1440,height:900},{width:1024,height:768}])for(const theme of ['light','dark'])for(const scale of [90,100,110,125]){
  await page.setViewportSize(size);await appearance(page,theme,scale)
  await expect(card).toBeVisible();expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBe(true)
  await page.locator('.workspace-page').evaluate(el=>el.scrollTop=0)
  await page.screenshot({path:resolve(evidence,`build-${size.width}-${theme}-${scale}.png`),animations:'disabled'})
  const options=w.getByRole('button',{name:'Output options',exact:true});await options.focus();await page.keyboard.press('Enter');await expect(page.getByRole('dialog',{name:'Output options',exact:true})).toBeVisible();await page.keyboard.press('Escape');await expect(options).toBeFocused()
  await page.keyboard.press('Tab');await expect(w.getByRole('button',{name:'Plan volumes',exact:true})).toBeFocused();await page.keyboard.press('Space');await expect(page.locator('.volume-workspace')).toBeVisible();await page.keyboard.press('Escape');await expect(w.getByRole('button',{name:'Plan volumes',exact:true})).toBeFocused()
  await page.keyboard.press('Tab');await expect(w.getByRole('button',{name:'Build bundle',exact:true})).toBeFocused();await expect(w.getByRole('button',{name:'Build bundle',exact:true})).toBeInViewport();await page.keyboard.press('Shift+Tab');await expect(w.getByRole('button',{name:'Plan volumes',exact:true})).toBeFocused()
  if(size.width===1024&&scale===125)await page.screenshot({path:resolve(evidence,`keyboard-${theme}.png`)})
  expect(await page.evaluate(()=>JSON.parse(localStorage.getItem('courtbundle.appearance.v1')!))).toEqual({theme,scale})
  matrix.push({...size,theme,scale,keyboard:true})
 }
 expect(await(await request.get(`${base}/matters/${matter.id}/bundle-settings`,{headers})).json()).toEqual(before)
 await writeFile(resolve(evidence,'matrix.json'),JSON.stringify(matrix,null,2))
})
test('B03C live summaries, options persistence and shared workspace routes',async({page,request})=>{
 const {matter}=await seed(request);await enter(page);const w=page.locator('.build-workspace')
 await expect(w).toContainText('1 included documents · 1 reference-only documents');await expect(w.locator('.build-summary')).toContainText('No optional volume byte limit')
 await w.getByRole('button',{name:'Output options',exact:true}).click();const d=page.getByRole('dialog',{name:'Output options',exact:true});await d.getByLabel('Page quality',{exact:true}).selectOption('lossless');await d.getByLabel('XLSX',{exact:true}).check();await d.getByRole('button',{name:'Apply options',exact:true}).click();await expect(w).toContainText('Lossless optimization');await expect(w).toContainText('XLSX review drafts selected')
 await w.getByRole('button',{name:'Edit numbering',exact:true}).click();await expect(page.getByRole('heading',{name:'Numbering & navigation',exact:true})).toBeVisible();await page.getByRole('button',{name:'Back to build settings',exact:true}).click();await expect(w.getByRole('button',{name:'Edit numbering',exact:true})).toBeFocused()
 await w.getByRole('button',{name:'Preview',exact:true}).click();await expect(page.locator('.bundle-structure')).toBeVisible();await page.getByRole('button',{name:'Back to build settings',exact:true}).click();await expect(w.getByRole('button',{name:'Preview',exact:true})).toBeFocused()
 await w.getByRole('button',{name:'Preflight',exact:true}).click();await expect(page.locator('.preflight-workspace')).toBeVisible();await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click()
 await page.setViewportSize({width:1024,height:768});await appearance(page,'dark',110);await page.reload();await expect(page.locator('.pdf-paper')).toBeVisible();await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click();await expect(w.locator('.build-compact-note')).toContainText('Lossless optimization');await expect(w.locator('.build-compact-note')).toContainText('XLSX review drafts selected');expect((await(await request.get(`${base}/matters/${matter.id}/bundle-settings`,{headers})).json()).output_marks.office_xlsx).toBe(true)
})
test('B03C unavailable and mixed-snapshot summaries reject stale data and recover',async({page,request})=>{
 await seed(request);await page.route('**/bundle-preview',r=>r.fulfill({status:503,contentType:'application/json',body:'{"detail":"Synthetic summary unavailable"}'}));await page.setViewportSize({width:1440,height:900});await page.goto('/');await expect(page.locator('.pdf-paper')).toBeVisible();await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click();const w=page.locator('.build-workspace');await expect(w.getByRole('alert')).toContainText('Synthetic summary unavailable');await page.unroute('**/bundle-preview')
 await page.route('**/bundle-settings',async r=>{const response=await r.fetch();const data=await response.json();data.input_snapshot='a'.repeat(64);await r.fulfill({response,json:data})});await w.getByRole('button',{name:'Refresh summary'}).click();await expect(w.getByRole('alert')).toContainText('Matter changed while loading');await expect(w.locator('.build-summary')).toContainText('Saved byte policy unavailable');await page.unroute('**/bundle-settings');await w.getByRole('button',{name:'Refresh summary'}).click();await expect(w.getByRole('alert')).toBeHidden();await expect(w.locator('.build-summary')).toContainText('No optional volume byte limit')
})
