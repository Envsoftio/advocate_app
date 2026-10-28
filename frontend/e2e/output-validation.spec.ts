import {test,expect} from '@playwright/test'
import {readFile,mkdir,writeFile} from 'node:fs/promises'
import {resolve} from 'node:path'
import {execFileSync} from 'node:child_process'
import {createHash} from 'node:crypto'
const base=`http://127.0.0.1:${process.env.COURTBUNDLE_E2E_BACKEND_PORT||8768}`
const headers={Authorization:'Bearer e2e-isolated-token'}
async function fixture(request:any){
 const matter=await(await request.post(`${base}/matters`,{headers,data:{name:'B01C synthetic output review',court_profile:'delhi-hc-illustrative',case_number:'DEMO / 2026'}})).json()
 const buffer=execFileSync(resolve(import.meta.dirname,'../../backend/.venv312/bin/python'),['-c',`import pymupdf,sys
p=pymupdf.open()
for i in range(3):
 page=p.new_page()
 if i!=1: page.insert_text((60,80),f'Synthetic validation document - source {i+1}')
sys.stdout.buffer.write(p.tobytes())`])
 const doc=(await(await request.post(`${base}/matters/${matter.id}/documents?included=true`,{headers,multipart:{files:{name:'Synthetic review.pdf',mimeType:'application/pdf',buffer}}})).json())[0]
 const job=await(await request.post(`${base}/matters/${matter.id}/build`,{headers})).json()
 await expect.poll(async()=> (await(await request.get(`${base}/jobs/${job.id}`,{headers})).json()).status).toBe('completed')
 const build=(await(await request.get(`${base}/matters/${matter.id}/exports`,{headers})).json())[0]
 return {matter,doc,build,buffer}
}
async function matrix(page:any,info:any,name:string){
 for(const size of [{width:1440,height:900},{width:1024,height:768}])for(const theme of ['light','dark'])for(const scale of [90,100,110,125]){
  await page.setViewportSize(size)
  await page.evaluate(({theme,scale}:any)=>{document.documentElement.dataset.theme=theme;document.documentElement.dataset.effectiveTheme=theme;document.documentElement.style.setProperty('--ui-scale',String(scale/100));document.documentElement.style.zoom=scale+'%'},{theme,scale})
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBeTruthy()
  const modal=page.locator('.output-checks-dialog')
  if(await modal.count()){
   const box=(await modal.boundingBox())!;expect(box.width).toBeLessThanOrEqual(size.width);expect(box.height).toBeLessThanOrEqual(size.height)
   await modal.getByRole('button',{name:'Done',exact:true}).scrollIntoViewIfNeeded();await expect(modal.getByRole('button',{name:'Done',exact:true})).toBeInViewport()
   await modal.evaluate((e:HTMLElement)=>e.scrollTop=0)
  }
  const finding=page.getByRole('complementary',{name:'Preflight finding'})
  if(await finding.count()){await finding.getByRole('button',{name:'Keep page & return'}).scrollIntoViewIfNeeded();await expect(finding.getByRole('button',{name:'Keep page & return'})).toBeInViewport({ratio:1});const pdf=(await page.locator('.pdf-paper').boundingBox())!;expect(pdf.width).toBeGreaterThan(200)}
  await page.screenshot({path:info.outputPath(`${name}-${size.width}-${theme}-${scale}.png`),animations:'disabled'})
 }
 await page.setViewportSize({width:1440,height:900});await page.evaluate(()=>{document.documentElement.dataset.theme='light';document.documentElement.dataset.effectiveTheme='light';document.documentElement.style.setProperty('--ui-scale','1');document.documentElement.style.zoom='100%'})
}
test('B01C new design, exact findings, persisted review, measured output and release',async({page,request},info)=>{
 test.setTimeout(240_000)
 const {matter,doc,build,buffer}=await fixture(request)
 await page.setViewportSize({width:1440,height:900});await page.goto('/');await expect(page.locator('.pdf-paper')).toBeVisible()
 await page.locator('.matter-rail').getByRole('button',{name:'Preflight checks',exact:true}).click()
 const preflight=page.locator('.preflight-workspace');await expect(preflight).toContainText('UNSEARCHABLE_PAGE'.toLowerCase().split('_').join(' '));await expect(preflight.getByRole('button',{name:'Review source 2'})).toBeVisible()
 await expect(page.locator('.section-enter-active')).toHaveCount(0);await page.evaluate(()=>document.fonts.ready)
 const summary=(await page.locator('.preflight-summary').boundingBox())!;expect(Math.round(summary.x)).toBe(208);expect(Math.round(summary.width)).toBe(1208)
 await matrix(page,info,'14C-preflight')
 await preflight.getByLabel('Show',{exact:true}).selectOption('warning')
 await preflight.getByRole('button',{name:'Review source 2'}).click()
 const panel=page.getByRole('complementary',{name:'Preflight finding'})
 await expect(panel).toContainText('Source 2 · Revision 1');await expect(page.getByRole('spinbutton',{name:'Go to page'})).toHaveValue('2')
 await panel.getByLabel('Review note').fill('Synthetic intentional blank page')
 await matrix(page,info,'ED34-finding')
 await panel.getByRole('button',{name:'Keep page & return'}).click()
 await expect(preflight.getByLabel('Show',{exact:true})).toHaveValue('warning');await expect(preflight.getByRole('button',{name:'Review source 2'})).toBeFocused()
 await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click()
 const workspace=page.locator('.build-workspace');await expect(workspace.getByRole('button',{name:'View output checks'})).toBeVisible()
 await workspace.getByRole('button',{name:'View output checks'}).click()
 const checks=page.getByRole('dialog',{name:/Output checks · build/});await expect(checks.locator('tbody tr')).toHaveCount(7)
 const box=(await checks.boundingBox())!;expect(Math.round(box.width)).toBe(1000);expect(Math.round(box.height)).toBe(704);expect(Math.round(box.x)).toBe(220);expect(Math.round(box.y)).toBe(98)
 await expect(checks).toContainText('not run');await expect(checks).toContainText('not applicable')
 await matrix(page,info,'BU07-output-checks')
 await checks.getByLabel('Output file',{exact:true}).selectOption('index.csv');await checks.getByRole('button',{name:'Run output checks'}).click();await expect(checks).toContainText('No PDFs selected');await checks.getByRole('button',{name:'Evidence: PDF counts match the page map',exact:true}).click();await expect(checks).toContainText('Selection contains no PDFs')
 await page.keyboard.press('Escape');await expect(workspace.getByRole('button',{name:'View output checks'})).toBeFocused()
 await workspace.getByRole('button',{name:'View output checks'}).click();await expect(checks.getByLabel('Output file',{exact:true})).toHaveValue('index.csv');await page.keyboard.press('Escape')
 for(const label of ['Index links and bookmarks','Printed page references','Document order and coverage','Scan and image readability'])await workspace.getByLabel(label,{exact:true}).check()
 await expect(workspace).toContainText('Technical and human review recorded')
 await page.reload();await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click();await expect(workspace.getByLabel('Document order and coverage')).toBeChecked()
 await workspace.getByRole('button',{name:'Export files',exact:true}).click();const exp=page.getByRole('dialog',{name:'Export bundle files'})
 await exp.getByLabel('Release reviewed output').check();await exp.getByLabel('File name').selectOption('combined.pdf')
 const event=page.waitForEvent('download');await exp.getByRole('button',{name:'Export files',exact:true}).click();const bytes=await readFile((await(await event).path())!)
 expect(createHash('sha256').update(bytes).digest('hex')).toBe(build.files.find((f:any)=>f.name==='combined.pdf').sha256)
 const output=info.outputPath('independently-reopened.pdf');await writeFile(output,bytes)
 const reopened=JSON.parse(execFileSync(resolve(import.meta.dirname,'../../backend/.venv312/bin/python'),['-c','import pymupdf,json,sys;p=pymupdf.open(sys.argv[1]);print(json.dumps({"pages":len(p),"toc":p.get_toc(),"labels":[x.get_label() for x in p]}))',output],{encoding:'utf8'}))
 expect(reopened.pages).toBe(build.pages);expect(reopened.toc).toEqual(build.validation_contract['combined.pdf'].toc)
 expect(reopened.labels).toEqual(build.validation_contract['combined.pdf'].labels)
 const original=await(await request.get(`${base}/documents/${doc.id}/original`,{headers})).body();expect(original.equals(buffer)).toBeTruthy()
 await request.patch(`${base}/documents/${doc.id}`,{headers,data:{title:'Changed effective input'}})
 expect((await request.get(`${base}/matters/${matter.id}/exports/${build.id}/combined.pdf?release=true`,{headers})).status()).toBe(409)
 expect((await request.get(`${base}/matters/${matter.id}/exports/${build.id}/inspection/combined.pdf`,{headers})).ok()).toBeTruthy()
})

test('B01C output history and late responses stay with the selected build',async({page,request})=>{
 const {matter,build}=await fixture(request)
 const job=await(await request.post(`${base}/matters/${matter.id}/build`,{headers})).json()
 await expect.poll(async()=>(await(await request.get(`${base}/jobs/${job.id}`,{headers})).json()).status).toBe('completed')
 await page.goto('/');await expect(page.locator('.pdf-paper')).toBeVisible();await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click()
 const workspace=page.locator('.build-workspace');await expect(workspace.getByRole('button',{name:'View output checks'})).toBeVisible()
 await page.evaluate(buildId=>{
  const original=window.fetch.bind(window),qa=window as any
  qa.heldValidation=0;qa.deliveredValidation=0
  let release!:()=>void;const held=new Promise<void>(resolve=>release=resolve)
  qa.releaseValidation=release
  window.fetch=async(...args:Parameters<typeof fetch>)=>{
   const response=await original(...args)
   if(response.url.includes(`/exports/${buildId}/validation/state`)){qa.heldValidation++;await held;qa.deliveredValidation++}
   return response
  }
 },build.id)
 try{
  await workspace.getByLabel('Saved build').selectOption(build.id)
  await workspace.getByRole('button',{name:'View output checks'}).click()
  await expect(page.getByRole('dialog',{name:/Output checks/})).toContainText('Checking saved file identities')
  await expect.poll(()=>page.evaluate(()=>(window as any).heldValidation)).toBeGreaterThan(0)
  await page.keyboard.press('Escape')
  await workspace.getByLabel('Saved build').selectOption(job.id)
  await expect(workspace).toContainText(job.id)
  await page.evaluate(()=>(window as any).releaseValidation())
  await expect.poll(()=>page.evaluate(()=>(window as any).deliveredValidation)).toBeGreaterThan(0)
  await workspace.getByRole('button',{name:'View output checks'}).click()
  const checks=page.getByRole('dialog',{name:/Output checks/});await expect(checks).toContainText(job.id.slice(0,8));await expect(checks).not.toContainText(build.id.slice(0,8))
 }finally{await page.evaluate(()=>(window as any).releaseValidation())}
})

test('B01C output finding returns to its selected run and rechecks without passing an acknowledgement',async({page,request})=>{
 const {matter,build}=await fixture(request)
 // Only this disposable build's expected label changes; saved bytes still match
 // their hashes, so the named check must detect the measured discrepancy.
 execFileSync(resolve(import.meta.dirname,'../../backend/.venv312/bin/python'),['-c',`import json,sys
from pathlib import Path
sys.path.insert(0,str(Path('..','backend').resolve()))
from app.file_io import read_bytes,write_bytes
p=Path(sys.argv[1])/'matters'/sys.argv[2]/'exports'/sys.argv[3]/'export_manifest.json'
m=json.loads(read_bytes(p));c=m['validation_contract']['combined.pdf'];i=int(next(iter(c['sources'])))
c['labels'][i]='UNEXPECTED';write_bytes(p,json.dumps(m).encode())`,process.env.COURTBUNDLE_E2E_DATA_DIR!,matter.id,build.id])
 const url=`${base}/matters/${matter.id}/exports/${build.id}/validation`
 const state=await(await request.post(url+'/run',{headers,data:{}})).json()
 const originalRun=state.history[0],finding=originalRun.checks.find((c:any)=>c.id==='labels').findings[0]
 expect(state.can_download).toBe(false)
 await page.goto('/');await expect(page.locator('.pdf-paper')).toBeVisible();await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click()
 await page.locator('.build-workspace').getByRole('button',{name:'View output checks'}).click()
 const checks=page.getByRole('dialog',{name:/Output checks/})
 await checks.getByLabel('Output file',{exact:true}).selectOption('combined.pdf');await checks.getByLabel('Validation history').selectOption(originalRun.id)
 await checks.getByRole('button',{name:`Review source ${finding.page_index+1}`,exact:true}).click()
 const panel=page.getByRole('complementary',{name:'Preflight finding'})
 await expect(page.getByRole('spinbutton',{name:'Go to page'})).toHaveValue(String(finding.page_index+1))
 await panel.getByLabel('Review note').fill('Synthetic review does not repair the saved PDF')
 await panel.getByRole('button',{name:'Keep page & return'}).click()
 await expect(checks).toBeVisible();await expect(checks.getByLabel('Output file',{exact:true})).toHaveValue('combined.pdf');await expect(checks.getByLabel('Validation history')).toHaveValue(originalRun.id)
 await expect(checks.getByRole('button',{name:`Review source ${finding.page_index+1}`,exact:true})).toBeFocused()
 const after=await(await request.get(url+'/state',{headers})).json()
 expect(after.history.length).toBe(state.history.length+1);expect(after.can_download).toBe(false)
 expect(after.run.checks.find((c:any)=>c.id==='labels').outcome).toBe('failed')
 const note=await(await request.get(`${base}/matters/${matter.id}/validation/findings/${originalRun.id}/${finding.finding_id}`,{headers})).json()
 expect(note.review.acknowledged).toBe(true)
})
