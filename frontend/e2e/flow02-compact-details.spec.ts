import { expect, test } from '@playwright/test'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import { enterLatestMatter } from './navigation'

const base = `http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT || 8768}`
const headers = {Authorization:'Bearer e2e-isolated-token'}

test('05C compact details floats over the working reader and Cancel discards the draft', async ({page,request},info) => {
  const matter = await (await request.post(`${base}/matters`, {headers,data:{name:'Synthetic compact details',court_profile:'general-demo'}})).json()
  const bytes = await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'))
  const uploaded = await request.post(`${base}/matters/${matter.id}/documents?included=true`,{
    headers,multipart:{files:{name:'synthetic-petition.pdf',mimeType:'application/pdf',buffer:bytes}},
  })
  expect(uploaded.ok()).toBeTruthy()
  const [doc] = await uploaded.json()
  await page.goto('/')
  await enterLatestMatter(page)
  await page.setViewportSize({width:1024,height:768})
  if (await page.getByRole('button',{name:'Collapse sidebar',exact:true}).isVisible())
    await page.getByRole('button',{name:'Collapse sidebar',exact:true}).click()
  await page.locator('.matter-rail').getByRole('button',{name:'Documents',exact:true}).click()
  await page.getByRole('button',{name:'Document details',exact:true}).click()
  const drawer = page.getByLabel('Document details',{exact:true}).filter({has:page.getByRole('button',{name:'Close document details'})})
  await expect(drawer).toBeVisible()
  await expect.poll(async()=>{const box=await drawer.boundingBox();return [Math.round(box!.x),Math.round(box!.y),Math.round(box!.width),Math.round(box!.height)]}).toEqual([664,116,360,624])
  await drawer.getByRole('textbox',{name:'Display title'}).fill('Unsaved compact title')
  await expect(drawer).toContainText('Unsaved changes')
  await page.screenshot({path:info.outputPath('05C-compact-details.png'),animations:'disabled'})
  await drawer.getByRole('button',{name:'Cancel',exact:true}).click()
  await expect(drawer).toBeHidden()
  const saved = (await (await request.get(`${base}/matters/${matter.id}/documents`,{headers})).json())[0]
  expect(saved.title).toBe(doc.title)
})
