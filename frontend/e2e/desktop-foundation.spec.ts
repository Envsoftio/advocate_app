import { openBuildTool, submitBundleBuild, openOutputTools } from './navigation'
import { openDocumentActions } from './navigation'
import { expect, test } from '@playwright/test'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'

const backend = `http://127.0.0.1:${process.env.COURTBUNDLE_E2E_BACKEND_PORT || 8768}`
const headers = { Authorization: 'Bearer e2e-isolated-token' }
const source = resolve(import.meta.dirname, '../../samples/01_petition.pdf')

test('storage, idle autosave, Office downloads and print copy use one completed PDF build', async ({page, request}, testInfo) => {
  const created = await request.post(`${backend}/matters`, {headers, data:{
    name:'E2E Desktop Foundation', court_profile:'general-demo', max_pages_per_volume:20,
  }})
  expect(created.ok()).toBeTruthy()
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, {headers,
    multipart:{files:{name:'01_petition.pdf', mimeType:'application/pdf', buffer:await readFile(source)}},
  })
  expect(uploaded.ok()).toBeTruthy()
  const documentId = (await uploaded.json())[0].id as string

  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Desktop Foundation'})).toBeVisible()
  await page.getByRole('button', {name:'Overview', exact:true}).click()
  await expect(page.getByRole('region', {name:'Desktop file tools'})).toContainText('Open, save, print and export')
  await page.screenshot({path:testInfo.outputPath('desktop-file-tools.png')})
  await page.getByRole('button', {name:'PDF tools & print'}).click()
  await openDocumentActions(page)
  await page.getByRole('button', {name:'Document details'}).click()
  await page.getByRole('textbox', {name:'Display title'}).fill('Autosaved petition')
  await expect(page.getByRole('status').filter({hasText:'Document details saved'})).toBeVisible({timeout:15_000})
  const saved = await (await request.get(`${backend}/matters/${matterId}/documents`, {headers})).json()
  expect(saved.find((item:{id:string})=>item.id===documentId).title).toBe('Autosaved petition')

  await openDocumentActions(page)
  await page.getByRole('button', {name:'Print working copy'}).click()
  await expect(page.getByRole('heading', {name:'Print Autosaved petition'})).toBeVisible()
  await page.getByRole('textbox', {name:'Print pages'}).fill('2')
  await page.getByRole('button', {name:'Prepare print copy'}).click()
  await expect(page.getByTitle('Print copy preview')).toBeVisible()
  await page.getByRole('button', {name:'Close', exact:true}).click()

  await page.getByRole('button', {name:'Storage', exact:true}).click()
  await page.locator('.storage-maintenance>summary').click()
  await expect(page.getByRole('region', {name:'Matter storage'})).toContainText('Autosaved petition')
  await expect(page.getByRole('region', {name:'Matter storage'})).toContainText('Imported originals')
  await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click()
  await openBuildTool(page)
  await page.getByRole('spinbutton', {name:/Maximum pages per volume/}).fill('10')
  await expect(page.getByRole('status').filter({hasText:'Bundle settings saved'})).toBeVisible({timeout:15_000})
  await submitBundleBuild(page)
  await expect(page.locator('.generated-files')).toContainText('combined.pdf', {timeout:90_000})
  await openOutputTools(page)
  await expect(page.getByRole('group', {name:'Office review exports'})).toContainText('Word draft')
  await page.getByRole('button', {name:'Overview', exact:true}).click()
  await page.getByRole('button', {name:'Office exports', exact:true}).click()
  await expect(page.getByRole('group', {name:'Office review exports'})).toBeInViewport({ratio:.5})
  await page.screenshot({path:testInfo.outputPath('office-review-exports.png')})

  for (const name of ['Save Word review draft from combined.pdf',
                      'Save PowerPoint review draft from combined.pdf',
                      'Save Excel index workbook']) {
    const downloadPromise = page.waitForEvent('download')
    await page.getByRole('button', {name}).click()
    const download = await downloadPromise
    expect(download.suggestedFilename()).toMatch(/-review\.(docx|pptx|xlsx)$/)
  }
  await page.getByRole('button', {name:'Print combined.pdf'}).click()
  await page.getByRole('textbox', {name:'Print pages'}).fill('1')
  await page.getByRole('combobox', {name:'Print paper orientation'}).selectOption('landscape')
  await page.getByRole('button', {name:'Prepare print copy'}).click()
  await expect(page.getByTitle('Print copy preview')).toBeVisible()
  await page.getByRole('button', {name:'Close', exact:true}).click()
  await expect(page.getByTitle('Print copy preview')).toHaveCount(0)
})
