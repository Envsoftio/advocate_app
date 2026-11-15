import { expect, test } from '@playwright/test'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import { enterLatestMatter, openDocumentBrowser } from './navigation'

const base = `http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT || 8768}`
const headers = { Authorization: 'Bearer e2e-isolated-token' }

test('redaction review lists saved marks by source page and keeps the working PDF explicit', async ({ page, request }, info) => {
  const matter = await (await request.post(`${base}/matters`, { headers, data: { name: 'Synthetic redaction review', court_profile: 'general-demo' } })).json()
  const upload = await request.post(`${base}/matters/${matter.id}/documents?included=true`, { headers, multipart: {
    files: { name: '01_petition.pdf', mimeType: 'application/pdf', buffer: await readFile(resolve(import.meta.dirname, '../../samples/01_petition.pdf')) },
  } })
  expect(upload.ok()).toBe(true)
  const document = (await upload.json())[0]
  const mark = await request.post(`${base}/documents/${document.id}/annotations`, { headers, data: {
    kind: 'redaction', page_index: 0, expected_revision: document.revision,
    rect_x: .2, rect_y: .2, rect_width: .15, rect_height: .05,
    body: '', color: 'yellow', output_scope: 'pdf',
  } })
  expect(mark.ok()).toBe(true)
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto('/')
  await enterLatestMatter(page)
  await openDocumentBrowser(page)
  await page.locator('.doc-item').filter({ hasText: '01_petition' }).click()
  await page.getByRole('tab', { name: 'Redact' }).click()
  await expect(page.getByRole('button', { name: 'Mark 1 · Source 1' })).toBeVisible()
  await expect(page.getByLabel('Pending redaction marks')).toContainText('current working PDF still contains it')
  const geometry = await page.evaluate(() => ({
    canvas: document.querySelector('.viewer-page-column')!.getBoundingClientRect().toJSON(),
    inspector: document.querySelector('.viewer-tool-inspector')!.getBoundingClientRect().toJSON(),
  }))
  expect(Math.abs(geometry.canvas.right - geometry.inspector.left)).toBeLessThan(1)
  expect(geometry.inspector.width).toBe(292)
  await page.screenshot({ path: info.outputPath('redaction-review-1440.png'), animations: 'disabled' })
  await page.setViewportSize({ width: 1024, height: 768 })
  await expect(page.getByLabel('Pending redaction marks')).toBeVisible()
  await expect(page.locator('.thumbnail-button canvas.is-rendered').first()).toBeVisible()
  await page.screenshot({ path: info.outputPath('redaction-review-1024.png'), animations: 'disabled' })
})
