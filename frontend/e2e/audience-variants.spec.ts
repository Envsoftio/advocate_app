import {test,expect} from '@playwright/test'
import {readFile,mkdir} from 'node:fs/promises'
import {resolve} from 'node:path'
import {createHash} from 'node:crypto'
import {openWorkspaceNavigation,closeWorkspaceNavigation} from './navigation'

const base=`http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT||8768}`
const headers={Authorization:'Bearer e2e-isolated-token'}
const evidence=resolve(import.meta.dirname,'../../docs/ui-evidence/b04c')

test('B04C creates, reviews, builds and reopens an independent audience artifact',async({page,request})=>{
 await mkdir(evidence,{recursive:true})
 const matter=await(await request.post(`${base}/matters`,{headers,data:{name:'B04C Synthetic audience review',court_profile:'general-demo'}})).json()
 const root=`${base}/matters/${matter.id}`
 const buffer=await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'))
 const docs=[]
 for(const filename of ['Included source.pdf','Reference material.pdf'])docs.push((await(await request.post(root+'/documents',{headers,multipart:{files:{name:filename,mimeType:'application/pdf',buffer}}})).json())[0])
 const membership=await(await request.get(root+'/membership',{headers})).json()
 await request.put(root+'/membership',{headers,data:{included_ids:[docs[0].id],expected_snapshot:membership.input_snapshot}})
 const primary=await(await request.post(root+'/build',{headers})).json()
 await expect.poll(async()=>(await(await request.get(`${base}/jobs/${primary.id}`,{headers})).json()).status).toBe('completed')
 await page.setViewportSize({width:1440,height:900});await page.goto('/');await expect(page.locator('.pdf-paper')).toBeVisible()
 await page.locator('.matter-rail').getByRole('button',{name:'Index & bookmarks'}).click()
 await page.getByRole('button',{name:'Bundle variants'}).click()
 const w=page.locator('.variant-workspace');await expect(w).toBeVisible()
 await w.getByLabel('Variant name').fill('Internal review')
 await w.getByRole('button',{name:'Create variant'}).click()
 await expect(w.getByRole('heading',{name:'Internal review'})).toBeVisible()
 await expect(page.locator('.application-status')).toContainText('Variant membership')
 await page.locator('.application-status').getByRole('button',{name:'Variant membership'}).click()
 await expect(w.getByRole('heading',{name:'Review membership'})).toBeVisible()
 await w.getByRole('button',{name:'Cancel',exact:true}).click()
 await expect(page.locator('.application-status').getByRole('button',{name:'Variant membership'})).toBeFocused()
 const plan=(await(await request.get(root+'/variants',{headers})).json()).plans[0]
 expect(plan.included_ids).toEqual([docs[0].id])
 await w.getByRole('button',{name:'Review membership'}).click()
 await expect(w.getByRole('heading',{name:'Review membership'})).toBeVisible()
 await w.getByLabel('Include Reference material').check()
 await w.getByRole('button',{name:'Cancel',exact:true}).click()
 await expect(w.getByRole('button',{name:'Review membership'})).toBeFocused()
 await w.getByRole('button',{name:'Review membership'}).click()
 await expect(w.getByLabel('Include Reference material')).not.toBeChecked()
 await w.getByLabel('Include Reference material').check()
 await w.getByRole('button',{name:'Apply membership'}).click()
 await expect(w.getByRole('button',{name:'Review membership'})).toBeFocused()
 await expect(w).toContainText('2 included documents')
 await w.getByRole('button',{name:'Review settings'}).click()
 await w.getByLabel('Front matter').selectOption('cover_index')
 await w.getByRole('button',{name:'Apply settings'}).click()
 await expect(w.getByRole('button',{name:'Review settings'})).toBeFocused()
 const saved=(await(await request.get(root+'/variants',{headers})).json()).plans[0]
 expect(saved.version).toBe(3);expect(saved.included_ids).toEqual([docs[0].id,docs[1].id])
 await w.getByRole('button',{name:'Review differences and Preflight'}).click()
 await expect(w.getByRole('heading',{name:'Inclusion differences and Preflight'})).toBeVisible()
 await expect(w.locator('.variant-difference-grid')).toContainText('added Reference material')
 await w.getByRole('button',{name:'Build Internal review'}).click()
 await expect.poll(async()=>(await(await request.get(`${base}/jobs/${(await(await request.get(root+'/jobs',{headers})).json())[0].id}`,{headers})).json()).status,{timeout:60_000}).toBe('completed')
 await expect(w.getByRole('heading',{name:'Independent output history'})).toBeVisible()
 const exports=await(await request.get(root+'/exports',{headers})).json(),variant=exports.find((a:any)=>a.artifact_kind==='variant')
 expect(variant.variant_plan_id).toBe(plan.id);expect(variant.variant_plan_version).toBe(3)
 expect(variant.pages).toBe(docs[0].page_count+docs[1].page_count+2)
 await w.getByRole('button',{name:'Checks'}).click()
 const checks=page.getByRole('dialog',{name:/Output checks/});await expect(checks).toBeVisible()
 await checks.getByRole('button',{name:'Run output checks'}).click();await expect(checks.getByText('Consistent',{exact:true})).toBeVisible()
 await checks.getByRole('button',{name:'Done'}).click()
 await w.getByRole('button',{name:'Export / Save'}).click()
 const exportDialog=page.getByRole('dialog',{name:'Export bundle files'})
 await expect(exportDialog).toBeVisible();await exportDialog.getByLabel('File name').selectOption('combined.pdf')
 const downloadPromise=page.waitForEvent('download');await exportDialog.getByRole('button',{name:'Export files'}).click()
 const downloaded=await downloadPromise, exported=await readFile(await downloaded.path())
 expect(createHash('sha256').update(exported).digest('hex')).toBe(variant.files.find((f:any)=>f.name==='combined.pdf').sha256)
 await exportDialog.getByRole('button',{name:'Close',exact:true}).click()
 await w.getByRole('button',{name:'Review PDF'}).click();await expect(page.locator('.pdf-paper')).toBeVisible()
 await page.reload();await expect(page.locator('.pdf-paper')).toBeVisible()
 await page.locator('.matter-rail').getByRole('button',{name:'Index & bookmarks'}).click();await page.getByRole('button',{name:'Bundle variants'}).click()
 await expect(w.getByRole('heading',{name:'Internal review'})).toBeVisible()
 await w.getByRole('button',{name:'View 1 saved output'}).click();await expect(w).toContainText(variant.id.slice(0,8))
})

test('B04C navigation guard retains an unsaved variant draft and copied settings are optional',async({page,request})=>{
 const matter=await(await request.post(`${base}/matters`,{headers,data:{name:'B04C Synthetic draft guard',court_profile:'general-demo'}})).json()
 const root=`${base}/matters/${matter.id}`
 const buffer=await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'))
 const doc=(await(await request.post(root+'/documents',{headers,multipart:{files:{name:'Guard.pdf',mimeType:'application/pdf',buffer}}})).json())[0]
 const membership=await(await request.get(root+'/membership',{headers})).json()
 await request.put(root+'/membership',{headers,data:{included_ids:[doc.id],expected_snapshot:membership.input_snapshot}})
 await page.goto('/');await expect(page.locator('.pdf-paper')).toBeVisible();await page.locator('.matter-rail').getByRole('button',{name:'Index & bookmarks'}).click();await page.getByRole('button',{name:'Bundle variants'}).click()
 const w=page.locator('.variant-workspace');await w.getByLabel('Variant name').fill('Uncopied plan');await w.getByLabel('Copy membership and settings').uncheck();await w.getByRole('button',{name:'Create variant'}).click()
 const uncopied=(await(await request.get(root+'/variants',{headers})).json())
 expect(uncopied.plans[0].included_ids).toEqual([])
 expect(uncopied.plans[0].settings.output_marks).toEqual(uncopied.blank_settings.output_marks)
 await w.getByRole('button',{name:'Review membership'}).click();await w.getByLabel('Include Guard').check()
 await page.locator('.matter-rail').getByRole('button',{name:'Documents'}).click()
 const guard=page.getByRole('alertdialog',{name:'Unsaved work in this tab'});await expect(guard).toBeVisible()
 await guard.getByRole('button',{name:'Keep editing'}).click();await expect(w.getByLabel('Include Guard')).toBeChecked()
 await page.locator('.matter-rail').getByRole('button',{name:'Documents'}).click();await guard.getByRole('button',{name:'Save changes'}).click()
 await expect(w).toBeHidden();expect((await(await request.get(root+'/variants',{headers})).json()).plans[0].included_ids).toEqual([doc.id])
})

test('B04C BU11 reference composition, keyboard and responsive appearance',async({page,request})=>{
 await mkdir(evidence,{recursive:true})
 const matter=await(await request.post(`${base}/matters`,{headers,data:{name:'B04C Synthetic visual matter',court_profile:'general-demo'}})).json()
 const root=`${base}/matters/${matter.id}`
 const buffer=await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'))
 const doc=(await(await request.post(root+'/documents',{headers,multipart:{files:{name:'Audience.pdf',mimeType:'application/pdf',buffer}}})).json())[0]
 const membership=await(await request.get(root+'/membership',{headers})).json()
 await request.put(root+'/membership',{headers,data:{included_ids:[doc.id],expected_snapshot:membership.input_snapshot}})
 const catalog=await(await request.get(root+'/variants',{headers})).json()
 await request.post(root+'/variants',{headers,data:{name:'Internal review',baseline_kind:'primary',baseline_id:catalog.primary_baseline.id,included_ids:[doc.id],expected_snapshot:catalog.source_snapshot,...catalog.primary_settings}})
 await page.setViewportSize({width:1440,height:900});await page.goto('/');await expect(page.locator('.pdf-paper')).toBeVisible()
 await page.locator('.matter-rail').getByRole('button',{name:'Index & bookmarks'}).click();await page.getByRole('button',{name:'Bundle variants'}).click()
 const w=page.locator('.variant-workspace');await expect(w).toBeVisible();await page.evaluate(()=>document.fonts.ready)
 const card=(await w.locator('.variant-saved').boundingBox())!,create=(await w.locator('.variant-create').boundingBox())!
 expect({x:Math.round(card.x),y:Math.round(card.y),width:Math.round(card.width),height:Math.round(card.height)}).toEqual({x:208,y:238,width:740,height:588})
 expect({x:Math.round(create.x),y:Math.round(create.y),width:Math.round(create.width),height:Math.round(create.height)}).toEqual({x:968,y:238,width:448,height:588})
 await page.screenshot({path:resolve(evidence,'BU11-1440-light-100.png'),animations:'disabled'})
 const select=w.getByRole('button',{name:'Primary bundle',exact:true});await select.focus();await page.keyboard.press('Enter');await expect(select).toHaveAttribute('aria-pressed','true')
 for(const size of [{width:1440,height:900},{width:1024,height:768}])for(const theme of ['light','dark'])for(const scale of [90,100,110,125]){
  await page.setViewportSize(size);await openWorkspaceNavigation(page);await page.getByLabel('Theme',{exact:true}).selectOption(theme);await page.getByLabel('Interface size',{exact:true}).selectOption(String(scale));await closeWorkspaceNavigation(page)
  await page.locator('.workspace-page').evaluate(el=>el.scrollTop=0)
  await expect(w.locator('.variant-create-button')).toBeVisible()
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBe(true)
  await page.screenshot({path:resolve(evidence,`BU11-${size.width}-${theme}-${scale}.png`),animations:'disabled'})
 }
})
