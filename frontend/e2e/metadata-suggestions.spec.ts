import {test,expect} from '@playwright/test'
import {readFile} from 'node:fs/promises'
import {resolve} from 'node:path'
const base=`http://127.0.0.1:${process.env.COURTBUNDLE_E2E_BACKEND_PORT||8768}`,headers={Authorization:'Bearer e2e-isolated-token'}
async function seed(request:any){const matter=await(await request.post(`${base}/matters`,{headers,data:{name:'Synthetic metadata review',court_profile:'general-demo'}})).json();
 const buffer=await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'));
 const docs=[];for(const name of ['Order_12_Jan_2026.pdf','Annexure_A_Agreement.pdf','Written_Submissions_03-04-2026.pdf']){const r=await request.post(`${base}/matters/${matter.id}/documents?included=true`,{headers,multipart:{files:{name,mimeType:'application/pdf',buffer}}});
 expect(r.ok()).toBeTruthy();docs.push((await r.json())[0])}return {matter,docs}}
async function enter(page:any){page.on('pageerror',(e:Error)=>{throw e});
 await page.setViewportSize({width:1440,height:900});
 await page.goto('/');
 await expect(page.locator('.pdf-paper')).toBeVisible();
 await page.locator('.matter-rail').getByRole('button',{name:'Documents',exact:true}).click();
 await page.getByRole('button',{name:'Sort',exact:true}).click();
 await expect(page.locator('.batch-table tbody tr')).toHaveCount(3);
 await page.evaluate(()=>document.fonts.ready)}
async function suggestions(page:any){await page.getByRole('button',{name:'Review title & dates',exact:true}).click();
 await expect(page.locator('.metadata-source-page img')).toBeVisible()}
async function matrix(page:any,info:any,name:string){for(const size of [{width:1440,height:900},{width:1024,height:768}])for(const theme of ['light','dark'])for(const scale of [90,100,110,125]){await page.setViewportSize(size);
 await page.evaluate(({theme,scale})=>{document.documentElement.dataset.theme=theme;document.documentElement.dataset.effectiveTheme=theme;document.documentElement.style.setProperty('--ui-scale',String(scale/100));(document.querySelector('.app-shell') as HTMLElement).style.setProperty('--documents-interface-scale',String(scale/100));document.documentElement.style.zoom=scale+'%'},{theme,scale});
 expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBe(true);
 const action=page.getByRole('button',{name:name==='06A'?'Save changes':/Accept \d+ suggestions/,exact:name==='06A'});
 await expect(action).toBeInViewport();
 await page.screenshot({path:info.outputPath(`${name}-${size.width}-${theme}-${scale}.png`),animations:'disabled'})}await page.setViewportSize({width:1440,height:900});
 await page.evaluate(()=>{document.documentElement.dataset.theme='light';document.documentElement.dataset.effectiveTheme='light';document.documentElement.style.setProperty('--ui-scale','1');(document.querySelector('.app-shell') as HTMLElement).style.setProperty('--documents-interface-scale','1');document.documentElement.style.zoom='100%'})}
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
 await page.reload();
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
