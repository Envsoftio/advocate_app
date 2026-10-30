import { test, expect } from '@playwright/test'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
const base=`http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT || 8768}`,headers={Authorization:'Bearer e2e-isolated-token'}
const evidence=resolve(import.meta.dirname,'../../docs/ui-evidence/matter-flow')
test('FLOW 01: one sidebar, real queue, processing, review and persistent collapse',async({page,request})=>{
 const errors:string[]=[];page.on('pageerror',e=>errors.push(e.message))
 for(const name of ['Sharma v. State — Synthetic','Mehta Infrastructure — Synthetic','Rao Family Settlement — Synthetic','Zenith Industries — Synthetic','Kapoor Property Appeal — Synthetic']){
  const r=await request.post(`${base}/matters`,{headers,data:{name,court_profile:'general-demo',case_number:'SYN-2026',case_type:'Synthetic demonstration'}});expect(r.ok()).toBeTruthy()
 }
 await page.setViewportSize({width:1440,height:900});await page.goto('/')
 await expect(page.getByRole('heading',{name:'Matters',exact:true})).toBeVisible()
 await page.getByRole('textbox',{name:'Search matters',exact:true}).fill('— Synthetic')
 await expect(page.locator('.matter-table tbody tr')).toHaveCount(5)
 await page.getByRole('heading',{name:'Matters',exact:true}).click()
 await expect(page.locator('.section-enter-active,.section-leave-active,.page-enter-active,.page-leave-active')).toHaveCount(0)
 await expect.poll(async()=> (await page.locator('.flow-heading').boundingBox())!.y).toBe(144)
 await expect(page.locator('.section-enter-active,.section-leave-active,.page-enter-active,.page-leave-active')).toHaveCount(0);await page.screenshot({animations:'disabled',path:`${evidence}/01A-1440-light.png`})
 await page.locator('.matter-home-card').filter({hasText:'Sharma'}).click()
 await expect(page.getByRole('region',{name:'Matter overview'})).toBeVisible()
 await expect(page.locator('.matter-rail')).toHaveCSS('width','184px')
 await expect(page.locator('.section-enter-active,.section-leave-active,.page-enter-active,.page-leave-active')).toHaveCount(0)
 await expect.poll(async()=> (await page.locator('.overview-metrics').boundingBox())!.y).toBe(226)
 await expect(page.locator('.section-enter-active,.section-leave-active,.page-enter-active,.page-leave-active')).toHaveCount(0);await page.screenshot({animations:'disabled',path:`${evidence}/04A-empty-1440-light.png`})
 await page.getByRole('button',{name:'Import documents',exact:true}).last().click()
 const petition=await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'))
 await page.getByLabel('Choose import files').setInputFiles([
  {name:'Petition.pdf',mimeType:'application/pdf',buffer:petition},
  {name:'Petition_copy.pdf',mimeType:'application/pdf',buffer:petition},
  {name:'Notes.docx',mimeType:'application/octet-stream',buffer:Buffer.from('synthetic')},
  {name:'Damaged.pdf',mimeType:'application/pdf',buffer:Buffer.from('synthetic invalid PDF')},
 ])
 await expect(page.getByText('Identical file · excluded')).toBeVisible()
 await expect(page.getByRole('button',{name:'Import 2 files',exact:true})).toBeEnabled()
 await expect(page.locator('.section-enter-active,.section-leave-active,.page-enter-active,.page-leave-active')).toHaveCount(0)
 await expect.poll(async()=> (await page.locator('.import-drop').boundingBox())!.y).toBe(226)
 await expect(page.locator('.section-enter-active,.section-leave-active,.page-enter-active,.page-leave-active')).toHaveCount(0)
 await expect.poll(async()=> (await page.locator('.import-table').boundingBox())!.y).toBe(398)
 await expect(page.locator('.section-enter-active,.section-leave-active,.page-enter-active,.page-leave-active')).toHaveCount(0);await page.screenshot({animations:'disabled',path:`${evidence}/03A-1440-light.png`})
 let release!:()=>void;const held=new Promise<void>(r=>release=r)
 await page.route('**/documents/raw',async route=>{await held;await route.continue()})
 await page.getByRole('button',{name:'Import 2 files',exact:true}).click()
 await expect(page.getByRole('heading',{name:'Importing documents'})).toBeVisible()
 await expect(page.locator('.section-enter-active,.section-leave-active,.page-enter-active,.page-leave-active')).toHaveCount(0)
 await expect.poll(async()=> (await page.locator('.import-progress').boundingBox())!.y).toBe(232)
 await expect(page.locator('.section-enter-active,.section-leave-active,.page-enter-active,.page-leave-active')).toHaveCount(0)
 await expect.poll(async()=> (await page.locator('.import-table').boundingBox())!.y).toBe(436)
 await expect(page.locator('.section-enter-active,.section-leave-active,.page-enter-active,.page-leave-active')).toHaveCount(0);await page.screenshot({animations:'disabled',path:`${evidence}/03B-1440-light.png`})
 release()
 await expect(page.getByRole('heading',{name:'Import results'})).toBeVisible()
 await expect(page.getByText('1 of 2 files imported')).toBeVisible()
 await expect(page.getByRole('columnheader',{name:'Source pages',exact:true})).toBeVisible()
 await expect(page.locator('.import-table tbody tr').filter({hasText:'Petition.pdf'}).locator('td').nth(1)).toHaveText('3')
 await expect(page.locator('.flow-status[data-status=failed]')).toContainText(/PDF|document|file/i)
 await page.getByRole('button',{name:'Continue to document review'}).click()
 await expect(page.locator('.document-navigator')).toBeVisible()
 await expect(page.locator('.preparation-preview canvas').first()).toBeVisible()
 await page.locator('.matter-rail').getByRole('button',{name:'Collapse sidebar'}).click()
 await expect(page.locator('.matter-rail')).toHaveCSS('width','64px')
 for(const name of ['Overview','Index & bookmarks','Build bundle','Storage','Documents']){
  await page.locator('.matter-rail').getByRole('button',{name,exact:true}).click()
  await expect(page.locator('.matter-rail')).toHaveCSS('width','64px')
 }
 await page.reload();await expect(page.locator('.matter-rail')).toHaveCSS('width','64px')
 await page.locator('.matter-home-card').filter({hasText:'Sharma'}).click()
 await page.locator('.matter-rail').getByRole('button',{name:'Expand sidebar'}).click()
 await expect(page.getByRole('region',{name:'Matter overview'})).toBeVisible()
 await expect(page.locator('.overview-metrics')).toContainText('1Documents')
 await expect(page.locator('.section-enter-active,.section-leave-active,.page-enter-active,.page-leave-active')).toHaveCount(0);await page.screenshot({animations:'disabled',path:`${evidence}/04A-1440-light.png`})
 for(const theme of ['dark','light'])for(const scale of ['100','125']){
  await page.getByRole('button',{name:'Workspace settings',exact:true}).click()
  await page.getByRole('combobox',{name:'Theme',exact:true}).selectOption(theme)
  await page.getByRole('combobox',{name:'Interface size'}).selectOption(scale)
  await page.getByRole('button',{name:'Close workspace navigation'}).click()
  for(const width of [1440,1024]){
   await page.setViewportSize({width,height:900});await page.locator('.matter-overview').evaluate(el=>el.scrollTop=0);await expect(page.locator('.section-enter-active,.section-leave-active,.page-enter-active,.page-leave-active')).toHaveCount(0);await page.screenshot({animations:'disabled',path:`${evidence}/04A-${width}-${theme}-${scale}.png`})
   expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true)
   await page.getByRole('button',{name:'Import documents',exact:true}).last().scrollIntoViewIfNeeded();await expect(page.getByRole('button',{name:'Import documents',exact:true}).last()).toBeInViewport()
  }
 }
 expect(errors).toEqual([])
})
test('new matter enters no-documents state; cancellation preserves completed files',async({page,request})=>{
 await page.goto('/');await page.locator('.matter-home').getByRole('button',{name:'New matter',exact:true}).first().click()
 await page.getByRole('textbox',{name:'Matter name'}).fill('Synthetic import cancellation')
 await page.getByRole('button',{name:'Create matter',exact:true}).click()
 await expect(page.getByRole('heading',{name:'Add the source documents'})).toBeVisible()
 await page.getByRole('button',{name:'Import documents',exact:true}).click()
 await expect(page.getByRole('heading',{name:'Import documents',exact:true})).toBeVisible()
 await page.getByLabel('Choose import files').setInputFiles([resolve(import.meta.dirname,'../../samples/01_petition.pdf'),resolve(import.meta.dirname,'../../samples/03_affidavit.pdf')])
 await expect(page.getByRole('button',{name:'Import 2 files'})).toBeEnabled()
 let release!:()=>void;const held=new Promise<void>(r=>release=r)
 await page.route('**/documents/raw',async route=>{await held;await route.continue()})
 await page.getByRole('button',{name:'Import 2 files'}).click();await page.getByRole('button',{name:'Cancel remaining imports'}).click();release()
 await expect(page.getByRole('heading',{name:'Import results'})).toBeVisible()
 await expect(page.locator('.flow-status[data-status=complete]')).toHaveCount(1)
 await expect(page.locator('.flow-status[data-status=cancelled]')).toHaveCount(1)
 const matters=await(await request.get(`${base}/matters`,{headers})).json();const id=matters.find((m:any)=>m.name==='Synthetic import cancellation').id
 const docs=await(await request.get(`${base}/matters/${id}/documents`,{headers})).json();expect(docs).toHaveLength(1);expect(docs[0].bundle_included).toBe(0)
 await page.getByRole('button',{name:'Continue to document review'}).click();await expect(page.locator('.document-navigator')).toContainText('01_petition')
})
