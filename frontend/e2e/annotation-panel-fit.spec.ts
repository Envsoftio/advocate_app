import {expect,test} from '@playwright/test'
import {readFile} from 'node:fs/promises'
import {resolve} from 'node:path'
import {enterLatestMatter,openWorkspaceNavigation,closeWorkspaceNavigation} from './navigation'

const base=`http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT || 8768}`
const headers={Authorization:'Bearer e2e-isolated-token'}

test('annotation properties fit the right edge at supported sizes',async({page,request})=>{
  const matter=await (await request.post(`${base}/matters`,{headers,data:{name:'E2E Annotation Panel Fit',court_profile:'general-demo'}})).json()
  const uploaded=await request.post(`${base}/matters/${matter.id}/documents?included=true`,{headers,multipart:{
    files:{name:'01_petition.pdf',mimeType:'application/pdf',buffer:await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'))},
  }})
  expect(uploaded.ok()).toBe(true)
  const document=(await uploaded.json())[0]
  const created=await request.post(`${base}/documents/${document.id}/annotations`,{headers,data:{
    kind:'highlight',page_index:0,expected_revision:document.revision,rect_x:.2,rect_y:.2,
    rect_width:.22,rect_height:.04,body:'Review this passage',color:'yellow',output_scope:'private',
  }})
  expect(created.ok()).toBe(true)
  await page.goto('/');await enterLatestMatter(page)
  await page.locator('.pdf-review-highlight').click()
  await expect(page.getByLabel('PDF editing controls').getByText('Highlight',{exact:true}).first()).toBeVisible()
  await openWorkspaceNavigation(page)
  await page.getByRole('combobox',{name:'Theme'}).selectOption('dark')
  await closeWorkspaceNavigation(page)
  for(const width of [1440,1024,900,768]) {
    await page.setViewportSize({width,height:768})
    for(const scale of [90,100,110,125]) {
      await openWorkspaceNavigation(page)
      await page.getByRole('combobox',{name:'Interface size'}).selectOption(String(scale))
      await closeWorkspaceNavigation(page)
      const geometry=await page.evaluate(()=>{
        const rect=(selector:string)=>document.querySelector(selector)?.getBoundingClientRect().toJSON()
        const inspector=document.querySelector('.viewer-tool-inspector') as HTMLElement
        return {viewport:innerWidth,workspace:rect('.viewer-edit-workspace'),inspector:rect('.viewer-tool-inspector'),page:rect('.viewer-page-column'),
          inspectorClient:inspector?.clientWidth,inspectorScroll:inspector?.scrollWidth,documentScroll:document.documentElement.scrollWidth}
      })
      expect(geometry.workspace).toBeTruthy()
      expect(geometry.inspector).toBeTruthy()
      expect(geometry.page).toBeTruthy()
      expect(Math.abs(geometry.inspector!.right-geometry.workspace!.right)).toBeLessThan(1)
      expect(Math.abs(geometry.inspector!.top-geometry.workspace!.top)).toBeLessThan(1)
      expect(Math.abs(geometry.inspector!.bottom-geometry.workspace!.bottom)).toBeLessThan(1)
      expect(Math.abs(geometry.inspector!.left-geometry.page!.right)).toBeLessThan(1)
      expect(geometry.inspectorScroll).toBeLessThanOrEqual(geometry.inspectorClient+1)
      expect(geometry.documentScroll).toBeLessThanOrEqual(geometry.viewport+1)
      if(width===1024 && scale===125) {
        await page.screenshot({path:test.info().outputPath('highlight-panel-1024-dark-125.png'),animations:'disabled'})
        await page.locator('.viewer-tool-inspector').evaluate(element=>element.scrollTop=element.scrollHeight)
        const action=await page.getByRole('button',{name:'Update mark'}).boundingBox()
        const panel=await page.locator('.viewer-tool-inspector').boundingBox()
        expect(action).toBeTruthy()
        expect(panel).toBeTruthy()
        expect(action!.y).toBeGreaterThanOrEqual(panel!.y)
        expect(action!.y+action!.height).toBeLessThanOrEqual(panel!.y+panel!.height)
      }
    }
  }
})
