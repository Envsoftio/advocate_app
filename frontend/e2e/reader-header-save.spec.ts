import { enterLatestMatter } from './navigation'
import { test, expect } from '@playwright/test'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
const base='http://127.0.0.1:8768',headers={Authorization:'Bearer e2e-isolated-token'}
test('document header Save commits a real tool draft once, without exporting or changing the original',async({page,request})=>{
  const source=await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'))
  const matter=await (await request.post(`${base}/matters`,{headers,data:{name:'Synthetic header save',court_profile:'general-demo'}})).json()
  const [doc]=await (await request.post(`${base}/matters/${matter.id}/documents?included=true`,{headers,multipart:{files:{name:'header-save.pdf',mimeType:'application/pdf',buffer:source}}})).json()
  await page.setViewportSize({width:1440,height:900});await page.goto('/');await enterLatestMatter(page)
  const save=page.getByRole('button',{name:'Save',exact:true})
  await expect(page.locator('.pdf-text-layer')).toContainText('DRAFT PETITION')
  await expect(save).toBeDisabled()
  await page.getByRole('tab',{name:'Edit',exact:true}).click();await page.getByRole('button',{name:'Add text',exact:true}).click()
  await page.locator('.pdf-paper').click({position:{x:140,y:230}})
  const draft=page.getByRole('textbox',{name:/Add text on page/});await draft.fill('Saved through the document header')
  await expect(page.locator('.document-save-state')).toContainText('Unsaved changes')
  let writes=0,downloads=0,release!:()=>void
  const gate=new Promise<void>(resolve=>{release=resolve})
  await page.route(`**/documents/${doc.id}/annotations`,async route=>{
    if(route.request().method()==='POST'){writes++;await gate}
    await route.continue()
  })
  page.on('download',()=>downloads++)
  try {
    await save.click()
    await expect.poll(()=>writes).toBe(1)
    await expect(save).toBeDisabled()
    await expect(page.locator('.preparation-utility').getByRole('button',{name:'Undo last matter change',exact:true})).toBeDisabled()
    await expect(page.locator('.document-save-state')).toContainText('Saving')
  } finally {release()}
  await expect(page.locator('.document-save-state')).toContainText('Saved locally')
  await expect(save).toBeDisabled()
  const marks=await (await request.get(`${base}/matters/${matter.id}/annotations`,{headers})).json()
  expect(marks.filter((mark:{body:string})=>mark.body==='Saved through the document header')).toHaveLength(1)
  expect(writes).toBe(1);expect(downloads).toBe(0)
  const docs=await (await request.get(`${base}/matters/${matter.id}/documents`,{headers})).json()
  expect(docs.find((item:{id:string})=>item.id===doc.id).source_sha256).toBe(doc.source_sha256)
})
