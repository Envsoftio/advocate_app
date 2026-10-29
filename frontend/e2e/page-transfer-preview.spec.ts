import { closeWorkspaceNavigation, openDocumentBrowser } from './navigation'
import { expect, test } from '@playwright/test'
import { execFileSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { resolve } from 'node:path'

const backend = 'http://127.0.0.1:8768'
const headers = { Authorization: 'Bearer e2e-isolated-token' }
const backendRoot = resolve(import.meta.dirname, '../../backend')
const python = process.env.MATTERDRAFT_TEST_PYTHON || [
  resolve(backendRoot, '.venv312/bin/python'), resolve(backendRoot, '.venv/bin/python'),
].find(existsSync) || 'python3'
function syntheticPdf(label: string) {
  return execFileSync(python, ['-c', `import pymupdf,sys
with pymupdf.open() as pdf:
 for index in range(3):
  page=pdf.new_page(width=420,height=595)
  page.insert_text((36,70),f'{sys.argv[1]} page {index+1}',fontsize=16)
  if sys.argv[1]=='SOURCE' and index==1: page.set_rotation(90)
 sys.stdout.buffer.write(pdf.tobytes())
`, label])
}
test('page transfer preview shows actual source and destination pages and revision-bound result order', async ({ page, request }) => {
  await page.setViewportSize({width:1440,height:1000})
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Visual Page Transfer', court_profile: 'general-demo', max_pages_per_volume: 100,
  } })
  const matterId = (await created.json()).id as string
  async function upload(name: string, label: string) {
    const response = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
      multipart: { files: { name, mimeType: 'application/pdf', buffer: syntheticPdf(label) } } })
    expect(response.ok()).toBeTruthy()
    return (await response.json())[0]
  }
  const target = await upload('transfer-target.pdf', 'TARGET')
  const source = await upload('transfer-source.pdf', 'SOURCE')
  const initialSource = await (await request.get(`${backend}/documents/${source.id}/pdf`, { headers })).body()
  async function targetText() {
    const response = await request.get(`${backend}/documents/${target.id}/pdf`, { headers })
    return JSON.parse(execFileSync(python, ['-c', `import pymupdf,sys,json
with pymupdf.open(stream=sys.stdin.buffer.read(),filetype='pdf') as pdf:
 print(json.dumps([page.get_text().strip() for page in pdf]))
`], { input: await response.body(), encoding: 'utf8' })) as string[]
  }
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Visual Page Transfer'})).toBeVisible()
  await openDocumentBrowser(page)
  await page.locator('.doc-item').filter({ hasText: 'transfer-target' }).click()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', { name: 'Pages', exact: true }).click()
  await page.getByRole('tab', { name: 'Pages', exact: true }).click()
  await page.getByRole('button', { name: 'Add pages', exact: true }).click()
  const tools = page.getByRole('dialog', { name: 'Page tools' })
  await expect(tools.getByRole('heading', { name: 'Add pages' })).toBeVisible()
  const preview = tools.getByRole('region', { name: 'Page transfer preview' })
  await tools.getByRole('combobox', { name: 'Source document' }).selectOption(source.id)
  await tools.getByRole('textbox', { name: 'Source pages' }).fill('2,1')
  await tools.getByRole('combobox', { name: 'Where to add pages' }).selectOption('after')
  await tools.getByRole('spinbutton', { name: 'Insert after page' }).fill('2')
  await expect(preview).toContainText('Insert between current pages 2 and 3')
  await expect(preview).toContainText('Result order: current pages 1–2 → source pages 2, 1 → current pages 3.')
  await expect(tools.getByRole('button', { name: 'Add selected pages', exact: true })).toBeEnabled()
  await expect(preview.locator('canvas')).toHaveCount(3)
  const sourcePreview = preview.getByRole('article', { name: 'Pages to add: page 2 of transfer-source' })
  const bounds = await sourcePreview.locator('.pdf-page-surface').boundingBox()
  expect(bounds).not.toBeNull()
  expect(bounds!.width / bounds!.height).toBeCloseTo(595 / 420, 1)
  await preview.screenshot({ path: test.info().outputPath('page-transfer-preview.png') })
  await preview.getByRole('button', { name: 'Next selected source page' }).click()
  await expect(preview.getByRole('article', { name: 'Pages to add: page 1 of transfer-source' })).toBeVisible()
  await expect(preview).toContainText('Result page 4')
  const submitted = page.waitForRequest(item => item.method()==='POST' && item.url().endsWith(`/documents/${target.id}/pages/insert`))
  await tools.getByRole('button', { name: 'Add selected pages', exact: true }).click()
  const payload = (await submitted).postDataJSON()
  expect(payload.expected_revision).toBe(1)
  expect(payload.expected_source_revision).toBe(1)
  expect(payload.source_pages).toEqual([1, 0])
  expect(payload.before_index).toBe(2)
  await expect(tools.getByRole('status')).toContainText('Working copy updated')
  expect(await targetText()).toEqual(['TARGET page 1', 'TARGET page 2', 'SOURCE page 2', 'SOURCE page 1', 'TARGET page 3'])
  expect(await (await request.get(`${backend}/documents/${source.id}/pdf`, { headers })).body()).toEqual(initialSource)

  await tools.getByRole('button', { name: 'Replace pages', exact: true }).click()
  await tools.getByRole('textbox', { name: 'Source pages' }).fill('1,3')
  await tools.getByRole('textbox', { name: 'Target pages to replace' }).fill('1,5')
  await expect(tools.getByRole('button', { name: 'Replace selected pages', exact: true })).toBeEnabled()
  await expect(preview.locator('canvas')).toHaveCount(2)
  await preview.locator('summary').click()
  await expect(preview).toContainText('Current page 5 ← source page 3. Result remains page 5.')
  await tools.getByRole('button', { name: 'Replace selected pages', exact: true }).click()
  await expect(tools.getByRole('status')).toContainText('Working copy updated')
  await expect.poll(targetText).toEqual(['SOURCE page 1', 'TARGET page 2', 'SOURCE page 2', 'SOURCE page 1', 'SOURCE page 3'])

  await tools.getByRole('button', { name: 'Combine PDFs', exact: true }).click()
  await expect(preview).toContainText('Append after current page 5')
  await expect(preview).toContainText('8 pages after applying')
  await expect(tools.getByRole('button', { name: 'Combine documents', exact: true })).toBeEnabled()
  await expect(preview.getByRole('article', { name: 'End of document', exact: true })).toBeVisible()
  await expect(preview.locator('canvas')).toHaveCount(2)
  await tools.getByRole('button', { name: 'Replace whole PDF', exact: true }).click()
  await expect(preview).toContainText('All 5 current working pages will be replaced by the 3 source pages, in source order.')
  await expect(tools.getByRole('button', { name: 'Replace entire working document', exact: true })).toBeEnabled()
  const changed = await request.post(`${backend}/documents/${source.id}/pages/rotate`, { headers,
    data: { page_index: 0, degrees: 90, expected_revision: 1 } })
  expect(changed.ok()).toBeTruthy()
  await tools.getByRole('button', { name: 'Replace entire working document', exact: true }).click()
  await expect(tools.getByRole('alert')).toContainText(/revision changed/i)
  expect(await targetText()).toEqual(['SOURCE page 1', 'TARGET page 2', 'SOURCE page 2', 'SOURCE page 1', 'SOURCE page 3'])
  await tools.getByRole('button', { name: 'Close page tools' }).click()
  await page.getByRole('button', { name: 'Add pages', exact: true }).click()
  await expect(tools.getByRole('heading', { name: 'Add pages' })).toBeVisible()
  expect(errors).toEqual([])
})
