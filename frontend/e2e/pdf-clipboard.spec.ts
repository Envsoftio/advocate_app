import {test,expect,type Locator,type Page} from '@playwright/test'
import {readFile} from 'node:fs/promises'
import {resolve} from 'node:path'
import {openDocumentBrowser} from './navigation'
const base=`http://127.0.0.1:${process.env.COURTBUNDLE_E2E_BACKEND_PORT||8768}`
const headers={Authorization:'Bearer e2e-isolated-token'}

async function copyPdfWord(page:Page,layer:Locator){
 const span=layer.locator('span').filter({hasText:'DRAFT PETITION - EXAMPLE ONLY'}).first()
 await expect(span).toBeVisible()
 const point=await span.evaluate(el=>{
  const text=el.firstChild!,start=text.textContent!.indexOf('PETITION'),r=document.createRange()
  r.setStart(text,start);r.setEnd(text,start+8);const b=r.getBoundingClientRect()
  return {x:b.x+b.width/2,y:b.y+b.height/2}
 })
 await page.mouse.dblclick(point.x,point.y)
 await expect.poll(()=>page.evaluate(()=>getSelection()?.toString().trim())).toBe('PETITION')
 await page.keyboard.press('ControlOrMeta+c')
 await expect.poll(()=>page.evaluate(()=>navigator.clipboard.readText())).toBe('PETITION')
}

test('PDF copy pastes into searches and editable text while app chrome cannot be selected',async({page,request,context},info)=>{
 await context.grantPermissions(['clipboard-read','clipboard-write'])
 const matter=await(await request.post(`${base}/matters`,{headers,data:{name:'Synthetic clipboard test',court_profile:'general-demo'}})).json()
 const source=await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'))
 await request.post(`${base}/matters/${matter.id}/documents?included=true`,{headers,multipart:{files:{name:'Petition.pdf',mimeType:'application/pdf',buffer:source}}})
 await page.setViewportSize({width:1440,height:900});await page.goto('/')
 const layer=page.locator('.viewer-root .pdf-text-layer')
 await copyPdfWord(page,layer)
 await page.screenshot({path:info.outputPath('pdf-text-selected.png')})
 await page.keyboard.press('ControlOrMeta+f')
 const search=page.getByRole('searchbox',{name:'Find text in this PDF'})
 await search.focus();await page.keyboard.press('ControlOrMeta+v');await expect(search).toHaveValue('PETITION')
 await page.keyboard.press('ControlOrMeta+a');await page.keyboard.press('ControlOrMeta+x');await expect(search).toHaveValue('')
 await page.keyboard.press('ControlOrMeta+v');await expect(search).toHaveValue('PETITION')
 await page.keyboard.press('Escape');await openDocumentBrowser(page)
 const documents=page.getByRole('textbox',{name:'Search documents',exact:true})
 await documents.focus();await page.keyboard.press('ControlOrMeta+v');await expect(documents).toHaveValue('PETITION')
 await page.getByRole('button',{name:'Close document browser',exact:true}).click()
 await page.getByRole('tab',{name:'Edit',exact:true}).click();await page.getByRole('button',{name:'Add text',exact:true}).click();await page.locator('.pdf-paper').click({position:{x:150,y:250}})
 const text=page.getByRole('textbox',{name:/Add text on page/});await text.focus();await page.keyboard.press('ControlOrMeta+v');await expect(text).toHaveValue('PETITION')
 await text.fill('')
 await page.keyboard.press('Escape');await page.getByRole('tab',{name:'Read',exact:true}).click()
 for(const locator of [page.locator('.application-brand strong'),page.getByRole('tab',{name:'Read',exact:true}),page.locator('.document-revision')]){
  await expect(locator).toHaveCSS('user-select','none')
  await page.evaluate(()=>getSelection()?.removeAllRanges());await locator.dblclick();expect(await page.evaluate(()=>getSelection()?.toString())).toBe('')
 }
 await page.getByRole('combobox',{name:'Document version',exact:true}).selectOption('original')
 const review=page.getByRole('dialog',{name:/original/i})
 await copyPdfWord(page,review.locator('.pdf-text-layer'))
 await review.getByRole('button',{name:'Search artifact',exact:true}).click()
 const artifactSearch=review.getByRole('searchbox');await artifactSearch.focus();await page.keyboard.press('ControlOrMeta+v');await expect(artifactSearch).toHaveValue('PETITION')
})
