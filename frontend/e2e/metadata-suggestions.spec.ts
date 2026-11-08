import { enterLatestMatter } from './navigation'
import {test,expect} from '@playwright/test'
import {readFile} from 'node:fs/promises'
import {resolve} from 'node:path'
const base=`http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT||8768}`,headers={Authorization:'Bearer e2e-isolated-token'}
async function seed(request:any){const matter=await(await request.post(`${base}/matters`,{headers,data:{name:'Synthetic metadata review',court_profile:'general-demo'}})).json();
 const buffer=await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'));
 const docs=[];for(const name of ['Order_12_Jan_2026.pdf','Annexure_A_Agreement.pdf','Written_Submissions_03-04-2026.pdf']){const r=await request.post(`${base}/matters/${matter.id}/documents?included=true`,{headers,multipart:{files:{name,mimeType:'application/pdf',buffer}}});
 expect(r.ok()).toBeTruthy();docs.push((await r.json())[0])}return {matter,docs}}
async function enter(page:any,expectedCount=3){page.on('pageerror',(e:Error)=>{throw e});
 await page.setViewportSize({width:1440,height:900});
 await page.goto('/');await enterLatestMatter(page);await page.getByRole('button',{name:'Collapse sidebar',exact:true}).click();
 await expect(page.locator('.pdf-paper')).toBeVisible();
 await page.locator('.matter-rail').getByRole('button',{name:'Documents',exact:true}).click();
 await page.getByRole('button',{name:'Sort',exact:true}).click();
 await expect(page.locator('.batch-table tbody tr')).toHaveCount(expectedCount);
 await page.evaluate(()=>document.fonts.ready)}
async function suggestions(page:any){await page.locator('.batch-tools-menu>summary').click();await page.getByRole('button',{name:'Review title & dates',exact:true}).click();
 await expect(page.locator('.metadata-source-page img')).toBeVisible()}
async function matrix(page:any,info:any,name:string){for(const size of [{width:1440,height:900},{width:1024,height:768}])for(const theme of ['light','dark'])for(const scale of [90,100,110,125]){await page.setViewportSize(size);
 await page.evaluate(({theme,scale})=>{document.documentElement.dataset.theme=theme;document.documentElement.dataset.effectiveTheme=theme;document.documentElement.style.setProperty('--ui-scale',String(scale/100));(document.querySelector('.app-shell') as HTMLElement).style.setProperty('--documents-interface-scale',String(scale/100));document.documentElement.style.zoom=scale+'%'},{theme,scale});
 expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBe(true);
 const action=page.getByRole('button',{name:name==='06A'?'Save changes':/Accept \d+ suggestions/,exact:name==='06A'});
 await expect(action).toBeInViewport();
 await page.screenshot({path:info.outputPath(`${name}-${size.width}-${theme}-${scale}.png`),animations:'disabled'})}await page.setViewportSize({width:1440,height:900});
 await page.evaluate(()=>{document.documentElement.dataset.theme='light';document.documentElement.dataset.effectiveTheme='light';document.documentElement.style.setProperty('--ui-scale','1');(document.querySelector('.app-shell') as HTMLElement).style.setProperty('--documents-interface-scale','1');document.documentElement.style.zoom='100%'})}
test('Batch review filters, resizes columns and dismisses menus outside',async({page,request})=>{
 const {docs}=await seed(request)
 await enter(page)
 const register=page.getByRole('region',{name:'Editable document table'})
 await register.getByLabel(`Select ${docs[0].title}`).check()
 await register.getByRole('searchbox',{name:'Filter rows'}).fill('Annexure_A')
 await expect(register.locator('.batch-table tbody tr')).toHaveCount(1)
 await expect(register.getByText('1 selected',{exact:true})).toHaveAttribute('title','0 visible in this filter')
 await register.getByRole('button',{name:'Filters'}).click()
 await register.getByRole('combobox',{name:'Filter by Part'}).selectOption('2')
 await expect(register.locator('.batch-table tbody tr')).toHaveCount(0)
 await register.getByRole('combobox',{name:'Filter by Part'}).selectOption('1')
 await expect(register.locator('.batch-table tbody tr')).toHaveCount(1)
 await register.getByRole('combobox',{name:'Filter by type'}).selectOption('affidavit')
 await expect(register.locator('.batch-table tbody tr')).toHaveCount(0)
 await register.getByRole('combobox',{name:'Filter by type'}).selectOption('')
 await register.getByRole('combobox',{name:'Filter by status'}).selectOption('needs_ocr')
 await expect(register.locator('.batch-table tbody tr')).toHaveCount(0)
 await register.getByRole('combobox',{name:'Filter by status'}).selectOption('needs_review')
 await expect(register.locator('.batch-table tbody tr')).toHaveCount(1)
 await register.getByRole('button',{name:'Clear filters'}).click()
 await expect(register.locator('.batch-table tbody tr')).toHaveCount(3)
 await register.getByRole('button',{name:'Filters'}).click()
 await register.getByRole('heading',{name:'Batch review'}).click()
 await expect(register.locator('#batch-filter-panel')).toHaveCount(0)
 await register.getByRole('button',{name:'Columns'}).click()
 await expect(register.locator('#batch-columns-panel')).toBeVisible()
 await register.getByRole('heading',{name:'Batch review'}).click()
 await expect(register.locator('#batch-columns-panel')).toHaveCount(0)
 const titleColumn=register.locator('.batch-table th').filter({hasText:'Title'})
 const before=(await titleColumn.boundingBox())!.width
 const handle=register.getByRole('button',{name:'Resize Title column'})
 const bounds=(await handle.boundingBox())!
 await page.mouse.move(bounds.x+bounds.width/2,bounds.y+bounds.height/2)
 await page.mouse.down();await page.mouse.move(bounds.x+bounds.width/2+90,bounds.y+bounds.height/2,{steps:5});await page.mouse.up()
 await expect.poll(async()=>(await titleColumn.boundingBox())!.width).toBeGreaterThan(before+80)
 await handle.focus();await page.keyboard.press('ArrowLeft')
 await expect.poll(async()=>(await titleColumn.boundingBox())!.width).toBeLessThan(before+80)
 const after=(await titleColumn.boundingBox())!.width
 await page.reload();await enterLatestMatter(page)
 await page.locator('.matter-rail').getByRole('button',{name:'Documents',exact:true}).click()
 await page.getByRole('button',{name:'Sort',exact:true}).click()
 await expect.poll(async()=>(await page.locator('.batch-table th').filter({hasText:'Title'}).boundingBox())!.width).toBeGreaterThan(after-2)
 await page.locator('.batch-row-menu summary').first().click()
 await expect(page.locator('.batch-row-menu[open]')).toHaveCount(1)
 await page.getByRole('heading',{name:'Batch review'}).click()
 await expect(page.locator('.batch-row-menu[open]')).toHaveCount(0)
})
test('Batch review OCR filter finds an image-only source',async({page,request})=>{
 const {matter}=await seed(request)
 const buffer=await readFile(resolve(import.meta.dirname,'../../samples/04_scanned_annexure.jpg'))
 const upload=await request.post(`${base}/matters/${matter.id}/documents?included=true`,{headers,multipart:{files:{name:'Synthetic_scan.jpg',mimeType:'image/jpeg',buffer}}})
 expect(upload.ok()).toBeTruthy()
 const scan=(await upload.json())[0]
 expect(scan.ocr_status).toBe('needs_ocr')
 await enter(page,4)
 await page.getByRole('button',{name:'Filters'}).click()
 await page.getByRole('combobox',{name:'Filter by status'}).selectOption('needs_ocr')
 await expect(page.locator('.batch-table tbody tr')).toHaveCount(1)
 await expect(page.locator('.batch-table tbody tr').first()).toContainText('Synthetic_scan.jpg')
})
test('Batch review explains and applies the two selected-row controls',async({page,request})=>{
 const {matter,docs}=await seed(request)
 await enter(page)
 const register=page.getByRole('region',{name:'Editable document table'})
 const part=register.getByRole('combobox',{name:'Set selected Part'})
 const type=register.getByRole('combobox',{name:'Set selected document type'})
 const apply=register.locator('.batch-apply-action')
 await expect(part).toBeDisabled();await expect(type).toBeDisabled()
 await expect(apply).toHaveText('Select rows first')
 await register.getByLabel(`Select ${docs[0].title}`).check()
 await register.getByLabel(`Select ${docs[1].title}`).check()
 await expect(part).toBeEnabled();await expect(type).toBeEnabled()
 await part.selectOption('2');await type.selectOption('annexure')
 await expect(apply).toHaveText('Apply to 2 selected')
 await apply.click()
 await expect(register.getByText('Updated 2 selected document(s) in one undoable action.')).toBeVisible()
 const saved=await(await request.get(`${base}/matters/${matter.id}/documents`,{headers})).json()
 expect(saved.slice(0,2).every((doc:any)=>doc.part===2&&doc.kind==='annexure')).toBe(true)
 expect(saved[2].part).toBe(1);expect(saved[2].kind).toBe('other')
 await expect(part).toHaveValue('');await expect(type).toHaveValue('')
 await expect(apply).toBeDisabled()
 const title=register.getByLabel(`Title for ${docs[0].original_name}`)
 await title.fill('Temporary title');await title.press('Escape')
 await expect(title).toHaveValue(docs[0].title)
 await register.locator('.batch-tools-menu>summary').click()
 await register.getByRole('button',{name:'More tools'}).click()
 await register.getByRole('button',{name:'Preview order',exact:true}).click()
 await expect(register.getByRole('dialog',{name:'Sort preview'})).toBeVisible()
 await register.getByRole('heading',{name:'Batch review'}).click()
 await expect(register.locator('.batch-advanced')).toHaveCount(0)
 await expect(register.getByRole('dialog',{name:'Sort preview'})).toHaveCount(0)
})
test('Batch review manages matter Parts without losing document assignments',async({page,request},info)=>{
 const {matter,docs}=await seed(request)
 await enter(page)
 const register=page.getByRole('region',{name:'Editable document table'})
 await register.getByRole('button',{name:'Manage Parts'}).click()
 const manager=page.getByRole('dialog',{name:'Manage Parts'})
 await expect(manager).toBeVisible()
 await expect(manager.getByText('Part 1',{exact:true})).toBeVisible()
 await expect(manager.getByRole('button',{name:'Remove Part 1'})).toBeDisabled()
 await manager.getByRole('button',{name:'Add Part 5'}).click()
 await expect(manager.getByText('Part 5',{exact:true})).toBeVisible()
 await page.screenshot({path:info.outputPath('manage-parts-1440-light.png'),animations:'disabled'})
 await page.setViewportSize({width:1024,height:768})
 await page.evaluate(()=>{document.documentElement.dataset.theme='dark';document.documentElement.dataset.effectiveTheme='dark';document.documentElement.style.zoom='125%'})
 await expect(manager).toBeInViewport()
 await expect(manager.getByRole('button',{name:'Add Part 6'})).toBeInViewport()
 await page.screenshot({path:info.outputPath('manage-parts-1024-dark-125.png'),animations:'disabled'})
 await page.setViewportSize({width:1440,height:900})
 await page.evaluate(()=>{document.documentElement.dataset.theme='light';document.documentElement.dataset.effectiveTheme='light';document.documentElement.style.zoom='100%'})
 await manager.getByRole('button',{name:'Close Manage Parts'}).click()
 await register.getByLabel(`Select ${docs[0].title}`).check()
 await register.getByRole('combobox',{name:'Set selected Part'}).selectOption('5')
 await register.locator('.batch-apply-action').click()
 await expect.poll(async()=>{const saved=await(await request.get(`${base}/matters/${matter.id}/documents`,{headers})).json();return saved[0].part}).toBe(5)
 await register.getByRole('button',{name:'Manage Parts'}).click()
 await expect(manager.getByRole('button',{name:'Remove Part 5'})).toBeDisabled()
 await manager.getByRole('button',{name:'Close Manage Parts'}).click()
 await register.getByRole('combobox',{name:'Set selected Part'}).selectOption('1')
 await register.locator('.batch-apply-action').click()
 await register.getByRole('button',{name:'Manage Parts'}).click()
 await manager.getByRole('button',{name:'Remove Part 5'}).click()
 await expect(manager.getByText('Part 5',{exact:true})).toHaveCount(0)
 await manager.getByRole('button',{name:'Add Part 6'}).click()
 await expect(manager.getByText('Part 6',{exact:true})).toBeVisible()
 await manager.getByRole('button',{name:'Close Manage Parts'}).click()
 await expect(register.getByRole('combobox',{name:'Set selected Part'}).locator('option[value="6"]')).toHaveCount(1)
 await expect(register.getByRole('combobox',{name:'Set selected Part'}).locator('option[value="5"]')).toHaveCount(0)
})
test('Batch review remains usable with 38 real synthetic documents across theme and scale',async({page,request},info)=>{
 test.setTimeout(180000)
 const {matter}=await seed(request)
 const buffer=await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'))
 for(let start=4;start<=38;start+=5){
  const uploads=await Promise.all(Array.from({length:Math.min(5,39-start)},(_,offset)=>request.post(`${base}/matters/${matter.id}/documents?included=true`,{headers,multipart:{files:{name:`Synthetic_record_${String(start+offset).padStart(2,'0')}.pdf`,mimeType:'application/pdf',buffer}}})))
  expect(uploads.every((response:any)=>response.ok())).toBe(true)
 }
 await enter(page,38)
 await matrix(page,info,'06A')
 const scroller=page.locator('.batch-table-scroll')
 await page.setViewportSize({width:1024,height:768})
 await page.evaluate(()=>{document.documentElement.dataset.theme='dark';document.documentElement.dataset.effectiveTheme='dark';document.documentElement.style.setProperty('--ui-scale','1.25');(document.querySelector('.app-shell') as HTMLElement).style.setProperty('--documents-interface-scale','1.25');document.documentElement.style.zoom='125%'})
 await scroller.evaluate(element=>{element.scrollTop=element.scrollHeight})
 await expect(page.locator('.batch-table tbody tr').last()).toBeInViewport()
 await expect(page.getByRole('button',{name:'Save changes',exact:true})).toBeInViewport()
 expect(await scroller.evaluate(element=>element.scrollWidth>element.clientWidth)).toBe(true)
 await page.screenshot({path:info.outputPath('06A-dense-38-1024-dark-125-bottom.png'),animations:'disabled'})
 const frozenTitle=page.locator('.batch-table th').filter({hasText:'Title'})
 const titleX=(await frozenTitle.boundingBox())!.x
 await scroller.evaluate(element=>{element.scrollLeft=element.scrollWidth})
 expect(Math.abs((await frozenTitle.boundingBox())!.x-titleX)).toBeLessThan(2)
 await expect(page.locator('.batch-table th').filter({hasText:'Status'})).toBeInViewport()
 await page.screenshot({path:info.outputPath('06A-dense-38-1024-dark-125-right.png'),animations:'disabled'})
})
test('06B batch save shows the supplied saved state and persists the reviewed value',async({page,request},info)=>{
 const {matter,docs}=await seed(request)
 await enter(page)
 const geometry=[] as Array<{selector:string,actual:number[],expected:number[]}>
 for(const [selector,x,y,width,height] of [
  ['.batch-review-toolbar',88,220,1328,48],
  ['.batch-part-select select',236,227,220,34],
  ['.batch-kind-select',470,227,154,34],
  ['.batch-columns-trigger',1196,227,112,34],
  ['.batch-density-trigger',1314,227,92,34],
  ['.batch-table thead',88,284,1328,38],
  ['.batch-table tbody tr:first-child',88,322,1328,38],
  ['.batch-review-heading>.membership-button',1236,140,180,34],
 ] as const){const box=(await page.locator(selector).boundingBox())!;geometry.push({selector,actual:[box.x,box.y,box.width,box.height].map(Math.round),expected:[x,y,width,height]})}
 for(const item of geometry)for(let index=0;index<4;index++)expect(Math.abs(item.actual[index]-item.expected[index]),`${item.selector} coordinate ${index}`).toBeLessThanOrEqual(1)
 const title=page.getByLabel(`Title for ${docs[0].original_name}`,{exact:true})
 await title.fill('Reviewed synthetic order')
 await expect(page.locator('.batch-savebar .unsaved')).toHaveText('1 unsaved changes')
 await expect(page.locator('.preparation-save-state')).toContainText('Unsaved changes')
 await page.screenshot({path:info.outputPath('06A-dirty-1440-light-100.png'),animations:'disabled'})
 await page.getByRole('button',{name:'Save changes',exact:true}).click()
 await expect(page.locator('.batch-savebar .saved')).toHaveText('All changes saved')
 await expect(page.locator('.batch-savebar').getByRole('button',{name:'Save changes'})).toBeDisabled()
 await expect(page.locator('.design-shell>.toast.success')).toContainText('Changes saved locally')
 await page.screenshot({path:info.outputPath('06B-1440-light-100.png'),animations:'disabled'})
 const toast=(await page.locator('.design-shell>.toast.success').boundingBox())!
 expect([toast.x,toast.y,toast.width,toast.height].map(Math.round)).toEqual([1130,806,282,44])
 await expect(page.locator('.preparation-save-state')).toContainText('Saved locally')
 const saved=await(await request.get(`${base}/matters/${matter.id}/documents`,{headers})).json()
 expect(saved[0].title).toBe('Reviewed synthetic order')
 expect(saved[0].source_sha256).toBe(docs[0].source_sha256)
 expect(saved.slice(1)).toEqual(docs.slice(1))
})
test('B02C new batch and suggestion layouts, explicit one-field acceptance and persistence',async({page,request},info)=>{test.setTimeout(180000);
 const {matter,docs}=await seed(request);
 await enter(page);
 await matrix(page,info,'06A');
 await suggestions(page);
 await expect(page.getByRole('button',{name:'Accept 0 suggestions',exact:true})).toBeDisabled();
 await matrix(page,info,'06D');for(const [selector,x,width] of [['.suggestion-list',208,724],['.source-review',948,468]] as const){const b=(await page.locator(selector).boundingBox())!;
 expect(Math.round(b.x)).toBe(x);
 expect(Math.round(b.y)).toBe(266);
 expect(Math.round(b.width)).toBe(width);
 expect(Math.round(b.height)).toBe(586)}await page.getByLabel('Title to save',{exact:true}).fill('Reviewed order title');
 await page.getByLabel('Accept suggested title',{exact:true}).check();
 await expect(page.getByRole('button',{name:'Accept 1 suggestions',exact:true})).toBeEnabled();
 await page.getByRole('button',{name:'Accept 1 suggestions',exact:true}).click();
 await expect(page.getByRole('status').filter({hasText:'Selected fields accepted'})).toBeVisible();
 const saved=await(await request.get(`${base}/matters/${matter.id}/documents`,{headers})).json();
 expect(saved[0].title).toBe('Reviewed order title');
 expect(saved[0].document_date).toBe(docs[0].document_date);
 expect(saved.map((d:any)=>d.id)).toEqual(docs.map((d:any)=>d.id));
 expect(saved[1]).toEqual(docs[1]);
 await page.getByRole('button',{name:'Dismiss date',exact:true}).click();
 expect((await(await request.get(`${base}/matters/${matter.id}/documents`,{headers})).json())[0].document_date).toBe(docs[0].document_date);
 await page.getByRole('button',{name:'Back to batch review',exact:true}).click();
 await expect(page.getByLabel(`Title for ${docs[0].original_name}`,{exact:true})).toHaveValue('Reviewed order title');
 await expect(page.locator('.batch-tools-menu')).not.toHaveAttribute('open');
 await page.reload();await enterLatestMatter(page);
 await page.locator('.matter-rail').getByRole('button',{name:'Documents',exact:true}).click();
 await page.getByRole('button',{name:'Sort',exact:true}).click();
 await expect(page.getByLabel(`Title for ${docs[0].original_name}`,{exact:true})).toHaveValue('Reviewed order title')})
test('B02C failure retry stale rejection and Save Discard Keep editing guards',async({page,request})=>{const {matter,docs}=await seed(request);
 await enter(page);
 const title=page.getByLabel(`Title for ${docs[0].original_name}`,{exact:true});
 await title.fill('Batch draft');
 await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click();
 const guard=page.getByRole('alertdialog');
 await guard.getByRole('button',{name:'Keep editing',exact:true}).click();
 await expect(title).toHaveValue('Batch draft');
 await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click();
 await guard.getByRole('button',{name:'Save changes',exact:true}).click();
 await expect(page.locator('.build-workspace')).toBeVisible();
 await page.locator('.matter-rail').getByRole('button',{name:'Documents',exact:true}).click();
 await page.getByRole('button',{name:'Sort',exact:true}).click();
 await suggestions(page);
 await page.getByLabel('Title to save',{exact:true}).fill('Retained retry draft');
 await page.getByLabel('Accept suggested title',{exact:true}).check();
 await page.route('**/metadata-suggestions/accept',r=>r.fulfill({status:503,contentType:'application/json',body:'{"detail":"Synthetic accept failure"}'}));
 await page.getByRole('button',{name:'Accept 1 suggestions',exact:true}).click();
 await expect(page.getByRole('alert')).toContainText('Synthetic accept failure');
 await expect(page.getByLabel('Title to save',{exact:true})).toHaveValue('Retained retry draft');
 await page.unroute('**/metadata-suggestions/accept');
 await page.getByRole('button',{name:'Retry',exact:true}).click();
 await expect(page.getByRole('status').filter({hasText:'Selected fields accepted'})).toBeVisible();
 await page.getByLabel('Title to save',{exact:true}).fill('Stale draft');
 await page.getByLabel('Accept suggested title',{exact:true}).check();
 await request.patch(`${base}/documents/${docs[0].id}`,{headers,data:{title:'Concurrent confirmed title'}});
 await page.getByRole('button',{name:'Accept 1 suggestions',exact:true}).click();
 await expect(page.getByRole('alert')).toContainText('stale');
 await expect(page.getByLabel('Title to save',{exact:true})).toHaveValue('Stale draft');
 await page.getByRole('button',{name:'Back to batch review',exact:true}).click();
 await guard.getByRole('button',{name:'Keep editing',exact:true}).click();
 await expect(page.getByLabel('Title to save',{exact:true})).toHaveValue('Stale draft');
 await page.getByRole('button',{name:'Back to batch review',exact:true}).click();
 await guard.getByRole('button',{name:'Discard changes',exact:true}).click();
 await expect(title).toHaveValue('Concurrent confirmed title')})

test('B02C counts, empty filters, late source responses and compact keyboard review',async({page,request},info)=>{
 const {docs}=await seed(request)
 await enter(page)
 await suggestions(page)
 await expect(page.getByRole('button',{name:'All suggestions · 6',exact:true})).toBeVisible()
 await page.getByRole('button',{name:/Uncertain date ·/}).click()
 await expect(page.locator('.suggestion-table tbody tr')).toHaveCount(2)
 await page.getByRole('button',{name:'All suggestions · 6',exact:true}).click()
 // Hold a real source response, switch documents, then release the obsolete result.
 let release:()=>void=()=>{};const hold=new Promise<void>(r=>release=r)
 await page.route(`**/documents/${docs[1].id}/metadata-source?*`,async route=>{const response=await route.fetch();await hold;await route.fulfill({response})})
 await page.locator('.source-row-button').filter({hasText:docs[1].original_name}).first().click()
 await expect(page.locator('.source-review')).toContainText('Rendering source page')
 await page.locator('.source-row-button').filter({hasText:docs[2].original_name}).first().click()
 await expect(page.locator('.metadata-source-page img')).toHaveAttribute('alt',new RegExp(docs[2].original_name.replaceAll('.','\\.')))
 const image=await page.locator('.metadata-source-page img').getAttribute('alt')
 release()
 await expect(page.locator('.metadata-source-page img')).toHaveAttribute('alt',image!)
 await page.setViewportSize({width:1024,height:768})
 await page.evaluate(()=>{document.documentElement.dataset.theme='dark';document.documentElement.style.setProperty('--ui-scale','1.25');(document.querySelector('.app-shell') as HTMLElement).style.setProperty('--documents-interface-scale','1.25');document.documentElement.style.zoom='125%'})
 await page.getByRole('button',{name:'Review selected source',exact:true}).focus()
 await page.keyboard.press('Enter')
 await expect(page.locator('.source-review')).toBeVisible()
 await page.getByLabel('Date to save',{exact:true}).focus()
 await expect(page.getByLabel('Date to save',{exact:true})).toBeInViewport()
 await page.screenshot({path:info.outputPath('06D-compact-source-keyboard.png')})
 await page.keyboard.press('Escape')
 await expect(page.locator('.source-review')).toBeHidden()
 await expect(page.getByRole('button',{name:'Review selected source',exact:true})).toBeFocused()
 await page.getByRole('button',{name:'Review selected source',exact:true}).click()
 await expect(page.locator('.source-review')).toBeVisible()
 await page.mouse.click(250,350)
 await expect(page.locator('.source-review')).toBeHidden()
})

test('B02C unselected drafts survive acceptance and empty filters do not save dismissed dates',async({page,request})=>{
 const {matter,docs}=await seed(request)
 await enter(page);await suggestions(page)
 await page.getByLabel('Date to save',{exact:true}).fill('2026-09-17')
 await page.getByLabel('Title to save',{exact:true}).fill('Only title accepted')
 await page.getByLabel('Accept suggested title',{exact:true}).check()
 await page.getByRole('button',{name:'Accept 1 suggestions',exact:true}).click()
 await expect(page.getByRole('status').filter({hasText:'Selected fields accepted'})).toBeVisible()
 await expect(page.getByLabel('Date to save',{exact:true})).toHaveValue('2026-09-17')
 expect((await(await request.get(`${base}/matters/${matter.id}/documents`,{headers})).json())[0].document_date).toBe('')
 await page.getByRole('button',{name:'Dismiss date',exact:true}).click()
 for(const d of docs.slice(1)){
  await page.locator('.source-row-button').filter({hasText:d.original_name}).first().click()
  await page.getByRole('button',{name:'Dismiss date',exact:true}).click()
 }
 await page.getByRole('button',{name:'Uncertain date · 0',exact:true}).click()
 await expect(page.locator('.review-empty')).toContainText('No suggestions in this filter')
 await expect(page.getByRole('button',{name:'Accept 0 suggestions',exact:true})).toBeDisabled()
 await page.getByRole('button',{name:'Enlarge source',exact:true}).click()
 expect(await page.locator('.metadata-source-page img').evaluate(el=>el.getBoundingClientRect().width>el.parentElement!.clientWidth)).toBe(true)
 const saved=await(await request.get(`${base}/matters/${matter.id}/documents`,{headers})).json()
 expect(saved.every((d:any)=>d.document_date==='')).toBe(true)
})

test('B02C late parent refresh of our own save does not mark accepted evidence stale',async({page,request})=>{
 const {matter}=await seed(request)
 await enter(page);await suggestions(page)
 let release:()=>void=()=>{};const hold=new Promise<void>(r=>release=r)
 await page.route(`**/matters/${matter.id}/documents`,async route=>{const response=await route.fetch();await hold;await route.fulfill({response})})
 await page.getByLabel('Title to save',{exact:true}).fill('Accepted before parent refresh')
 await page.getByLabel('Accept suggested title',{exact:true}).check()
 await page.getByRole('button',{name:'Accept 1 suggestions',exact:true}).click()
 await expect(page.getByRole('status').filter({hasText:'Selected fields accepted'})).toBeVisible()
 release()
 await expect(page.locator('.source-review .review-context')).toContainText('Accepted before parent refresh')
 await page.getByLabel('Accept suggested date',{exact:true}).check()
 await expect(page.getByRole('button',{name:'Accept 1 suggestions',exact:true})).toBeEnabled()
 await expect(page.getByText('Review is stale. Your draft is retained.',{exact:false})).toBeHidden()
})
