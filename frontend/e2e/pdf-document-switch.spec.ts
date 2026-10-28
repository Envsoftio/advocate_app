import { openReaderView, closeWorkspaceNavigation, openDocumentActions, openDocumentBrowser } from './navigation'
import { expect, test, type Page } from '@playwright/test'
import { execFileSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { resolve } from 'node:path'

const backend = 'http://127.0.0.1:8768'
const headers = { Authorization: 'Bearer e2e-isolated-token' }
const backendRoot = resolve(import.meta.dirname, '../../backend')
const python = process.env.COURTBUNDLE_TEST_PYTHON || [
  resolve(backendRoot, '.venv312/bin/python'), resolve(backendRoot, '.venv/bin/python'),
].find(existsSync) || 'python3'
function fixture(label: string) {
  return execFileSync(python, ['-c', `import pymupdf,sys
with pymupdf.open() as pdf:
 for index in range(4):
  page=pdf.new_page(width=420,height=595)
  page.insert_text((36,70),f'{sys.argv[1]} page {index+1}',fontsize=16)
 pdf.set_toc([[1,sys.argv[1]+' outline',1]])
 sys.stdout.buffer.write(pdf.tobytes())
`, label])
}
async function holdPosition(page: Page, key: string) {
  let release!: () => void, observed!: () => void
  const gate = new Promise<void>(resolve => { release = resolve })
  const requested = new Promise<void>(resolve => { observed = resolve })
  await page.route('**/reader-position?*', async route => {
    if (new URL(route.request().url()).searchParams.get('key') !== key) return route.continue()
    observed()
    await gate
    await route.continue()
  })
  return { requested, release }
}

test('document switches clear old PDF content before delayed reading state and ignore late responses', async ({ page, request }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const matter = await (await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Delayed Document Switch', court_profile: 'general-demo',
  } })).json()
  const documents = []
  for (const label of ['ALPHA', 'BRAVO', 'CHARLIE']) {
    const uploaded = await request.post(`${backend}/matters/${matter.id}/documents?included=true`, { headers,
      multipart: { files: { name: `${label.toLowerCase()}-switch.pdf`, mimeType: 'application/pdf', buffer: fixture(label) } } })
    expect(uploaded.ok()).toBeTruthy()
    documents.push((await uploaded.json())[0])
  }
  const saved = await request.put(`${backend}/reader-position`, { headers, data: {
    key: `${documents[1].id}:1`, page: 3, x: 0, y: .15, zoom: '75', mode: 'continuous',
  } })
  expect(saved.ok()).toBeTruthy()
  await page.setViewportSize({ width: 1440, height: 1000 })
  await page.goto('/')
  await openDocumentBrowser(page)
  await page.locator('.doc-item').filter({ hasText: 'alpha-switch' }).click()
  await expect(page.locator('.pdf-text-layer')).toContainText('ALPHA page 1')
  await expect(page.locator('.thumbnail-strip canvas[data-rendered="true"]').first()).toBeVisible()
  await page.getByRole('button', { name: 'Document bookmarks', exact: true }).click()
  await expect(page.getByRole('button', { name: 'ALPHA outline · p1', exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Search', exact: true }).click()
  await page.getByRole('searchbox', { name: 'Find text in this PDF' }).fill('ALPHA')
  await expect(page.locator('.pdf-findbar')).toContainText('4 matches')

  const bravo = await holdPosition(page, `${documents[1].id}:1`)
  await openDocumentBrowser(page)
  await page.locator('.doc-item').filter({ hasText: 'bravo-switch' }).click()
  await bravo.requested
  await expect(page.locator('.reader-document strong')).toHaveText('bravo-switch')
  await expect(page.getByRole('status').filter({ hasText: 'Loading PDF' })).toBeVisible()
  await expect(page.locator('.pdf-find-results')).toHaveCount(0)
  await expect(page.locator('.pdf-findbar')).toContainText('0 matches')
  await expect(page.getByRole('searchbox', { name: 'Find text in this PDF' })).toBeDisabled()
  await page.getByRole('button', { name: 'Document bookmarks', exact: true }).click()
  await expect(page.getByRole('region', { name: 'Source PDF outline' })).toContainText('Loading PDF outline')
  await expect(page.getByRole('region', { name: 'Source PDF outline' })).not.toContainText('No outline')
  await page.getByRole('button', { name: 'Search', exact: true }).click()
  await expect(page.getByRole('button', { name: 'ALPHA outline · p1', exact: true })).toHaveCount(0)
  await expect(page.locator('.pdf-text-layer span')).toHaveCount(0)
  await expect(page.locator('.thumbnail-strip canvas[data-rendered="true"]')).toHaveCount(0)
  await expect.poll(() => page.locator('.viewer-root canvas').evaluateAll(items =>
    items.every(item => (item as HTMLCanvasElement).width * (item as HTMLCanvasElement).height <= 1))).toBeTruthy()
  await openDocumentActions(page)
  await expect(page.getByRole('button', { name: 'Save working copy', exact: true })).toBeDisabled()
  bravo.release()
  await expect(page.getByRole('spinbutton', { name: 'Go to page', exact: true })).toHaveValue('3')
  await expect(page.getByRole('combobox', { name: 'Zoom', exact: true })).toHaveValue('75')
  await openReaderView(page)
  await expect(page.getByRole('combobox', { name: 'Reading mode' })).toHaveValue('continuous')
  await page.getByRole('button',{name:'View options'}).click()
  await expect(page.locator('[data-reader-page="3"] .pdf-text-layer')).toContainText('BRAVO page 3')

  // Return to ALPHA while a third document's metadata is pending. Its late
  // response must not reopen CHARLIE or restore CHARLIE's navigation state.
  const charlie = await holdPosition(page, `${documents[2].id}:1`)
  let charliePdfRequests = 0
  page.on('request', item => { if (item.url().includes(`/documents/${documents[2].id}/pdf?`)) charliePdfRequests++ })
  await openDocumentBrowser(page)
  await page.locator('.doc-item').filter({ hasText: 'charlie-switch' }).click()
  await charlie.requested
  await expect(page.locator('.pdf-text-layer span')).toHaveCount(0)
  await openDocumentBrowser(page)
  await page.locator('.doc-item').filter({ hasText: 'alpha-switch' }).click()
  await expect(page.locator('.reader-document strong')).toHaveText('alpha-switch')
  await expect(page.locator('.pdf-text-layer').first()).toContainText('ALPHA')
  const lateResponse = page.waitForResponse(item => item.url().includes('/reader-position?') &&
    new URL(item.url()).searchParams.get('key') === `${documents[2].id}:1`)
  charlie.release()
  await lateResponse
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))
  expect(charliePdfRequests).toBe(0)
  await expect(page.locator('.reader-document strong')).toHaveText('alpha-switch')
  await expect(page.locator('.pdf-text-layer').first()).toContainText('ALPHA')
  expect(errors).toEqual([])
})

test('same-document text edits retain their snapshot during delayed reading-state restoration', async ({ page, request }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const matter = await (await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Delayed Edit Refresh', court_profile: 'general-demo',
  } })).json()
  const document = (await (await request.post(`${backend}/matters/${matter.id}/documents?included=true`, { headers,
    multipart: { files: { name: 'snapshot-refresh.pdf', mimeType: 'application/pdf', buffer: fixture('BEFORE') } } })).json())[0]
  await page.goto('/')
  await openDocumentBrowser(page)
  await page.locator('.doc-item').filter({ hasText: 'snapshot-refresh' }).click()
  await expect(page.locator('.pdf-text-layer')).toContainText('BEFORE page 1')
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', { name: 'Edit', exact: true }).click()
  await page.getByRole('button', { name: 'Edit text', exact: true }).click()
  await page.getByRole('button', { name: 'Edit text: BEFORE page 1', exact: true }).click()
  await page.getByRole('textbox', { name: 'Replacement' }).fill('AFTER page 1')
  const position = await holdPosition(page, `${document.id}:2`)
  await page.getByRole('button', { name: 'Apply replacement', exact: true }).click()
  await position.requested
  await expect(page.locator('.pdf-edit-snapshot')).toBeVisible()
  await expect(page.locator('.pdf-placement-surface')).toHaveClass(/is-edit-refreshing/)
  await expect(page.locator('.pdf-text-layer span')).toHaveCount(0)
  position.release()
  await expect(page.locator('.pdf-text-layer')).toContainText('AFTER page 1')
  await expect(page.locator('.pdf-edit-snapshot')).toHaveCount(0)
  await expect(page.getByRole('spinbutton', { name: 'Go to page', exact: true })).toHaveValue('1')
  expect(errors).toEqual([])
})
