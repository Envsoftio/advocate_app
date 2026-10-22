import { expect, test } from '@playwright/test'
import { execFileSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { resolve } from 'node:path'

const backend = 'http://127.0.0.1:8768'
const headers = { Authorization: 'Bearer e2e-isolated-token' }
const backendRoot = resolve(import.meta.dirname, '../../backend')
const python = process.env.COURTBUNDLE_TEST_PYTHON || [
  resolve(backendRoot, '.venv312/bin/python'), resolve(backendRoot, '.venv/bin/python'),
].find(existsSync) || 'python3'

function syntheticPdf() {
  return execFileSync(python, ['-c', `import pymupdf,sys
with pymupdf.open() as pdf:
 for index in range(30):
  page=pdf.new_page(width=420,height=595)
  page.insert_text((36,70),f'Organizer synthetic page {index+1}',fontsize=16)
 sys.stdout.buffer.write(pdf.tobytes())
`])
}

test('page organizer bounds previews, preserves explicit selections, and applies one undoable action', async ({ page, request }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Page Organizer', court_profile: 'general-demo', max_pages_per_volume: 100,
  } })
  expect(created.ok()).toBeTruthy()
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents`, { headers,
    multipart: { files: { name: 'organizer-synthetic.pdf', mimeType: 'application/pdf', buffer: syntheticPdf() } },
  })
  expect(uploaded.ok()).toBeTruthy()
  const document = (await uploaded.json())[0]
  async function history() {
    return (await (await request.get(`${backend}/matters/${matterId}/history`, { headers })).json()).undo_count as number
  }
  async function state() {
    const response = await request.get(`${backend}/documents/${document.id}/pdf`, { headers })
    return JSON.parse(execFileSync(python, ['-c', `import pymupdf,sys,json
with pymupdf.open(stream=sys.stdin.buffer.read(),filetype='pdf') as pdf:
 print(json.dumps([{'text':page.get_text().strip(),'rotation':page.rotation} for page in pdf]))
`], { input: await response.body(), encoding: 'utf8' })) as { text: string, rotation: number }[]
  }
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'E2E Page Organizer' })).toBeVisible()
  await page.getByRole('tab', { name: 'Pages', exact: true }).click()
  await page.getByRole('button', { name: 'Organize pages', exact: true }).click()
  const organizer = page.getByRole('dialog', { name: 'Organize pages' })
  await expect(organizer).toBeVisible()
  await expect(organizer.getByRole('button', { name: 'Next page grid' })).toBeEnabled()
  await expect(organizer.locator('canvas')).toHaveCount(24)
  await expect(organizer.locator('.thumbnail-ready').first()).toBeVisible()
  await organizer.screenshot({ path: test.info().outputPath('page-organizer.png') })
  await organizer.getByRole('checkbox', { name: 'Select page 2', exact: true }).click()
  await organizer.getByRole('checkbox', { name: 'Select page 5', exact: true }).click({ modifiers: ['Shift'] })
  await expect(organizer.getByText('4 of 30 selected', { exact: true })).toBeVisible()
  await organizer.getByLabel('Select page ranges', { exact: true }).fill('1-2, 25-26')
  await organizer.getByRole('button', { name: 'Select ranges', exact: true }).click()
  await organizer.getByRole('button', { name: 'Next page grid' }).click()
  await expect(organizer.locator('canvas')).toHaveCount(6)
  await expect(organizer.getByRole('checkbox', { name: 'Select page 25', exact: true })).toBeChecked()
  await expect(organizer.getByRole('checkbox', { name: 'Select page 26', exact: true })).toBeChecked()
  await expect(organizer.getByText('4 of 30 selected', { exact: true })).toBeVisible()
  const before = await history()
  await organizer.getByRole('button', { name: 'Rotate right', exact: true }).click()
  await expect(organizer.getByRole('region', { name: 'Review page action' })).toContainText('Pages 1–2, 25–26')
  await organizer.getByRole('button', { name: 'Rotate 4 pages', exact: true }).click()
  await expect(organizer.getByRole('status')).toContainText('4 pages rotated')
  expect(await history()).toBe(before + 1)
  expect((await state()).map(item => item.rotation)).toEqual(Array.from({ length: 30 }, (_, index) => [0, 1, 24, 25].includes(index) ? 90 : 0))
  await organizer.getByRole('button', { name: 'Done', exact: true }).click()
  await page.getByRole('button', { name: 'Undo last matter change', exact: true }).click()
  await expect.poll(async () => (await state()).every(item => item.rotation === 0)).toBeTruthy()

  await page.getByRole('button', { name: 'Organize pages', exact: true }).click()
  await expect(organizer.getByRole('button', { name: 'Move page 1 later', exact: true })).toBeEnabled()
  const beforePreview = await history()
  await organizer.getByRole('button', { name: 'Move page 1 later', exact: true }).click()
  await expect(organizer.locator('.organizer-page').first()).toHaveAttribute('data-page', '1')
  await expect(organizer.getByRole('button', { name: 'Apply page order', exact: true })).toBeEnabled()
  expect(await history()).toBe(beforePreview)
  page.once('dialog', prompt => prompt.accept())
  await organizer.getByRole('button', { name: 'Close page organizer' }).click()
  expect((await state())[0].text).toBe('Organizer synthetic page 1')

  await page.getByRole('button', { name: 'Organize pages', exact: true }).click()
  await expect(organizer.getByRole('button', { name: 'Move page 1 later', exact: true })).toBeEnabled()
  await organizer.getByRole('button', { name: 'Move page 1 later', exact: true }).click()
  await organizer.getByRole('button', { name: 'Apply page order', exact: true }).click()
  await organizer.getByRole('region', { name: 'Review page action' }).getByRole('button', { name: 'Apply page order', exact: true }).click()
  await expect(organizer.getByRole('status')).toContainText('Page order applied')
  expect((await state()).slice(0, 3).map(item => item.text)).toEqual([
    'Organizer synthetic page 2', 'Organizer synthetic page 1', 'Organizer synthetic page 3',
  ])
  expect(await history()).toBe(beforePreview + 1)
  await organizer.getByRole('button', { name: 'Select all', exact: true }).click()
  await expect(organizer.getByRole('button', { name: 'Delete selected', exact: true })).toBeDisabled()
  await organizer.getByLabel('Select page ranges', { exact: true }).fill('3-4')
  await organizer.getByRole('button', { name: 'Select ranges', exact: true }).click()
  await organizer.getByRole('button', { name: 'Delete selected', exact: true }).click()
  await organizer.getByRole('button', { name: 'Delete 2 pages', exact: true }).click()
  await expect(organizer.getByRole('status')).toContainText('2 pages deleted')
  expect(await state()).toHaveLength(28)
  expect(await history()).toBe(beforePreview + 2)
  await organizer.getByRole('button', { name: 'Done', exact: true }).click()
  await page.getByRole('button', { name: 'Undo last matter change', exact: true }).click()
  await expect.poll(async () => (await state()).length).toBe(30)
  expect(errors).toEqual([])
})
