import { test, expect } from '@playwright/test'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import { enterLatestMatter } from './navigation'

const base = `http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT || 8768}`
const headers = { Authorization: 'Bearer e2e-isolated-token' }

test('FLOW / 02 source, OCR and focus states use real document and job data', async ({ page, request }, info) => {
  test.setTimeout(120_000)
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const matter = await (await request.post(`${base}/matters`, { headers, data: { name: 'Synthetic source states', court_profile: 'general-demo' } })).json()
  const source = await readFile(resolve(import.meta.dirname, '../../samples/04_scanned_annexure.jpg'))
  const uploaded = await request.post(`${base}/matters/${matter.id}/documents?included=true`, {
    headers, multipart: { files: { name: 'Scanned_source.jpg', mimeType: 'image/jpeg', buffer: source } },
  })
  expect(uploaded.ok()).toBeTruthy()
  const [doc] = await uploaded.json()
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto('/')
  await enterLatestMatter(page)
  if (await page.getByRole('button', { name: 'Collapse sidebar', exact: true }).isVisible())
    await page.getByRole('button', { name: 'Collapse sidebar', exact: true }).click()
  await page.locator('.matter-rail').getByRole('button', { name: 'Documents', exact: true }).click()
  await expect(page.getByRole('status', { name: 'Scanned pages detected' })).toBeVisible()
  await expect.poll(async () => Math.round((await page.getByRole('status', { name: 'Scanned pages detected' }).boundingBox())!.height)).toBe(88)
  await page.screenshot({ path: info.outputPath('07A-1440-light.png'), animations: 'disabled' })

  await page.getByRole('combobox', { name: 'Document version' }).selectOption('original')
  const original = page.getByRole('dialog', { name: 'Review imported original' })
  await expect(original).toBeVisible()
  await expect(original).toContainText('Read only')
  await expect(original).toContainText('Original image')
  await page.screenshot({ path: info.outputPath('05B-1440-light.png'), animations: 'disabled' })
  await original.getByRole('button', { name: 'Close review' }).click()

  await page.getByRole('button', { name: 'Run OCR', exact: true }).click()
  await expect(page.getByRole('status', { name: 'OCR processing' })).toBeVisible()
  await expect.poll(async () => Math.round((await page.getByRole('status', { name: 'OCR processing' }).boundingBox())!.y)).toBe(724)
  await page.screenshot({ path: info.outputPath('07B-1440-light.png'), animations: 'disabled' })
  await expect.poll(async () => {
    const rows = await (await request.get(`${base}/matters/${matter.id}/documents`, { headers })).json()
    return rows.find((row: { id: string }) => row.id === doc.id)?.ocr_status
  }, { timeout: 40_000 }).toBe('processed')
  await expect(page.getByRole('status', { name: 'OCR review recommended' })).toBeVisible()
  const inspector = page.getByRole('complementary', { name: 'OCR confidence inspector' })
  await expect(inspector).toContainText(/Recognition confidence · \d+%/)
  await expect(page.locator('.viewer-page-column canvas.pdf-paper').first()).toBeVisible()
  await page.screenshot({ path: info.outputPath('07C-1440-light.png'), animations: 'disabled' })
  const current = (await (await request.get(`${base}/matters/${matter.id}/documents`, { headers })).json())[0]
  expect(current.source_sha256).toBe(doc.source_sha256)

  await inspector.getByRole('button', { name: 'Rerun this page' }).click()
  await expect(page.getByRole('status', { name: 'OCR processing' })).toBeVisible()
  await expect.poll(async () => {
    const rows = await (await request.get(`${base}/matters/${matter.id}/documents`, { headers })).json()
    return rows.find((row: { id: string }) => row.id === doc.id)?.revision
  }, { timeout: 40_000 }).toBe(current.revision + 1)
  await expect(inspector).toContainText('Recognition confidence')
  await page.getByRole('button', { name: 'Accept review' }).click()
  await expect(page.getByRole('status', { name: 'OCR review recommended' })).toContainText('OCR page review accepted')

  await page.getByRole('button', { name: 'Focused reading', exact: true }).click()
  await expect(page.locator('.preparation-preview.focused-reading')).toBeVisible()
  await expect(page.getByRole('navigation', { name: 'Page thumbnails' })).toBeVisible()
  await expect(page.getByRole('navigation', { name: 'Page thumbnails' }).locator('canvas.is-rendered')).toBeVisible()
  await expect.poll(async () => Math.round((await page.getByRole('navigation', { name: 'Page thumbnails' }).boundingBox())!.y)).toBe(116)
  await expect.poll(async () => Math.round((await page.locator('.viewer-page-column').boundingBox())!.x)).toBe(208)
  await expect(page.locator('.pdf-paper')).toBeVisible()
  await page.screenshot({ path: info.outputPath('08A-1440-light.png'), animations: 'disabled' })
  await page.getByRole('button', { name: 'Search pages' }).click()
  await expect(page.getByRole('region', { name: 'Search document' })).toBeVisible()
  await page.getByRole('button', { name: 'Close PDF search' }).click()
  await expect(page.getByRole('navigation', { name: 'Page thumbnails' })).toBeVisible()
  await page.getByRole('button', { name: 'Exit focused reading' }).click()
  await page.getByRole('button', { name: 'Open editor' }).click()
  await page.getByRole('button', { name: 'Focused reading', exact: true }).click()
  await expect.poll(async () => Math.round((await page.locator('.viewer-page-column').boundingBox())!.y)).toBe(108)
  expect(errors).toEqual([])
})

test('FLOW / 02 unsaved batch state keeps its draft and destination', async ({ page, request }, info) => {
  const matter = await (await request.post(`${base}/matters`, { headers, data: { name: 'Synthetic unsaved batch', court_profile: 'general-demo' } })).json()
  const source = await readFile(resolve(import.meta.dirname, '../../samples/01_petition.pdf'))
  const uploaded = await request.post(`${base}/matters/${matter.id}/documents?included=true`, {
    headers, multipart: { files: { name: 'Petition.pdf', mimeType: 'application/pdf', buffer: source } },
  })
  const [doc] = await uploaded.json()
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto('/')
  await enterLatestMatter(page)
  if (await page.getByRole('button', { name: 'Collapse sidebar', exact: true }).isVisible())
    await page.getByRole('button', { name: 'Collapse sidebar', exact: true }).click()
  await page.locator('.matter-rail').getByRole('button', { name: 'Documents', exact: true }).click()
  await page.getByRole('combobox', { name: 'Document version' }).selectOption('original')
  const original = page.getByRole('dialog', { name: 'Review imported original' })
  await expect(original.getByRole('button', { name: 'Search artifact' })).toBeVisible()
  await expect.poll(async () => Math.round((await original.boundingBox())!.x)).toBe(344)
  await expect.poll(async () => Math.round((await original.boundingBox())!.y)).toBe(116)
  await expect(original.getByText('Changes are made to a separate working copy.')).toBeVisible()
  await expect(original.locator('.pdf-paper')).toBeVisible()
  const originalPage = await original.locator('.pdf-paper').boundingBox()
  expect(originalPage!.height).toBeGreaterThan(originalPage!.width)
  await original.getByRole('button', { name: 'Rotate original view clockwise' }).click()
  await expect.poll(async () => {
    const box = await original.locator('.pdf-paper').boundingBox()
    return box!.width > box!.height
  }).toBeTruthy()
  await original.getByRole('button', { name: 'Print imported original' }).click()
  const print = page.getByRole('dialog', { name: 'Print Petition.pdf' })
  await expect(print).toBeVisible()
  await print.getByRole('button', { name: 'Prepare print copy' }).click()
  await expect(print.getByTitle('Print copy preview')).toBeVisible()
  await print.getByRole('button', { name: 'Close', exact: true }).click()
  await original.getByRole('button', { name: 'Open working copy' }).click()
  await page.getByRole('button', { name: 'Sort', exact: true }).click()
  const title = page.getByRole('textbox', { name: `Title for ${doc.original_name}` })
  await title.fill('Unsaved synthetic title')
  await page.locator('.matter-rail').getByRole('button', { name: 'Build bundle', exact: true }).click()
  const guard = page.getByRole('alertdialog')
  await expect(guard).toContainText('Save changes before leaving?')
  await expect(guard).toContainText('1 metadata edit has not been saved')
  await page.screenshot({ path: info.outputPath('06C-1440-light.png'), animations: 'disabled' })
  await guard.getByRole('button', { name: 'Keep editing' }).click()
  await expect(title).toHaveValue('Unsaved synthetic title')
  await page.locator('.matter-rail').getByRole('button', { name: 'Build bundle', exact: true }).click()
  await guard.getByRole('button', { name: 'Save changes' }).click()
  await expect(page.locator('.build-workspace')).toBeVisible()
  const saved = (await (await request.get(`${base}/matters/${matter.id}/documents`, { headers })).json())[0]
  expect(saved.title).toBe('Unsaved synthetic title')
  expect(saved.source_sha256).toBe(doc.source_sha256)
})
