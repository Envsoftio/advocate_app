import {test,expect} from '@playwright/test'
import {readFile} from 'node:fs/promises'
import {resolve} from 'node:path'
const base=`http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT||8768}`,headers={Authorization:'Bearer e2e-isolated-token'}
async function seed(request:any){
 const matter=await(await request.post(`${base}/matters`,{headers,data:{name:'Synthetic structure review',court_profile:'general-demo',petitioner:'Example Applicant',respondent:'Example Respondent',case_number:'DEMO 01/2026'}})).json()
 const buffer=await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'))
 for(const name of ['Petition','Affidavit','Order'])expect((await request.post(`${base}/matters/${matter.id}/documents?included=true`,{headers,multipart:{files:{name:name+'.pdf',mimeType:'application/pdf',buffer}}})).ok()).toBeTruthy()
 const s=await(await request.get(`${base}/matters/${matter.id}/bundle-settings`,{headers})).json()
 s.output_marks.front_matter='cover_index';s.output_marks.index_date=true;s.output_marks.index_style='review'
 for(const [name,cap] of [['Standard review',300],['Hearing review',200],['Print review',100]])await request.post(`${base}/output-presets`,{headers,data:{name,max_pages_per_volume:cap,keep_groups_intact:true,output_marks:s.output_marks}})
 const saved=await request.put(`${base}/matters/${matter.id}/bundle-settings`,{headers,data:{max_pages_per_volume:300,keep_groups_intact:true,output_marks:{...s.output_marks,cover_heading:'SYNTHETIC REVIEW FORUM',cover_title:'HEARING BUNDLE'},expected_snapshot:s.input_snapshot}});expect(saved.ok()).toBeTruthy()
 return matter
}
async function enter(page:any){await page.setViewportSize({width:1440,height:900});await page.goto('/');await expect(page.locator('.pdf-paper')).toBeVisible();await page.locator('.matter-rail').getByRole('button',{name:'Index & bookmarks',exact:true}).click();await expect(page.locator('.bundle-structure')).toBeVisible();await expect(page.locator('.section-enter-active')).toHaveCount(0);await page.evaluate(()=>document.fonts.ready)}
async function matrix(page:any,info:any,name:string){
 for(const size of [{width:1440,height:900},{width:1024,height:768}])for(const theme of ['light','dark'])for(const scale of [90,100,110,125]){
  await page.setViewportSize(size);await page.evaluate(({theme,scale})=>{document.documentElement.dataset.theme=theme;document.documentElement.dataset.effectiveTheme=theme;document.documentElement.style.setProperty('--ui-scale',String(scale/100));document.documentElement.style.zoom=scale+'%'},{theme,scale})
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBe(true)
  await page.screenshot({path:info.outputPath(`${name}-${size.width}-${theme}-${scale}.png`),animations:'disabled'})
 }
 await page.setViewportSize({width:1440,height:900});await page.evaluate(()=>{document.documentElement.dataset.theme='light';document.documentElement.dataset.effectiveTheme='light';document.documentElement.style.setProperty('--ui-scale','1');document.documentElement.style.zoom='100%'})
}
test('B02A new BU01/BU02 design, persisted overrides, real preview and cancel',async({page,request},info)=>{
 test.setTimeout(240000);const matter=await seed(request);await enter(page)
 const workspace=page.locator('.bundle-structure')
 await expect(workspace.locator('.page-map .current').first()).toBeVisible();await page.screenshot({path:info.outputPath('11C-index-1440-light.png'),animations:'disabled'})
 await workspace.getByRole('button',{name:'Presets',exact:true}).click()
 const dialog=page.getByRole('dialog',{name:'Bundle presets'})
 await expect(dialog.getByRole('radio')).toHaveCount(3)
 const box=(await dialog.boundingBox())!;expect(Math.round(box.width)).toBe(1060);expect(Math.round(box.height)).toBe(720);expect(Math.round(box.x)).toBe(190);expect(Math.round(box.y)).toBe(90)
 await expect(dialog.locator('.preset-list>label').first()).toHaveCSS('flex-direction','row');await expect(dialog.getByRole('radio').first()).toHaveCSS('width','16px');
 await matrix(page,info,'BU01')
 await dialog.getByLabel('Standard review', {exact:false}).check()
 await dialog.getByRole('button',{name:'Use selected preset'}).click();await expect(dialog).toBeHidden()
 await expect(workspace).toContainText('Applied: Standard review v1')
 await workspace.getByRole('button',{name:'Cover & index',exact:true}).click()
 await expect(workspace.locator('.front-preview-pages img')).toHaveCount(2)
 for(const [selector,x,y,width,height] of [['.front-settings',208,266,340,586],['.generated-front',564,266,852,586]] as const){const b=(await page.locator(selector).boundingBox())!;expect(Math.round(b.x)).toBe(x);expect(Math.round(b.y)).toBe(y);expect(Math.round(b.width)).toBe(width);expect(Math.round(b.height)).toBe(height)}
 await expect(workspace.getByLabel('Bundle title',{exact:true})).toHaveCSS('height','36px');
 await matrix(page,info,'BU02')
 await workspace.getByLabel('Bundle title',{exact:true}).fill('UNSAVED TITLE')
 await workspace.getByRole('button',{name:'Cancel changes'}).click();await expect(workspace.getByLabel('Bundle title',{exact:true})).toHaveValue('HEARING BUNDLE')
 await workspace.getByLabel('Date',{exact:true}).uncheck()
 await workspace.getByRole('button',{name:'Apply settings',exact:true}).click();await expect(workspace).toContainText('1 field override(s)')
 await workspace.getByRole('button',{name:'Preset',exact:true}).click();await expect(dialog).toContainText('Index date column')
 await dialog.getByLabel('Hearing review',{exact:false}).check();await dialog.getByRole('button',{name:'Use selected preset'}).click();await expect(dialog).toBeHidden()
 await expect(workspace.getByLabel('Date',{exact:true})).not.toBeChecked()
 let saved=await(await request.get(`${base}/matters/${matter.id}/bundle-settings`,{headers})).json();expect(saved.max_pages_per_volume).toBe(200);expect(saved.output_marks.index_date).toBe(false);expect(saved.output_marks.cover_title).toBe('HEARING BUNDLE')
 await page.reload();await page.locator('.matter-rail').getByRole('button',{name:'Index & bookmarks',exact:true}).click();await expect(page.locator('.bundle-structure')).toBeVisible();await workspace.getByRole('button',{name:'Cover & index',exact:true}).click();await expect(workspace.getByLabel('Date',{exact:true})).not.toBeChecked()
 await workspace.getByRole('button',{name:'Preset',exact:true}).click();await dialog.getByRole('button',{name:'Cancel',exact:true}).click();await expect(workspace.getByRole('button',{name:'Preset',exact:true})).toBeFocused()
})
test('B02A failed saves keep drafts, stale preview is ignored, navigation saves atomically',async({page,request})=>{
 const matter=await seed(request);await enter(page);const workspace=page.locator('.bundle-structure');await workspace.getByRole('button',{name:'Cover & index',exact:true}).click();await expect(workspace.locator('.front-preview-pages img')).toHaveCount(2)
 await page.route('**/bundle-settings',async route=>route.request().method()==='PUT'?route.fulfill({status:503,contentType:'application/json',body:'{"detail":"Synthetic save failure"}'}):route.continue())
 await workspace.getByLabel('Bundle title',{exact:true}).fill('RETRY TITLE');await workspace.getByRole('button',{name:'Apply settings',exact:true}).click();await expect(workspace.getByRole('alert')).toContainText('Synthetic save failure');await expect(workspace.getByLabel('Bundle title',{exact:true})).toHaveValue('RETRY TITLE')
 await page.unroute('**/bundle-settings');await workspace.getByRole('button',{name:'Retry settings',exact:true}).click();await expect(workspace).toContainText('Settings applied')
 // Hold completion of one real preview in the browser, then accept a newer response.
 await page.evaluate(()=>{const w=window as any;w.__originalFetch=window.fetch;w.__holdPreview=true;window.fetch=async(...args)=>{const result=await w.__originalFetch(...args);if(String(args[0]).includes('/front-matter-preview')&&w.__holdPreview){w.__holdPreview=false;w.__heldPreview=true;await new Promise<void>(resolve=>w.__releasePreview=resolve)}return result}})
 await workspace.getByLabel('Bundle title',{exact:true}).fill('OLDER TITLE');await expect.poll(()=>page.evaluate(()=>(window as any).__heldPreview)).toBe(true)
 await workspace.getByLabel('Bundle title',{exact:true}).fill('NEWER TITLE');await expect(workspace.locator('.front-preview-pages img')).toHaveCount(2)
 const newer=await workspace.locator('.front-preview-pages img').first().getAttribute('src');await page.evaluate(()=>(window as any).__releasePreview());await expect(workspace.locator('.front-preview-pages img').first()).toHaveAttribute('src',newer!)
 await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click();const guard=page.getByRole('alertdialog');await expect(guard).toBeVisible();await guard.getByRole('button',{name:"Save changes"}).click();await expect(page.locator('.build-workspace')).toBeVisible()
 const saved=await(await request.get(`${base}/matters/${matter.id}/bundle-settings`,{headers})).json();expect(saved.output_marks.cover_title).toBe('NEWER TITLE')
})
