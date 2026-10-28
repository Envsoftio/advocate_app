import { closeWorkspaceNavigation, openDocumentBrowser, openWorkspaceNavigation } from './navigation'
import { expect, test } from '@playwright/test'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'

test('matter home leads to overview and focused PDF editing', async ({ page, request }) => {
  const headers = { Authorization: 'Bearer e2e-isolated-token' }
  const base = 'http://127.0.0.1:8768'
  const response = await request.post(`${base}/matters`, { headers, data: {
    name: 'Synthetic workspace design', petitioner: 'Sample Applicant', respondent: 'Sample Respondent',
    case_number: 'DEMO-42', court_profile: 'general-demo',
  } })
  expect(response.ok()).toBeTruthy()
  const matter = await response.json()
  const upload = await request.post(`${base}/matters/${matter.id}/documents?included=true`, { headers, multipart: {
    files: { name: '01_petition.pdf', mimeType: 'application/pdf', buffer: await readFile(resolve(import.meta.dirname, '../../samples/01_petition.pdf')) },
  } })
  expect(upload.ok()).toBeTruthy()
  await page.setViewportSize({width:1440,height:1000})
  await page.goto('/')
  await openWorkspaceNavigation(page)
  await page.locator('.workspace-drawer').getByRole('button', {name:'Matters',exact:true}).click()
  await expect(page.getByRole('heading', {name:'Your matters'})).toBeVisible()
  await page.getByRole('textbox', {name:'Search matters'}).fill('no matching synthetic case')
  await expect(page.getByRole('heading', {name:'No matching matters'})).toBeVisible()
  await page.getByRole('textbox', {name:'Search matters'}).fill('DEMO-42')
  await page.screenshot({path:'test-results/matter-home.png',fullPage:true})
  await page.locator('.matter-home-card').click()
  await expect(page.getByRole('region', {name:'Matter overview'})).toBeVisible()
  await expect(page.locator('.overview-context').first()).toContainText('Sample Applicant')
  await page.screenshot({path:'test-results/matter-overview.png',fullPage:true})
  await page.getByRole('button', {name:'Open documents',exact:true}).click()
  await expect(page.locator('.preview-panel canvas').first()).toBeVisible()
  await openDocumentBrowser(page)
  await expect(page.locator('.document-thumbnail img').first()).toBeVisible()
  await expect.poll(()=>page.locator('.document-thumbnail img').first().evaluate((image:HTMLImageElement)=>image.naturalWidth)).toBeGreaterThan(0)
  await page.locator('.doc-item').first().click()
  await expect(page.locator('.docs-panel')).toBeHidden()
  await expect(page.getByRole('tab',{name:'Read',exact:true})).toHaveAttribute('aria-selected','true')
  await expect(page.locator('.zoom-select')).toHaveCSS('height','34px')
  await page.getByRole('button',{name:'Next page',exact:true}).click()
  await expect(page.locator('.page-counter input')).toHaveValue('2')
  await expect(page.locator('.thumbnail-rail canvas[data-page="2"]')).toHaveAttribute('data-rendered','true')
  const reader=await page.locator('.shared-reader-viewport').elementHandle()
  await page.getByRole('button',{name:'Focused reading',exact:true}).click()
  await expect(page.locator('.thumbnail-rail')).toBeHidden()
  await expect(page.getByRole('button',{name:'Exit focused reading'})).toBeFocused()
  await page.keyboard.press('Escape')
  await expect(page.getByRole('button',{name:'Focused reading',exact:true})).toBeFocused()
  expect(await reader!.evaluate(el=>el.isConnected)).toBe(true)
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab',{name:'Pages',exact:true}).click()
  await page.getByRole('button',{name:'Page tools',exact:true}).click()
  await expect(page.getByRole('dialog',{name:'Page tools'})).toBeVisible()
  await page.keyboard.press('Escape');await expect(page.getByRole('dialog',{name:'Page tools'})).toBeHidden()
  for(const width of [1440,900,390]) {
    await page.setViewportSize({width,height:900})
    await openWorkspaceNavigation(page)
    await page.getByRole('combobox',{name:'Interface size'}).selectOption(width===1440?'125':'100')
    await page.getByRole('button',{name:'Close workspace navigation',exact:true}).click()
    await expect(page.locator('.workspace-drawer')).toBeHidden()
    expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true)
    await expect(page.locator('.application-matter')).toContainText('Synthetic workspace design')
  }
})
