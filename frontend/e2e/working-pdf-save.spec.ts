import { openDocumentActions, openDocumentBrowser } from './navigation'
import { expect, test } from '@playwright/test'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

const backend = 'http://127.0.0.1:8768'
const headers = { Authorization: 'Bearer e2e-isolated-token' }

test('Save copy downloads exact revision bytes without private notes and refuses stale revisions', async ({ page, request }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Exact Working Save', court_profile: 'general-demo', max_pages_per_volume: 100,
  } })
  const matterId = (await created.json()).id as string
  const original = readFileSync(resolve(import.meta.dirname, '../../samples/01_petition.pdf'))
  const imported = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: 'save-working-synthetic.pdf', mimeType: 'application/pdf', buffer: original } } })
  expect(imported.ok()).toBeTruthy()
  const document = (await imported.json())[0]
  const annotated = await request.post(`${backend}/documents/${document.id}/annotations`, { headers, data: {
    kind: 'comment', page_index: 0, expected_revision: 1, rect_x: .1, rect_y: .2, rect_width: .1, rect_height: .05,
    body: 'PRIVATE SAVE TEST NOTE MUST STAY IN WORKSPACE',
  } })
  expect(annotated.ok()).toBeTruthy()
  async function history() {
    return await (await request.get(`${backend}/matters/${matterId}/history`, { headers })).json()
  }
  const before = await history()
  const firstBytes = await (await request.get(`${backend}/documents/${document.id}/pdf?revision=1`, { headers })).body()
  expect(firstBytes).toEqual(original)
  await page.setViewportSize({ width: 1440, height: 1000 })
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Exact Working Save'})).toBeVisible()
  await openDocumentBrowser(page)
  await page.locator('.doc-item').filter({ hasText: 'save-working-synthetic' }).click()
  await openDocumentActions(page)
  const save = page.getByRole('button', { name: 'Save working copy', exact: true })
  await expect(save).toBeEnabled()
  let downloadCount = 0
  page.on('download', () => downloadCount++)
  const firstRequest = page.waitForRequest(item => item.url().includes(`/documents/${document.id}/pdf?revision=1&editable=true`))
  const firstDownload = page.waitForEvent('download')
  await openDocumentActions(page)
  await save.click()
  expect((await firstRequest).method()).toBe('GET')
  const downloaded = await firstDownload
  expect(downloaded.suggestedFilename()).toBe('save-working-synthetic-working-r1.pdf')
  const downloadedPath = await downloaded.path()
  expect(downloadedPath).not.toBeNull()
  expect(readFileSync(downloadedPath!)).toEqual(firstBytes)
  await expect(page.getByRole('status').filter({ hasText: 'Download started for' })).toContainText('working revision 1')
  await expect(page.getByRole('status').filter({ hasText: 'Download started for' })).toContainText('Private notes are omitted')
  expect(await history()).toEqual(before)
  await page.locator('.viewer-root').screenshot({ path: test.info().outputPath('save-working-copy.png') })

  // Another client changes the working revision while this reader still shows r1.
  const rotated = await request.post(`${backend}/documents/${document.id}/pages/rotate`, { headers,
    data: { page_index: 0, degrees: 90, expected_revision: 1 } })
  expect(rotated.ok()).toBeTruthy()
  const conflict = page.waitForResponse(item => item.url().includes(`/documents/${document.id}/pdf?revision=1&editable=true`) && item.status() === 409)
  await openDocumentActions(page)
  await save.click()
  await conflict
  await expect(page.getByRole('alert').filter({ hasText: /revision changed/i })).toBeVisible()
  expect(downloadCount).toBe(1)
  expect(readFileSync(downloadedPath!)).toEqual(firstBytes)

  // Refresh to review r2; saving now must use exactly r2, not the old bytes.
  await page.reload()
  await openDocumentActions(page)
  await expect(save).toBeEnabled()
  const secondBytes = await (await request.get(`${backend}/documents/${document.id}/pdf?revision=2`, { headers })).body()
  expect(secondBytes).not.toEqual(firstBytes)
  const beforeSecondSave = await history()
  const secondRequest = page.waitForRequest(item => item.url().includes(`/documents/${document.id}/pdf?revision=2&editable=true`))
  const secondDownload = page.waitForEvent('download')
  await openDocumentActions(page)
  await save.click()
  await secondRequest
  const secondSaved = await secondDownload
  expect(secondSaved.suggestedFilename()).toBe('save-working-synthetic-working-r2.pdf')
  expect(readFileSync((await secondSaved.path())!)).toEqual(secondBytes)
  expect(await history()).toEqual(beforeSecondSave)
  expect(errors).toEqual([])
})
