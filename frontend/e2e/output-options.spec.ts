import {test,expect} from '@playwright/test'
import {readFile} from 'node:fs/promises'
import {resolve} from 'node:path'
import {createHash} from 'node:crypto'
const base=`http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT||8768}`
const headers={Authorization:'Bearer e2e-isolated-token'}
const hash=(b:Buffer)=>createHash('sha256').update(b).digest('hex')
async function seed(request:any){
 const matter=await(await request.post(`${base}/matters`,{headers,data:{name:'B03A Synthetic output options',court_profile:'general-demo'}})).json()
 const buffer=await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'))
 const docs=await(await request.post(`${base}/matters/${matter.id}/documents`,{headers,multipart:{files:{name:'Synthetic petition.pdf',mimeType:'application/pdf',buffer}}})).json()
 const membership=await(await request.get(`${base}/matters/${matter.id}/membership`,{headers})).json()
 expect((await request.put(`${base}/matters/${matter.id}/membership`,{headers,data:{included_ids:[docs[0].id],expected_snapshot:membership.input_snapshot}})).ok()).toBeTruthy()
 return {matter,doc:docs[0],buffer}
}
async function enter(page:any){await page.setViewportSize({width:1440,height:900});await page.goto('/');await expect(page.locator('.pdf-paper')).toBeVisible();await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click();await expect(page.locator('.section-enter-active')).toHaveCount(0);await page.evaluate(()=>document.fonts.ready);await page.getByRole('button',{name:'Output options',exact:true}).click();await expect(page.getByLabel('Page quality',{exact:true})).toHaveValue('original')}
test('B03A BU04 reference geometry, appearance matrix, keyboard and dirty cancellation',async({page,request},info)=>{
 await seed(request);await enter(page)
 const dialog=page.getByRole('dialog',{name:'Output options',exact:true})
 const box=(await dialog.boundingBox())!;expect(Math.round(box.x)).toBe(240);expect(Math.round(box.y)).toBe(104);expect(Math.round(box.width)).toBe(960);expect(Math.round(box.height)).toBe(692)
 for(const [selector,width] of [['.output-controls',440],['.output-preview',456]] as const){const b=(await page.locator(selector).boundingBox())!;expect(Math.abs(b.width-width)).toBeLessThanOrEqual(1);expect(Math.abs(b.y-218)).toBeLessThanOrEqual(2)}
 await expect(dialog.getByLabel('Page quality',{exact:true})).toHaveCSS('height','36px')
 for(const size of [{width:1440,height:900},{width:1024,height:768}])for(const theme of ['light','dark'])for(const scale of [90,100,110,125]){
  await page.setViewportSize(size);await page.evaluate(({theme,scale})=>{document.documentElement.dataset.theme=theme;document.documentElement.dataset.effectiveTheme=theme;document.documentElement.style.setProperty('--ui-scale',String(scale/100));document.documentElement.style.zoom=scale+'%'},{theme,scale})
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBe(true)
  await expect(dialog.getByRole('button',{name:'Apply options',exact:true})).toBeInViewport()
  await dialog.getByLabel('Page quality',{exact:true}).focus();for(let i=0;i<5;i++)await page.keyboard.press('Tab');await expect(dialog.getByLabel('PPTX',{exact:true})).toBeFocused();await expect(dialog.getByLabel('PPTX',{exact:true})).toBeInViewport();await page.keyboard.press('Space');await expect(dialog.getByLabel('PPTX',{exact:true})).toBeChecked();await page.keyboard.press('Space')
  await page.locator('.output-scroll').evaluate(el=>el.scrollTop=0)
  await page.screenshot({path:info.outputPath(`BU04-${size.width}-${theme}-${scale}.png`),animations:'disabled'})
 }
 await page.keyboard.press('Escape');await expect(dialog).toBeHidden();await expect(page.getByRole('button',{name:'Output options',exact:true})).toBeFocused()
 await page.getByRole('button',{name:'Output options',exact:true}).click();await dialog.getByLabel('DOCX',{exact:true}).check();await page.keyboard.press('Escape');await expect(dialog.getByRole('alertdialog')).toBeVisible();await dialog.getByRole('button',{name:'Keep editing',exact:true}).click();await expect(dialog.getByLabel('DOCX',{exact:true})).toBeChecked();await expect(dialog.getByLabel('DOCX',{exact:true})).toBeFocused();await dialog.getByRole('button',{name:'Cancel',exact:true}).click();await dialog.getByRole('button',{name:'Discard changes',exact:true}).click();await expect(dialog).toBeHidden()
})
test('B03A saved options produce measured artifacts, stale results and preserved prior bytes',async({page,request},info)=>{
 test.setTimeout(180000);const {matter,doc,buffer}=await seed(request);await enter(page)
 const dialog=page.getByRole('dialog',{name:'Output options',exact:true});const w=page.locator('.build-workspace')
 await dialog.getByRole('button',{name:'Cancel',exact:true}).click();await w.getByRole('button',{name:'Build bundle',exact:true}).click();await expect(w.getByRole('heading',{name:'Bundle ready for review'})).toBeVisible({timeout:60000})
 const first=(await(await request.get(`${base}/matters/${matter.id}/exports`,{headers})).json())[0]
 const hashes=Object.fromEntries(await Promise.all(first.files.map(async(f:any)=>[f.name,hash(await(await request.get(`${base}/matters/${matter.id}/exports/${first.id}/${f.name}`,{headers})).body())])))
 await w.getByRole('button',{name:'Rebuild',exact:true}).click();await w.getByRole('button',{name:'Output options',exact:true}).click();await dialog.getByLabel('Page quality',{exact:true}).selectOption('balanced');await dialog.getByLabel('Remove author and editing metadata').check();for(const name of ['DOCX','XLSX','PPTX'])await dialog.getByLabel(name,{exact:true}).check()
 await dialog.getByRole('button',{name:'Apply options',exact:true}).click();await expect(dialog).toBeHidden()
 const saved=await(await request.get(`${base}/matters/${matter.id}/bundle-settings`,{headers})).json();expect(saved.output_marks).toMatchObject({processing_quality:'balanced',cleanup_metadata:true,office_docx:true,office_xlsx:true,office_pptx:true})
 await w.getByRole('button',{name:'Output options',exact:true}).click();await expect(dialog.getByLabel('Page quality',{exact:true})).toHaveValue('balanced');await dialog.getByRole('button',{name:'Cancel',exact:true}).click();await w.getByRole('button',{name:'Build bundle',exact:true}).click();await expect(w.getByRole('heading',{name:'Bundle ready for review'})).toBeVisible({timeout:60000})
 const artifacts=await(await request.get(`${base}/matters/${matter.id}/exports`,{headers})).json();const latest=artifacts[0];expect(latest.id).not.toBe(first.id);expect(artifacts.find((x:any)=>x.id===first.id).stale).toBe(true)
 for(const f of latest.files){const response=await request.get(`${base}/matters/${matter.id}/exports/${latest.id}/${f.name}`,{headers});expect(response.ok()).toBe(true);const bytes=await response.body();expect(bytes.length).toBe(f.bytes);expect(hash(bytes)).toBe(f.sha256);await expect(w.locator(`td[title="${f.bytes.toLocaleString()} actual bytes"]`).first()).toBeVisible()}
 for(const [name,digest] of Object.entries(hashes))expect(hash(await(await request.get(`${base}/matters/${matter.id}/exports/${first.id}/${name}`,{headers})).body())).toBe(digest)
 expect(hash(await(await request.get(`${base}/documents/${doc.id}/pdf?variant=original`,{headers})).body())).toBe(hash(buffer))
 await expect(w).toContainText('combined-review.docx');await expect(w).toContainText('index-review.xlsx');await page.screenshot({path:info.outputPath('B03A-measured-output.png')})
 await info.attach('measured-artifact.json',{body:JSON.stringify(latest,null,2),contentType:'application/json'})
 await w.getByRole('button',{name:'Review bundle',exact:true}).click();await expect(page.getByRole('dialog',{name:'Review generated PDF'}).locator('.pdf-paper')).toBeVisible()
})
test('B03A failed save retains draft, stale snapshot rejects, retry and reload recover',async({page,request})=>{
 const {matter}=await seed(request);await enter(page);const d=page.getByRole('dialog',{name:'Output options',exact:true})
 await d.getByLabel('DOCX',{exact:true}).check();await page.keyboard.press(process.platform==='darwin'?'Meta+s':'Control+s');await expect(d.getByText('Unapplied changes',{exact:true})).toBeHidden();await expect(d).toBeVisible();await d.getByLabel('DOCX',{exact:true}).uncheck();await d.getByLabel('XLSX',{exact:true}).check();await page.route('**/bundle-settings',r=>r.request().method()==='PUT'?r.fulfill({status:503,contentType:'application/json',body:'{"detail":"Synthetic settings failure"}'}):r.continue());await d.getByRole('button',{name:'Apply options'}).click();await expect(d.getByRole('alert')).toContainText('Synthetic settings failure');await expect(d.getByLabel('XLSX',{exact:true})).toBeChecked();await page.unroute('**/bundle-settings');await d.getByRole('button',{name:'Retry options'}).click();await expect(d).toBeHidden()
 await page.getByRole('button',{name:'Output options',exact:true}).click();await d.getByLabel('DOCX',{exact:true}).check();await request.patch(`${base}/matters/${matter.id}`,{headers,data:{name:'Changed while options were open'}});await d.getByRole('button',{name:'Apply options'}).click();await expect(d.getByRole('alert')).toContainText('Matter changed');await expect(d.getByLabel('DOCX',{exact:true})).toBeChecked();await d.getByRole('button',{name:'Reload saved options'}).click();await expect(d.getByLabel('DOCX',{exact:true})).not.toBeChecked();await expect(d.getByLabel('XLSX',{exact:true})).toBeChecked()
})
