import { expect, test } from '@playwright/test'
import { createHash } from 'node:crypto'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'

const samples = resolve(import.meta.dirname, '../../samples')
const backend = 'http://127.0.0.1:8768'
const headers = { Authorization: 'Bearer e2e-isolated-token' }

test('unavailable engine shows a bounded error and reconnects', async ({ page }) => {
  await page.route('**/health', route => route.abort())
  await page.goto('/')
  await expect(page.getByText('Engine disconnected')).toBeVisible({ timeout: 20_000 })
  await expect(page.getByText('Python engine unavailable')).toBeVisible()
  await page.unroute('**/health')
  await page.getByRole('button', { name: 'Retry connection' }).click()
  await expect(page.getByText('Local engine connected')).toBeVisible()
})

test('synthetic browser journey, retained export and download bytes', async ({ page, request }) => {
  page.setDefaultTimeout(10_000)
  await page.goto('/')
  await expect(page.getByText('Local engine connected')).toBeVisible()
  await page.getByRole('button', { name: 'Create your first matter' }).click()
  await page.getByRole('dialog', { name: 'Create matter' }).getByPlaceholder('Example: ABC Pvt Ltd v. XYZ').fill('E2E Synthetic Matter')
  await page.getByRole('dialog', { name: 'Create matter' }).getByRole('button', { name: 'Create matter' }).click()
  await expect(page.getByRole('heading', { name: 'E2E Synthetic Matter' })).toBeVisible()

  await page.locator('input[accept=".pdf,.jpg,.jpeg,.png"]').setInputFiles([
    resolve(samples, '01_petition.pdf'),
    resolve(samples, '02_impugned_order.pdf'),
    resolve(samples, '03_affidavit.pdf'),
    resolve(samples, '04_scanned_annexure.jpg'),
  ])
  await expect(page.locator('.doc-item')).toHaveCount(4)
  await expect(page.locator('.page-counter')).toContainText('1 / 3')
  const canvas = page.locator('.pdf-paper')
  await expect.poll(() => canvas.evaluate((node: HTMLCanvasElement) => node.width)).toBeGreaterThan(0)
  const before = await canvas.evaluate((node: HTMLCanvasElement) => `${node.width}x${node.height}`)
  await page.getByRole('button', { name: 'Rotate' }).click()
  await expect.poll(() => canvas.evaluate((node: HTMLCanvasElement) => `${node.width}x${node.height}`)).not.toBe(before)
  await page.locator('button[title="Move page right"]').click()
  page.once('dialog', dialog => void dialog.accept())
  await page.locator('button[title^="Delete page"]').click()
  await expect(page.locator('.page-counter')).toContainText('1 / 2')

  await page.locator('.doc-item', { hasText: '04_scanned_annexure' }).click()
  await expect(page.locator('.page-counter')).toContainText('1 / 1')
  await page.getByRole('button', { name: 'Run English OCR' }).click()
  await expect(page.locator('.inspector-heading')).toContainText('processed', { timeout: 90_000 })
  await page.getByRole('textbox', { name: 'Display title' }).fill('Synthetic OCR Annexure')
  await page.getByRole('button', { name: 'Save document details' }).click()
  await expect(page.getByText('Document details updated.')).toBeVisible()
  await page.reload()
  await expect(page.locator('.doc-item')).toHaveCount(4)
  await page.locator('.doc-item', { hasText: 'Synthetic OCR Annexure' }).click()
  await expect(page.locator('.inspector-heading')).toContainText('processed')

  await page.getByRole('button', { name: 'Build bundle', exact: true }).click()
  await page.getByRole('spinbutton').fill('6')
  await page.locator('.build-panel').getByRole('button', { name: 'Build filing bundle' }).click()
  await expect(page.getByText('Latest filing package')).toBeVisible({ timeout: 90_000 })

  const matterResponse = await request.get(`${backend}/matters`, { headers })
  expect(matterResponse.ok()).toBeTruthy()
  const matterId = (await matterResponse.json())[0].id as string
  const exportResponse = await request.get(`${backend}/matters/${matterId}/exports`, { headers })
  expect(exportResponse.ok()).toBeTruthy()
  const exportId = (await exportResponse.json())[0].id as string
  const stored = await request.get(`${backend}/matters/${matterId}/exports/${exportId}/filing-package.zip`, { headers })
  expect(stored.ok()).toBeTruthy()
  const downloadPromise = page.waitForEvent('download')
  await page.getByRole('button', { name: 'Download filing-package.zip' }).click()
  const downloaded = await downloadPromise
  const bytes = await readFile(await downloaded.path())
  expect(bytes.subarray(0, 2).toString()).toBe('PK')
  expect(createHash('sha256').update(bytes).digest('hex'))
    .toBe(createHash('sha256').update(await stored.body()).digest('hex'))

  await page.getByRole('spinbutton').fill('2')
  await page.locator('.build-panel').getByRole('button', { name: 'Build filing bundle' }).click()
  await expect(page.getByText('Previous filing package')).toBeVisible({ timeout: 90_000 })
  await expect(page.locator('.notice.error')).toContainText('exceeds', { timeout: 90_000 })
  const retained = await request.get(`${backend}/matters/${matterId}/exports`, { headers })
  expect((await retained.json()).map((item: {id: string}) => item.id)).toEqual([exportId])
})

test('Memo of Parties autosaves per matter and reports a failed save', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Memo Matter', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  expect(created.ok()).toBeTruthy()
  const id = (await created.json()).id as string
  await page.goto('/')
  await expect(page.getByText('Local engine connected')).toBeVisible()
  await page.getByRole('button', { name: 'Court rules' }).click()
  const helper = page.locator('.memo-helper')
  await expect(helper.getByRole('status')).toContainText('Saved to this matter')
  await helper.getByRole('textbox', { name: 'Party name' }).fill('Synthetic Saved Party')
  await expect(helper.getByRole('status')).toContainText('Saved to this matter')
  const stored = await request.get(`${backend}/matters/${id}/memo-parties`, { headers })
  expect((await stored.json()).parties[0].name).toBe('Synthetic Saved Party')
  await page.reload()
  await page.getByRole('button', { name: 'Court rules' }).click()
  await expect(page.locator('.memo-helper').getByRole('textbox', { name: 'Party name' }))
    .toHaveValue('Synthetic Saved Party')

  await page.route(`**/matters/${id}/memo-parties`, route =>
    route.request().method() === 'PUT' ? route.abort() : route.continue())
  await page.locator('.memo-helper').getByRole('textbox', { name: 'Party name' })
    .fill('Synthetic Retried Party')
  await expect(page.locator('.memo-helper').getByRole('status')).toContainText('Save failed', { timeout: 10_000 })
  await page.unroute(`**/matters/${id}/memo-parties`)
  await page.locator('.memo-helper').getByRole('button', { name: 'Retry save' }).click()
  await expect(page.locator('.memo-helper').getByRole('status')).toContainText('Saved to this matter')
  const retried = await request.get(`${backend}/matters/${id}/memo-parties`, { headers })
  expect((await retried.json()).parties[0].name).toBe('Synthetic Retried Party')
})

test('unsaved details are guarded and a failed settings save stops a build', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Draft Guard', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const id = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${id}/documents`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'E2E Draft Guard' })).toBeVisible()
  await page.getByRole('textbox', { name: 'Display title' }).fill('Unsaved synthetic title')
  await expect(page.getByText('Unsaved document changes')).toBeVisible()
  page.once('dialog', dialog => void dialog.dismiss())
  await page.getByRole('button', { name: 'Build bundle', exact: true }).click()
  await expect(page.getByText('Unsaved document changes')).toBeVisible()
  page.once('dialog', dialog => void dialog.accept())
  await page.getByRole('button', { name: 'Build bundle', exact: true }).click()
  await expect(page.getByText('Unsaved document changes')).toBeHidden()
  await page.getByRole('spinbutton').fill('6')
  await expect(page.getByText('Unsaved bundle settings.')).toBeVisible()
  await page.route(`**/matters/${id}`, route =>
    route.request().method() === 'PATCH' ? route.abort() : route.continue())
  await page.locator('.build-panel').getByRole('button', { name: 'Build filing bundle' }).click()
  await expect(page.getByText('No exports yet')).toBeVisible()
  const jobs = await request.get(`${backend}/matters/${id}/jobs`, { headers })
  expect(await jobs.json()).toEqual([])
})

test('portable matter backup restores a separate local copy', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Portable Matter', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  const docId = (await uploaded.json())[0].id as string
  expect((await request.post(`${backend}/documents/${docId}/pages/rotate`, { headers,
    data: { page_index: 0, degrees: 90 } })).ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'E2E Portable Matter' })).toBeVisible()
  const pendingDownload = page.waitForEvent('download')
  await page.getByRole('button', { name: 'Back up matter' }).click()
  const download = await pendingDownload
  const packagePath = await download.path()
  expect(packagePath).toBeTruthy()
  await page.locator('input[accept=".cbmatter"]').setInputFiles(packagePath!)
  await expect(page.getByText('Matter restored with originals and working revisions.')).toBeVisible()
  const matters = await request.get(`${backend}/matters`, { headers })
  const copies = (await matters.json()).filter((item: {name: string}) => item.name === 'E2E Portable Matter')
  expect(copies).toHaveLength(2)
  const restoredId = copies.find((item: {id: string}) => item.id !== matterId).id as string
  const documents = await request.get(`${backend}/matters/${restoredId}/documents`, { headers })
  expect((await documents.json())[0].revision).toBe(2)
})

test('archived document and matter remain recoverable', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Archive Matter', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const id = (await created.json()).id as string
  await request.post(`${backend}/matters/${id}/documents`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'E2E Archive Matter' })).toBeVisible()
  page.once('dialog', dialog => void dialog.accept())
  await page.getByRole('button', { name: 'Archive document' }).click()
  await expect(page.locator('.doc-item')).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Restore', exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Restore', exact: true }).click()
  await expect(page.locator('.doc-item')).toHaveCount(1)
  page.once('dialog', dialog => void dialog.accept())
  await page.getByRole('button', { name: 'Archive matter', exact: true }).click()
  await expect(page.locator('button[title="Restore E2E Archive Matter"]')).toBeVisible()
  await page.locator('button[title="Restore E2E Archive Matter"]').click()
  await expect(page.getByRole('heading', { name: 'E2E Archive Matter' })).toBeVisible()
  await expect(page.locator('.doc-item')).toHaveCount(1)
})

test('undo and redo metadata persist across browser reload', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E History Matter', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const id = (await created.json()).id as string
  await request.post(`${backend}/matters/${id}/documents`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'E2E History Matter' })).toBeVisible()
  await page.getByRole('textbox', { name: 'Display title' }).fill('Changed title')
  await page.getByRole('button', { name: 'Save document details' }).click()
  await expect(page.locator('.doc-item')).toContainText('Changed title')
  await page.getByRole('button', { name: 'Undo', exact: true }).click()
  await expect(page.locator('.doc-item')).toContainText('01_petition')
  await page.reload()
  await expect(page.getByRole('button', { name: 'Redo', exact: true })).toBeEnabled()
  await page.getByRole('button', { name: 'Redo', exact: true }).click()
  await expect(page.locator('.doc-item')).toContainText('Changed title')
})
