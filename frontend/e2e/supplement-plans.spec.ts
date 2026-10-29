import { enterLatestMatter } from './navigation'
import {test,expect,type Page} from '@playwright/test'
import {readFile,mkdir,writeFile} from 'node:fs/promises'
import {resolve} from 'node:path'
import {openWorkspaceNavigation,closeWorkspaceNavigation} from './navigation'
const base=`http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT||8768}`,headers={Authorization:'Bearer e2e-isolated-token'}
const evidence=resolve(import.meta.dirname,'../../docs/ui-evidence/b04a')
const b04bEvidence=resolve(import.meta.dirname,'../../docs/ui-evidence/b04b')
async function seed(request:any){
 const m=await(await request.post(`${base}/matters`,{headers,data:{name:'B04A Synthetic supplemental plan',court_profile:'general-demo'}})).json(),root=`${base}/matters/${m.id}`
 const buffer=await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'));const docs=[]
 for(const name of ['Earlier evidence.pdf','Inserted material.pdf','Further evidence.pdf']){
  docs.push(...await(await request.post(root+'/documents',{headers,multipart:{files:{name,mimeType:'application/pdf',buffer}}})).json())
  if(docs.length===1){const member=await(await request.get(root+'/membership',{headers})).json();await request.put(root+'/membership',{headers,data:{included_ids:[docs[0].id],expected_snapshot:member.input_snapshot}});const j=await(await request.post(root+'/build',{headers})).json();await expect.poll(async()=> (await(await request.get(`${base}/jobs/${j.id}`,{headers})).json()).status).toBe('completed')}
 }
 await request.post(`${base}/documents/${docs[0].id}/pages/reorder`,{headers,data:{order:[2,0,1]}})
 const member=await(await request.get(root+'/membership',{headers})).json();await request.put(root+'/membership',{headers,data:{included_ids:docs.map(d=>d.id),expected_snapshot:member.input_snapshot}})
 const prior=(await(await request.get(root+'/exports',{headers})).json())[0],catalog=await(await request.get(root+'/supplements',{headers})).json()
 return {m,root,docs,prior,catalog}
}
async function enter(page:Page){await page.setViewportSize({width:1440,height:900});await page.goto('/');await enterLatestMatter(page);await expect(page.locator('.pdf-paper')).toBeVisible();await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click();await expect(page.locator('.build-workspace')).toBeVisible();await expect(page.locator('.section-enter-active')).toHaveCount(0);await page.evaluate(()=>document.fonts.ready)}
async function appearance(page:Page,theme:string,scale:number){await openWorkspaceNavigation(page);await page.getByLabel('Theme',{exact:true}).selectOption(theme);await page.getByLabel('Interface size',{exact:true}).selectOption(String(scale));await closeWorkspaceNavigation(page)}
async function seedPreservedInsert(request:any){
 const m=await(await request.post(`${base}/matters`,{headers,data:{name:'B04B Synthetic preserved insert',court_profile:'general-demo'}})).json(),root=`${base}/matters/${m.id}`
 const buffer=await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'))
 const target=(await(await request.post(root+'/documents',{headers,multipart:{files:{name:'Earlier issued material.pdf',mimeType:'application/pdf',buffer}}})).json())[0]
 const membership=await(await request.get(root+'/membership',{headers})).json();await request.put(root+'/membership',{headers,data:{included_ids:[target.id],expected_snapshot:membership.input_snapshot}})
 const primary=await(await request.post(root+'/build',{headers})).json();await expect.poll(async()=> (await(await request.get(`${base}/jobs/${primary.id}`,{headers})).json()).status).toBe('completed')
 const prior=(await(await request.get(root+'/exports',{headers})).json())[0]
 const source=(await(await request.post(root+'/documents',{headers,multipart:{files:{name:'Eight inserted pages.pdf',mimeType:'application/pdf',buffer}}})).json())[0]
 const inserted=await request.post(`${base}/documents/${target.id}/pages/insert`,{headers,data:{source_document_id:source.id,source_pages:[0,1,2,0,1,2,0,1],before_index:2,expected_revision:1,expected_source_revision:1}});expect(inserted.ok()).toBeTruthy()
 const catalog=await(await request.get(root+'/supplements',{headers})).json(),current=catalog.documents.find((d:any)=>d.id===target.id),exactIds=current.pages.slice(2,10).map((p:any)=>p.id)
 const payload={...catalog.defaults,baseline_build_id:prior.id,title:'Eight-page preserved insert',selected_page_ids:exactIds,expected_snapshot:catalog.input_snapshot,insertion_policy:'preserve_printed_labels',policy_reviewed:true}
 const saved=await request.post(root+'/supplements',{headers,data:payload});expect(saved.ok()).toBeTruthy()
 return {root,prior,exactIds,plan:await saved.json()}
}
test('B04A eight stable pages, caller Apply Cancel, saved reopen, real map and appearance matrix',async({page,request})=>{
 await mkdir(evidence,{recursive:true});const {root,catalog}=await seed(request);await enter(page)
 await expect(page.locator('.build-workspace')).toHaveAttribute('data-screen','stale');await page.screenshot({path:resolve(evidence,'19A-1440-light.png')})
 await page.getByRole('button',{name:'Review changes',exact:true}).click();await expect(page.locator('.revision-comparison')).toContainText('working PDF changed');await page.screenshot({path:resolve(evidence,'19B-1440-light.png')});await page.getByRole('button',{name:'Choose update policy'}).click()
 await page.getByRole('button',{name:'Create supplement',exact:true}).click();const w=page.locator('.supplement-workspace');await expect(w).toContainText('No source pages selected');await expect(w.getByRole('button',{name:'Prepare supplement'})).toBeDisabled()
 await page.getByRole('button',{name:'Supplement membership',exact:true}).click();const boxes=w.locator('.supplement-page-options input');for(let i=0;i<8;i++)await boxes.nth(i).check();await w.getByRole('button',{name:'Apply selection'}).click();await expect(w.getByRole('button',{name:'Choose other material'})).toBeFocused();await expect(w.locator('.supplement-summary')).toContainText('10')
 await w.getByRole('button',{name:'Choose other material'}).click();await boxes.nth(0).uncheck();await w.getByRole('button',{name:'Cancel selection'}).click();await expect(w.locator('.supplement-summary dd').first()).toHaveText('8')
 await w.locator('.supplement-field').click();await w.getByLabel('Prefix',{exact:true}).fill('S2 · ');await w.getByRole('button',{name:'Cancel numbering'}).click();await expect(w.locator('.supplement-field')).toContainText('S1');await expect(w.locator('.supplement-field')).toBeFocused()
 await w.locator('.supplement-field').click();await w.getByLabel('Prefix',{exact:true}).fill('SUP · ');await w.getByRole('button',{name:'Apply numbering'}).click();await expect(w.locator('.supplement-field')).toContainText('SUP')
 await w.getByRole('button',{name:'Save plan',exact:true}).click();await expect(w.locator('.supplement-footer')).toContainText('v1')
 const saved=(await(await request.get(root+'/supplements',{headers})).json()).plans[0];const exactIds=catalog.documents.flatMap((d:any)=>d.pages.map((p:any)=>p.id)).slice(0,8);expect(saved.selected_page_ids).toEqual(exactIds)
 const matrix=[]
 for(const size of [{width:1440,height:900},{width:1024,height:768}])for(const theme of ['light','dark'])for(const scale of [90,100,110,125]){
  await page.setViewportSize(size);await appearance(page,theme,scale);await page.locator('.workspace-page').evaluate(el=>el.scrollTop=0)
  await expect(w.locator('.supplement-material')).toBeVisible();expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBe(true)
  if(size.width===1440&&scale===100){const a=(await w.locator('.supplement-material').boundingBox())!,b=(await w.locator('.supplement-summary').boundingBox())!;expect(Math.round(a.x)).toBe(208);expect(Math.round(a.y)).toBe(236);expect(Math.round(a.width)).toBe(858);expect(Math.round(a.height)).toBe(538);expect(Math.round(b.width)).toBe(332);expect(Math.round((await w.getByLabel('Supplement title',{exact:true}).boundingBox())!.height)).toBe(36)}
  await page.screenshot({path:resolve(evidence,`BU08-${size.width}-${theme}-${scale}.png`),animations:'disabled'})
  const trigger=w.getByRole('button',{name:'Choose other material'});await trigger.focus();await page.keyboard.press('Enter');await expect(w).toHaveAttribute('data-mode','membership');await page.keyboard.press('Escape');await expect(trigger).toBeFocused();await page.keyboard.press('Tab');await expect(w.getByLabel('Supplement title',{exact:true})).toBeFocused();await page.keyboard.press('Shift+Tab');await expect(trigger).toBeFocused()
  await w.getByRole('button',{name:'Prepare supplement'}).focus();await expect(w.getByRole('button',{name:'Prepare supplement'})).toBeInViewport();matrix.push({...size,theme,scale})
 }
 await writeFile(resolve(evidence,'matrix.json'),JSON.stringify(matrix,null,2))
 await w.getByRole('button',{name:'Prepare supplement'}).click();await expect(w).toHaveAttribute('data-mode','map');await expect(w.locator('.page-map')).toContainText('10 physical pages');await page.screenshot({path:resolve(evidence,'11C-supplement-map.png')});await w.getByRole('button',{name:'Back to supplement'}).click();await expect(w.getByRole('button',{name:'Prepare supplement'})).toBeFocused()
 await page.reload();await enterLatestMatter(page);await expect(page.locator('.pdf-paper')).toBeVisible();await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click();await expect(w).toBeVisible();await expect(w.locator('.supplement-summary dd').first()).toHaveText('8');expect((await(await request.get(root+'/supplements',{headers})).json()).plans[0].selected_page_ids).toEqual(exactIds)
})
test('B04A failed saves, stale identities, delayed preview and explicit discard retain selection',async({page,request})=>{
 const {root,docs}=await seed(request);await enter(page);await page.getByRole('button',{name:'Create supplement',exact:true}).click();const w=page.locator('.supplement-workspace');await w.getByRole('button',{name:'Choose other material'}).click();await w.locator('.supplement-page-options input').first().check();await w.getByRole('button',{name:'Apply selection'}).click()
 await page.route('**/supplements',r=>r.request().method()==='POST'?r.fulfill({status:503,contentType:'application/json',body:'{"detail":"Synthetic save failure"}'}):r.continue());await w.getByRole('button',{name:'Save plan',exact:true}).click();await expect(w.getByRole('alert')).toContainText('Synthetic save failure');await expect(w.locator('.supplement-summary dd').first()).toHaveText('1');await page.unroute('**/supplements');await w.getByRole('button',{name:'Save plan',exact:true}).click();await expect(w.locator('.supplement-footer')).toContainText('v1')
 let release!:()=>void;const held=new Promise<void>(r=>release=r);let observed=false,completed=false
 await page.route('**/supplement-preview',async r=>{if(!observed){observed=true;const response=await r.fetch();await held;await r.fulfill({response});completed=true}else await r.continue()})
 await w.getByLabel('Supplement title',{exact:true}).fill('Delayed title');await expect.poll(()=>observed).toBe(true);await w.getByRole('button',{name:'Choose other material'}).click();await w.locator('.supplement-page-options input').nth(1).check();await w.getByRole('button',{name:'Apply selection'}).click();await expect(w.locator('.supplement-summary dd').nth(2)).toHaveText('4');release();await expect.poll(()=>completed).toBe(true);await page.unroute('**/supplement-preview');await expect(w.locator('.supplement-summary dd').nth(2)).toHaveText('4')
 await request.post(`${base}/documents/${docs[0].id}/pages/delete`,{headers,data:{page_index:0}});await w.getByRole('button',{name:'Save plan',exact:true}).click();await expect(w.getByRole('alert').first()).toContainText('Matter inputs changed');await w.getByRole('button',{name:'Reload material; retain selection'}).first().click();await expect(w.locator('.supplement-selection')).toContainText('missing or stale');await expect(w.getByRole('button',{name:'Prepare supplement'})).toBeDisabled()
 await w.getByRole('button',{name:'Cancel',exact:true}).click();await expect(w.getByRole('dialog')).toBeVisible();await w.getByRole('button',{name:'Keep editing'}).click();await w.getByRole('button',{name:'Cancel',exact:true}).click();await w.getByRole('button',{name:'Discard changes',exact:true}).click();await expect(w).toBeHidden();const plans=(await(await request.get(root+'/supplements',{headers})).json()).plans;expect(plans[0].selected_page_ids).toHaveLength(1)
})
test('B04A stale choices and revision comparison adapt to both themes and all interface scales',async({page,request})=>{
 await mkdir(evidence,{recursive:true});await seed(request);await enter(page)
 for(const size of [{width:1440,height:900},{width:1024,height:768}])for(const theme of ['light','dark'])for(const scale of [90,100,110,125]){
  await page.setViewportSize(size);await appearance(page,theme,scale);const w=page.locator('.build-workspace');await page.locator('.workspace-page').evaluate(el=>el.scrollTop=0)
  await expect(w).toHaveAttribute('data-screen','stale');expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBe(true);await page.screenshot({path:resolve(evidence,`19A-${size.width}-${theme}-${scale}.png`),animations:'disabled'})
  const changes=w.getByRole('button',{name:'Review changes',exact:true});await changes.focus();await page.keyboard.press('Enter');await expect(w.locator('.revision-comparison')).toContainText('working PDF changed');await page.locator('.workspace-page').evaluate(el=>el.scrollTop=0);expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBe(true);await page.screenshot({path:resolve(evidence,`19B-${size.width}-${theme}-${scale}.png`),animations:'disabled'})
  await w.locator('.revision-comparison td button').nth(1).focus();await page.keyboard.press('Space');await expect(w.locator('.revision-detail')).toContainText('Inserted material')
  const back=w.getByRole('button',{name:'Choose update policy',exact:true});await back.focus();await expect(back).toBeInViewport();await page.keyboard.press('Enter');await expect(w.getByRole('button',{name:'Review changes',exact:true})).toBeFocused()
 }
})

test('B04A navigation Save Discard Keep editing uses the supplemental draft',async({page,request})=>{
 const {root}=await seed(request);await enter(page);await page.getByRole('button',{name:'Create supplement',exact:true}).click();const w=page.locator('.supplement-workspace');await w.getByLabel('Supplement title',{exact:true}).fill('Native-style saved plan');await page.locator('.matter-rail').getByRole('button',{name:'Documents',exact:true}).click();const guard=page.getByRole('alertdialog',{name:'Unsaved work in this tab'});await expect(guard).toBeVisible();await guard.getByRole('button',{name:'Keep editing'}).click();await expect(w).toBeVisible();await page.locator('.matter-rail').getByRole('button',{name:'Documents',exact:true}).click();await guard.getByRole('button',{name:'Save changes',exact:true}).click();await expect(w).toBeHidden();expect((await(await request.get(root+'/supplements',{headers})).json()).plans[0].title).toBe('Native-style saved plan');await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click();await expect(w.getByLabel('Supplement title',{exact:true})).toHaveValue('Native-style saved plan');await w.getByLabel('Supplement title',{exact:true}).fill('Discard this');await w.getByRole('button',{name:'Cancel',exact:true}).click();await page.keyboard.press('Escape');await expect(w.getByRole('dialog')).toBeHidden();await w.getByRole('button',{name:'Cancel',exact:true}).click();await w.getByRole('button',{name:'Discard changes',exact:true}).click();expect((await(await request.get(root+'/supplements',{headers})).json()).plans[0].title).toBe('Native-style saved plan')
})

test('B04B BU09 preserved-label policy renders from reconciled data and builds a checked artifact',async({page,request})=>{
 await mkdir(b04bEvidence,{recursive:true});const {root,prior,exactIds}=await seedPreservedInsert(request);await enter(page)
 const previewButton=page.getByRole('button',{name:'Preview inserts',exact:true});await expect(previewButton).toBeEnabled();await previewButton.click()
 const w=page.locator('.supplement-workspace');await expect(w).toHaveAttribute('data-mode','policy');await expect(w.getByRole('heading',{name:'Preserve existing references'})).toBeVisible();await expect(w.locator('.supplement-insertion-card')).toContainText('10 planned physical pages');await expect(w.locator('.supplement-insertion-card tbody tr')).toHaveCount(4)
 const strip=(await w.locator('.supplement-policy-strip').boundingBox())!,card=(await w.locator('.supplement-insertion-card').boundingBox())!,actions=(await w.locator('.supplement-policy-actions').boundingBox())!
 expect({x:Math.round(strip.x),y:Math.round(strip.y),width:Math.round(strip.width),height:Math.round(strip.height)}).toEqual({x:208,y:214,width:1208,height:42})
 expect({x:Math.round(card.x),y:Math.round(card.y),width:Math.round(card.width),height:Math.round(card.height)}).toEqual({x:208,y:278,width:1208,height:420})
 expect({x:Math.round(actions.x),y:Math.round(actions.y),width:Math.round(actions.width)}).toEqual({x:208,y:718,width:1208})
 await page.screenshot({path:resolve(b04bEvidence,'BU09-1440-light-100.png'),animations:'disabled'})
 for(const size of [{width:1024,height:768}])for(const theme of ['light','dark'])for(const scale of [90,100,110,125]){
  await page.setViewportSize(size);await appearance(page,theme,scale);await expect(w.locator('.supplement-policy-strip')).toBeVisible();expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBe(true);await page.screenshot({path:resolve(b04bEvidence,`BU09-${size.width}-${theme}-${scale}.png`),animations:'disabled'})
 }
 await page.setViewportSize({width:1440,height:900});await appearance(page,'light',100);await w.getByRole('button',{name:'Use insert numbering'}).focus();await page.keyboard.press('Enter');await expect(w).toHaveAttribute('data-mode','plan');await expect(w.getByRole('button',{name:'Build supplement'})).toBeEnabled();await w.getByRole('button',{name:'Build supplement'}).click()
 await expect(w.getByRole('heading',{name:'Distinct supplemental artifact generated'})).toBeVisible({timeout:60_000});await expect(w).toContainText(`baseline ${prior.id}`);await w.getByRole('button',{name:'Output checks'}).click();const checks=page.getByRole('dialog',{name:/Output checks/});await expect(checks).toBeVisible();await checks.getByRole('button',{name:'Run output checks'}).click();await expect(checks.getByText('Consistent',{exact:true})).toBeVisible();await checks.getByRole('button',{name:'Done'}).click()
 const artifacts=await(await request.get(root+'/exports',{headers})).json(),supplement=artifacts.find((a:any)=>a.artifact_kind==='supplement');expect(supplement.bundle_membership.selected_page_ids).toEqual(exactIds);expect(supplement.insertion_policy).toBe('preserve_printed_labels');expect(supplement.baseline_build_id).toBe(prior.id)
 await w.getByRole('button',{name:'Review supplement'}).click();await expect(page.locator('.pdf-paper')).toBeVisible();await expect(page.getByText('original source pages · Generated PDF · Read only')).toBeVisible()
})
