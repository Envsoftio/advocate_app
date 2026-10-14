import { expect, test } from '@playwright/test'
import { createHash } from 'node:crypto'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'

const samples = resolve(import.meta.dirname, '../../samples')
const backend = 'http://127.0.0.1:8768'
const headers = { Authorization: 'Bearer e2e-isolated-token' }

test('compression exposes measured results and retains a usable PDF', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Compression', court_profile: 'general-demo', max_pages_per_volume: 300,
  } })
  const id = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${id}/documents`, { headers,
    multipart: { files: { name: '04_scanned_annexure.jpg', mimeType: 'image/jpeg',
      buffer: await readFile(resolve(samples, '04_scanned_annexure.jpg')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  const doc = (await uploaded.json())[0]
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'E2E Compression' })).toBeVisible()
  await page.getByRole('button', { name: 'Page tools', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Page tools' })
  await dialog.getByRole('combobox', { name: 'Compression quality' }).selectOption('small')
  await dialog.getByRole('button', { name: 'Compress working PDF' }).click()
  await expect(dialog.getByRole('status')).toContainText('bytes.')
  await expect(dialog.getByRole('status')).toContainText('Completed')
  await expect(dialog.getByRole('button', { name: 'Compress working PDF' })).toBeEnabled()
  const pdf = await request.get(`${backend}/documents/${doc.id}/pdf`, { headers })
  expect(pdf.ok()).toBeTruthy()
  expect((await pdf.body()).subarray(0, 5).toString()).toBe('%PDF-')
})

test('document import undo and redo refresh the workspace list', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Import History', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'E2E Import History' })).toBeVisible()
  await expect(page.locator('.doc-item')).toHaveCount(1)
  await page.getByRole('button', { name: 'Undo', exact: true }).click()
  await expect(page.locator('.doc-item')).toHaveCount(0)
  await page.reload()
  await expect(page.getByRole('button', { name: 'Redo', exact: true })).toBeEnabled()
  await page.getByRole('button', { name: 'Redo', exact: true }).click()
  await expect(page.locator('.doc-item')).toHaveCount(1)
})

test('manual bookmark survives reload and shows revision review', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Bookmark Matter', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  const docId = (await uploaded.json())[0].id as string
  await page.goto('/')
  await page.getByRole('button', { name: 'Build bundle', exact: true }).click()
  await page.getByRole('textbox', { name: 'New bookmark title' }).fill('Reviewed synthetic page')
  await page.getByRole('combobox', { name: 'Bookmark document' }).selectOption(docId)
  await page.getByRole('button', { name: 'Add bookmark' }).click()
  await expect(page.getByRole('textbox', { name: 'Bookmark title 1' })).toHaveValue('Reviewed synthetic page')
  await page.reload()
  await page.getByRole('button', { name: 'Build bundle', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Bookmark title 1' })).toHaveValue('Reviewed synthetic page')
  const rotated = await request.post(`${backend}/documents/${docId}/pages/rotate`,
    { headers, data: { page_index: 0, degrees: 90 } })
  expect(rotated.ok()).toBeTruthy()
  await page.reload()
  await page.getByRole('button', { name: 'Build bundle', exact: true }).click()
  await expect(page.getByText('Needs review')).toBeVisible()
  await page.getByRole('button', { name: 'Resolve to current page' }).click()
  await expect(page.getByText('Needs review')).toBeHidden()
})

test('text editing works on a page already marked as rotated', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Rotated Text', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  const docId = (await uploaded.json())[0].id as string
  const rotated = await request.post(`${backend}/documents/${docId}/pages/rotate`, { headers,
    data: { page_index: 0, degrees: 90 },
  })
  expect(rotated.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'E2E Rotated Text' })).toBeVisible()
  await page.getByRole('button', { name: 'Edit text', exact: true }).click()
  const run = page.getByRole('button', { name: /Edit text: DRAFT PETITION/ })
  await expect(run).toBeVisible()
  const canvas = await page.locator('.pdf-paper').boundingBox()
  const box = await run.boundingBox()
  expect(canvas && box).toBeTruthy()
  expect(box!.x).toBeGreaterThanOrEqual(canvas!.x)
  expect(box!.y).toBeGreaterThanOrEqual(canvas!.y)
  expect(box!.x + box!.width).toBeLessThanOrEqual(canvas!.x + canvas!.width + 1)
  expect(box!.y + box!.height).toBeLessThanOrEqual(canvas!.y + canvas!.height + 1)
  await run.click()
  await page.getByRole('textbox', { name: 'Replacement' }).fill('UPDATED PETITION')
  await page.getByRole('button', { name: 'Apply replacement' }).click()
  await expect(page.locator('.text-overlay-panel [role="status"]')).toContainText('Applied in')
})

test('selectable PDF text can be selected and replaced from the preview', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Replace Text', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  expect(created.ok()).toBeTruthy()
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'E2E Replace Text' })).toBeVisible()
  await page.getByRole('button', { name: 'Edit text', exact: true }).click()
  await page.getByRole('button', { name: /Edit text: DRAFT PETITION/ }).click()
  await expect(page.getByRole('textbox', { name: 'Selected text' })).toHaveValue('DRAFT PETITION - EXAMPLE ONLY')
  await page.getByRole('textbox', { name: 'Replacement' }).fill('UPDATED PETITION WITH MORE DETAILS')
  await page.getByRole('button', { name: 'Apply replacement' }).click()
  await expect(page.locator('.text-overlay-panel [role="status"]')).toContainText('Applied in')
  await expect.poll(async () => {
    const result = await request.get(`${backend}/matters/${matterId}/search?q=UPDATED%20PETITION%20WITH%20MORE%20DETAILS`, { headers })
    return (await result.json()).matches.length
  }).toBe(1)
  const old = await request.get(`${backend}/matters/${matterId}/search?q=DRAFT%20PETITION`, { headers })
  expect((await old.json()).matches.filter((match: {page:number}) => match.page === 1)).toHaveLength(0)
  await page.getByRole('button', { name: /Edit text: UPDATED PETITION/ }).click()
  await page.getByRole('textbox', { name: 'Replacement' }).fill('W'.repeat(200))
  await page.getByRole('button', { name: 'Apply replacement' }).click()
  await expect(page.locator('.text-overlay-panel [role="alert"]')).toContainText('cannot fit in clear space')
})

test('visible text overlay is placed from the preview and becomes searchable', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Text Overlay', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  expect(created.ok()).toBeTruthy()
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'E2E Text Overlay' })).toBeVisible()
  await page.getByRole('button', { name: 'Add text' }).click()
  await page.getByRole('textbox', { name: 'Text', exact: true }).fill('REVIEW OVERLAY')
  await expect(page.locator('.pdf-paper')).toBeVisible()
  await page.locator('.pdf-paper').click({ position: { x: 120, y: 180 } })
  await expect(page.getByRole('button', { name: 'Apply text' })).toBeEnabled()
  await page.getByRole('button', { name: 'Apply text' }).click()
  await expect(page.getByRole('textbox', { name: 'Text', exact: true })).toHaveValue('')
  await expect.poll(async () => {
    const result = await request.get(`${backend}/matters/${matterId}/search?q=REVIEW%20OVERLAY`, { headers })
    return (await result.json()).matches.length
  }).toBe(1)
})

test('appearance persists and keyboard search respects text editing', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Appearance Matter', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  expect(created.ok()).toBeTruthy()
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'E2E Appearance Matter' })).toBeVisible()
  await page.getByRole('combobox', { name: 'Theme' }).selectOption('dark')
  await page.getByRole('combobox', { name: 'Interface size' }).selectOption('125')
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark')
  await expect(page.locator('html')).toHaveAttribute('data-effective-theme', 'dark')
  await expect(page.locator('html')).toHaveCSS('zoom', '1.25')
  await expect(page.locator('.panel').first()).toHaveCSS('background-color', 'rgb(29, 41, 58)')
  await expect(page.locator('.upload-zone')).toHaveCSS('background-color', 'rgb(43, 60, 82)')
  await expect(page.locator('.canvas-scroll')).toHaveCSS('background-color', 'rgb(17, 25, 39)')
  await expect(page.locator('.page-editbar')).toHaveCSS('background-color', 'rgb(38, 54, 74)')
  await expect(page.locator('.topbar')).toBeVisible()
  const headerHeight = await page.locator('.topbar').evaluate(element => getComputedStyle(element).height)
  const pageTool = page.locator('.page-editbar .text-tool').first()
  await expect(pageTool).toBeEnabled()
  await pageTool.hover()
  await expect(pageTool).toHaveCSS('background-color', 'rgb(49, 68, 93)')
  await page.getByRole('button', { name: 'Collapse sidebar' }).click()
  await expect(page.locator('.sidebar')).toHaveCSS('width', '68px')
  await expect(page.locator('.sidebar .nav-label').first()).toBeHidden()
  await page.getByRole('button', { name: 'Expand sidebar' }).click()
  await expect(page.locator('.sidebar .nav-label').first()).toBeVisible()
  expect(await page.locator('.documents-workspace').evaluate(element => getComputedStyle(element).gridTemplateColumns.split(' ').length)).toBe(3)
  expect(await page.locator('.docs-panel').evaluate(element => element.getBoundingClientRect().top)).toBeLessThan(280)
  expect(await page.locator('.canvas-scroll').evaluate(element => element.getBoundingClientRect().top)).toBeLessThan(400)
  await expect.poll(() => page.locator('.inspector-panel').evaluate(element => window.innerHeight - element.getBoundingClientRect().bottom)).toBeLessThan(18)
  expect(await page.locator('.inspector-panel').evaluate(element => window.innerHeight - element.getBoundingClientRect().bottom)).toBeGreaterThan(-2)
  await expect(page.locator('.thumbnail-strip')).toHaveCount(0)
  await expect(page.getByRole('textbox', { name: 'Search documents' })).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)')
  await expect(page.getByRole('textbox', { name: 'Display title' })).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)')
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBeTruthy()
  await expect(page.getByRole('button', { name: 'Build filing bundle' }).first()).toBeInViewport()
  await page.getByRole('tab', { name: 'Tools' }).click()
  await page.keyboard.press('ArrowLeft')
  await expect(page.getByRole('tab', { name: 'Details' })).toHaveAttribute('aria-selected', 'true')
  await page.keyboard.press('ArrowRight')
  await page.getByRole('button', { name: 'Page tools', exact: true }).click()
  const pageToolsDialog = page.getByRole('dialog', { name: 'Page tools' })
  await expect(pageToolsDialog).toBeVisible()
  await expect(pageToolsDialog).toContainText('01_petition')
  await expect(pageToolsDialog.getByRole('combobox', { name: 'Source document' })).toHaveCSS('color-scheme', 'dark')
  await expect(pageToolsDialog.getByRole('combobox', { name: 'Source document' })).toHaveCSS('height', '44px')
  await expect(pageToolsDialog.locator('.page-tools input').first()).toHaveCSS('height', '44px')
  await pageToolsDialog.getByRole('button', { name: 'Close page tools' }).click()
  await expect(pageToolsDialog).toBeHidden()
  await page.getByRole('button', { name: 'Page tools', exact: true }).click()
  await page.keyboard.press('Escape')
  await expect(pageToolsDialog).toBeHidden()
  await page.getByRole('tab', { name: 'Details' }).click()
  await page.getByRole('button', { name: 'Court rules' }).click()
  await expect(page.locator('.topbar')).toHaveCSS('height', headerHeight)
  await expect(page.locator('.guide-card').first()).toHaveCSS('background-color', 'rgb(29, 41, 58)')
  await expect(page.locator('.template-row').first()).toHaveCSS('background-color', 'rgb(43, 60, 82)')
  await expect(page.locator('.party-form').first()).toHaveCSS('background-color', 'rgb(43, 60, 82)')
  await expect(page.locator('.party-form textarea').first()).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)')
  await expect(page.locator('.party-form select').first()).toHaveCSS('color-scheme', 'dark')
  await expect(page.locator('.party-form select').first()).toHaveCSS('height', '40px')
  await expect(page.locator('.party-form input').first()).toHaveCSS('height', '40px')
  await page.getByRole('button', { name: 'New matter' }).click()
  await expect(page.getByRole('dialog', { name: 'Create matter' })).toHaveCSS('background-color', 'rgb(29, 41, 58)')
  await expect(page.getByRole('dialog', { name: 'Create matter' }).getByPlaceholder('Example: ABC Pvt Ltd v. XYZ'))
    .toHaveCSS('background-color', 'rgba(0, 0, 0, 0)')
  await expect(page.getByRole('dialog', { name: 'Create matter' }).getByRole('combobox', { name: 'Court profile' }))
    .toHaveCSS('background-color', 'rgba(0, 0, 0, 0)')
  await expect(page.getByRole('dialog', { name: 'Create matter' }).getByRole('combobox', { name: 'Court profile' }))
    .toHaveCSS('color-scheme', 'dark')
  await expect(page.getByRole('dialog', { name: 'Create matter' }).getByRole('combobox', { name: 'Court profile' }))
    .toHaveCSS('height', '44px')
  await page.getByRole('dialog', { name: 'Create matter' }).getByRole('button', { name: 'Close' }).click()
  await page.getByRole('button', { name: 'About POC' }).click()
  await expect(page.locator('.topbar')).toHaveCSS('height', headerHeight)
  await expect(page.locator('.feature-strip > div').first()).toHaveCSS('background-color', 'rgb(29, 41, 58)')
  await page.getByRole('button', { name: 'Matters', exact: true }).click()
  await page.getByRole('button', { name: 'Build bundle', exact: true }).click()
  await expect(page.locator('.topbar')).toHaveCSS('height', headerHeight)
  await expect(page.locator('.checkbox-line')).toHaveCSS('background-color', 'rgb(43, 60, 82)')
  await page.getByRole('button', { name: /Preflight checks/ }).click()
  await expect(page.locator('.validation-intro')).toHaveCSS('background-color', 'rgb(43, 60, 82)')
  await page.getByRole('button', { name: 'Documents', exact: true }).click()
  await page.reload()
  await expect(page.getByRole('combobox', { name: 'Theme' })).toHaveValue('dark')
  await expect(page.getByRole('combobox', { name: 'Interface size' })).toHaveValue('125')
  await page.getByRole('link', { name: 'Skip to main content' }).focus()
  await page.keyboard.press('Enter')
  await expect(page.locator('#main-content')).toBeFocused()
  await page.locator('#main-content').click({ position: { x: 20, y: 20 } })
  await page.keyboard.press('/')
  await expect(page.getByRole('textbox', { name: 'Search documents' })).toBeFocused()
  await page.keyboard.type('sample')
  await page.keyboard.press('Control+z')
  await expect(page.getByRole('textbox', { name: 'Search documents' })).toBeFocused()
  await page.getByRole('button', { name: 'Build bundle', exact: true }).click()
  await page.keyboard.press('/')
  await expect(page.getByRole('textbox', { name: 'Search documents' })).toBeFocused()
  await page.setViewportSize({ width: 820, height: 700 })
  await expect(page.getByRole('button', { name: 'Open navigation' })).toBeVisible()
  await expect.poll(() => page.locator('.sidebar').evaluate(element => element.getBoundingClientRect().right <= 0)).toBeTruthy()
  await expect(page.getByRole('button', { name: 'Build bundle', exact: true })).toBeVisible()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBeTruthy()
  await page.getByRole('button', { name: 'Open navigation' }).click()
  await expect(page.getByRole('combobox', { name: 'Theme' })).toBeVisible()
  await page.keyboard.press('Escape')
  await expect.poll(() => page.locator('.sidebar').evaluate(element => element.getBoundingClientRect().right <= 0)).toBeTruthy()
  await page.getByRole('button', { name: 'Open navigation' }).click()
  await page.emulateMedia({ colorScheme: 'dark' })
  await page.getByRole('combobox', { name: 'Theme' }).selectOption('system')
  await expect(page.locator('html')).toHaveAttribute('data-effective-theme', 'dark')
  await page.emulateMedia({ colorScheme: 'light' })
  await expect(page.locator('html')).toHaveAttribute('data-effective-theme', 'light')
  await expect(page.getByRole('combobox', { name: 'Theme' })).toHaveCSS('color-scheme', 'light')
  await expect(page.locator('.panel').first()).toHaveCSS('background-color', 'rgb(255, 255, 255)')
})

test('working-copy page tools edit and save synthetic pages', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Page Tools', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const id = (await created.json()).id as string
  for (const name of ['01_petition.pdf', '03_affidavit.pdf']) {
    const uploaded = await request.post(`${backend}/matters/${id}/documents`, { headers,
      multipart: { files: { name, mimeType: 'application/pdf', buffer: await readFile(resolve(samples, name)) } },
    })
    expect(uploaded.ok()).toBeTruthy()
  }
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'E2E Page Tools' })).toBeVisible()
  await page.getByRole('button', { name: 'Page tools', exact: true }).click()
  const tools = page.getByRole('dialog', { name: 'Page tools' })
  await tools.getByRole('combobox', { name: 'Source document' }).selectOption({ index: 2 })
  await tools.getByRole('textbox', { name: 'Source pages' }).fill('1')
  await tools.getByRole('spinbutton', { name: 'Insert before page' }).fill('2')
  await tools.getByRole('button', { name: 'Insert selected pages' }).click()
  await expect(page.locator('.page-counter')).toContainText('/ 4')
  await tools.getByRole('spinbutton', { name: 'Or insert after page' }).fill('1')
  await tools.getByRole('button', { name: 'Insert after page' }).click()
  await expect(page.locator('.page-counter')).toContainText('/ 5')
  await tools.getByRole('textbox', { name: 'Target pages to replace' }).fill('1')
  await tools.getByRole('button', { name: 'Replace one-for-one' }).click()
  await expect(tools.getByRole('button', { name: 'Replace one-for-one' })).toBeEnabled()
  await tools.getByRole('button', { name: 'Merge: append all' }).click()
  await expect(page.locator('.page-counter')).toContainText('/ 7')
  await tools.getByRole('textbox', { name: 'Pages to crop' }).fill('1')
  await tools.getByRole('spinbutton', { name: 'left %' }).fill('10')
  await tools.getByRole('spinbutton', { name: 'right %' }).fill('90')
  await tools.getByRole('button', { name: 'Crop selected pages' }).click()
  await expect(tools.getByRole('button', { name: 'Crop selected pages' })).toBeEnabled()
  await expect.poll(async () => {
    const documents = await request.get(`${backend}/matters/${id}/documents`, { headers })
    return (await documents.json())[0].revision
  }).toBe(6)
  await tools.getByRole('textbox', { name: 'Pages to extract' }).fill('1,3-4')
  const extractedDownload = page.waitForEvent('download')
  await tools.getByRole('button', { name: 'Save extracted PDF' }).click()
  const extracted = await extractedDownload
  expect(extracted.suggestedFilename()).toBe('extracted-pages.pdf')
  expect((await readFile(await extracted.path())).subarray(0, 4).toString()).toBe('%PDF')
  await tools.getByRole('textbox', { name: 'Split after pages' }).fill('2')
  const splitDownload = page.waitForEvent('download')
  await tools.getByRole('button', { name: 'Save split ZIP' }).click()
  const split = await splitDownload
  expect(split.suggestedFilename()).toBe('split-pages.zip')
  expect((await readFile(await split.path())).subarray(0, 2).toString()).toBe('PK')
})

test('unavailable engine shows a bounded error and reconnects', async ({ page }) => {
  await page.route('**/health', route => route.abort())
  await page.goto('/')
  await expect(page.getByText('Engine disconnected')).toBeVisible({ timeout: 20_000 })
  await expect(page.getByText('Python engine unavailable')).toBeVisible()
  await page.unroute('**/health')
  await page.getByRole('button', { name: 'Retry connection' }).click()
  await expect(page.getByText('Local engine connected')).toBeVisible()
})

test('page text search opens a result and thumbnails navigate', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Search Matter', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const id = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${id}/documents`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'E2E Search Matter' })).toBeVisible()
  await page.getByRole('textbox', {name:'Find text on pages'}).fill('page 3/3')
  await expect(page.getByRole('button', {name:/01_petition · page 3/})).toBeVisible()
  await page.getByRole('button', {name:/01_petition · page 3/}).click()
  await expect(page.locator('.page-counter')).toContainText('3 / 3')
  await page.getByRole('button', {name:'Show thumbnails'}).click()
  await page.getByRole('button', {name:'Go to page 1'}).click()
  await expect(page.locator('.page-counter')).toContainText('1 / 3')
  await expect.poll(() => page.locator('.thumbnail-button canvas').first()
    .evaluate((node:HTMLCanvasElement) => node.width)).toBeGreaterThan(0)
  await page.getByRole('button', {name:/01_petition · page 3/}).click()
  page.once('dialog', dialog => void dialog.accept())
  await page.locator('button[title^="Delete page"]').click()
  await expect(page.locator('.page-counter')).toContainText('/ 2')
  await expect(page.getByText('0 matching page(s)')).toBeVisible()
})

test('synthetic browser journey, retained export and download bytes', async ({ page, request }) => {
  page.setDefaultTimeout(10_000)
  await page.goto('/')
  await expect(page.getByText('Local engine connected')).toBeVisible()
  await page.getByRole('button', { name: 'New matter' }).click()
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
  await page.getByRole('button', { name: 'Rotate current page' }).click()
  await expect.poll(() => canvas.evaluate((node: HTMLCanvasElement) => `${node.width}x${node.height}`)).not.toBe(before)
  await page.locator('button[title="Move page right"]').click()
  page.once('dialog', dialog => void dialog.accept())
  await page.locator('button[title^="Delete page"]').click()
  await expect(page.locator('.page-counter')).toContainText('1 / 2')

  await page.locator('.doc-item', { hasText: '04_scanned_annexure' }).click()
  await expect(page.locator('.page-counter')).toContainText('1 / 1')
  await page.getByRole('tab', { name: 'Tools' }).click()
  await page.getByRole('button', { name: 'Run English OCR' }).click()
  await page.getByRole('tab', { name: 'Details' }).click()
  await expect(page.locator('.inspector-heading')).toContainText('processed', { timeout: 90_000 })
  await page.getByRole('textbox', { name: 'Display title' }).fill('Synthetic OCR Annexure')
  await page.getByRole('button', { name: 'Save document details' }).click()
  await expect(page.getByText('Document details updated.')).toBeVisible()
  await page.reload()
  await expect(page.locator('.doc-item')).toHaveCount(4)
  await page.locator('.doc-item', { hasText: 'Synthetic OCR Annexure' }).click()
  await expect(page.locator('.inspector-heading')).toContainText('processed')

  await page.getByRole('button', { name: 'Build bundle', exact: true }).click()
  await page.getByRole('spinbutton', { name: /Maximum pages per volume/ }).fill('6')
  await page.locator('.build-panel').getByRole('button', { name: 'Build filing bundle' }).click()
  await expect(page.getByText('Latest filing package')).toBeVisible({ timeout: 90_000 })

  const matterResponse = await request.get(`${backend}/matters`, { headers })
  expect(matterResponse.ok()).toBeTruthy()
  const matterId = (await matterResponse.json()).find((item:{name:string}) => item.name === 'E2E Synthetic Matter').id as string
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

  await page.getByRole('spinbutton', { name: /Maximum pages per volume/ }).fill('2')
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
  await page.getByRole('spinbutton', { name: /Maximum pages per volume/ }).fill('6')
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
  await page.locator('.matter-more summary').click()
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
  await page.getByRole('tab', { name: 'Tools' }).click()
  await page.getByRole('button', { name: 'Archive document' }).click()
  await expect(page.locator('.doc-item')).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Restore', exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Restore', exact: true }).click()
  await expect(page.locator('.doc-item')).toHaveCount(1)
  page.once('dialog', dialog => void dialog.accept())
  await page.locator('.matter-more summary').click()
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
