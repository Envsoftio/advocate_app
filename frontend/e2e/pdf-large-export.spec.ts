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

function largeSyntheticPdf() {
  return execFileSync(python, ['-c', `import os,pymupdf,sys
with pymupdf.open() as pdf:
 page=pdf.new_page(width=420,height=595)
 page.insert_text((36,80),'Large synthetic document',fontsize=18)
 for index in range(4):
  page=pdf.new_page(width=420,height=595)
  image=pymupdf.Pixmap(pymupdf.csRGB,1600,1500,os.urandom(1600*1500*3),False)
  page.insert_image(pymupdf.Rect(36,120,384,500),pixmap=image)
  page.insert_text((36,70),f'Synthetic noise image {index+1}',fontsize=14)
 sys.stdout.buffer.write(pdf.tobytes())
`], { maxBuffer: 80 * 1024 * 1024 })
}

test('generated PDFs above 25 MiB use bounded ranges, cancel cleanly, retry and retain build identity', async ({ page, request }) => {
  const errors: string[] = []
  let phase = 'initial'
  page.on('pageerror', error => errors.push(`${phase}: ${error.stack || error.message}`))
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Large Generated Reader', court_profile: 'general-demo', max_pages_per_volume: 100,
  } })
  expect(created.ok()).toBeTruthy()
  const matterId = (await created.json()).id as string
  const source = largeSyntheticPdf()
  expect(source.length).toBeGreaterThan(25 * 1024 * 1024)
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: 'large-synthetic.pdf', mimeType: 'application/pdf', buffer: source } },
  })
  expect(uploaded.ok()).toBeTruthy()
  const document = (await uploaded.json())[0]
  const started = await request.post(`${backend}/matters/${matterId}/build`, { headers })
  expect(started.ok()).toBeTruthy()
  const jobId = (await started.json()).id
  let completed: { status: string; error?: string; result?: { id: string } } | undefined
  await expect.poll(async () => {
    completed = await (await request.get(`${backend}/jobs/${jobId}`, { headers })).json()
    return completed!.status
  }, { timeout: 90000 }).toBe('completed')
  const exportId = completed!.result!.id
  const manifest = (await (await request.get(`${backend}/matters/${matterId}/exports`, { headers })).json())[0]
  const file = manifest.files.find((item: { name: string }) => item.name === 'combined.pdf')
  expect(file.bytes).toBeGreaterThan(25 * 1024 * 1024)
  const pdfUrl = `${backend}/matters/${matterId}/exports/${exportId}/inspection/combined.pdf`
  const network = await page.context().newCDPSession(page)
  await network.send('Network.enable')
  const tracked = new Set<string>()
  const ranges: { range: string; bytes: number }[] = []
  let transferred = 0
  network.on('Network.requestWillBeSent', event => { if (event.request.url === pdfUrl) tracked.add(event.requestId) })
  network.on('Network.dataReceived', event => { if (tracked.has(event.requestId)) transferred += event.dataLength })
  page.on('response', response => {
    if (response.url() === pdfUrl && response.status() === 206) ranges.push({
      range: response.headers()['content-range'], bytes: Number(response.headers()['content-length']),
    })
  })
  let hold = true, fail = false, observedHeld = false
  let release!: () => void
  const held = new Promise<void>(resolve => { release = resolve })
  await page.route(pdfUrl, async route => {
    if (hold) { observedHeld = true; await held }
    if (fail) { await route.fulfill({ status: 503, contentType: 'application/json', body: '{"detail":"Synthetic unavailable export"}' }); return }
    try { await route.continue() } catch { /* Expected if closing cancelled this held request. */ }
  })
  await page.setViewportSize({ width: 1440, height: 1000 })
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Large Generated Reader'})).toBeVisible()
  await page.getByRole('button', { name: 'Build bundle', exact: true }).click()
  const review = page.getByRole('button', { name: 'Review combined.pdf', exact: true })
  const dialog = page.getByRole('dialog', { name: 'Review generated PDF' })

  phase = 'held request cancellation'
  await review.click()
  await expect(dialog).toBeVisible()
  await expect.poll(() => observedHeld).toBe(true)
  await dialog.getByRole('button', { name: 'Close review', exact: true }).click()
  await expect(dialog).toHaveCount(0)
  hold = false; release()

  fail = true
  phase = 'unavailable export'
  await review.click()
  await expect(dialog.getByRole('alert')).toContainText('could not be displayed')
  fail = false
  phase = 'range retry'
  transferred = 0; ranges.length = 0
  const startedAt = Date.now()
  await dialog.getByRole('button', { name: 'Retry review', exact: true }).click()
  await expect(dialog.locator('.pdf-page-surface')).not.toHaveClass(/is-rendering/)
  await expect(dialog.locator('.pdf-text-layer')).toContainText('INDEX')
  const pageInput = dialog.getByRole('spinbutton', { name: 'PDF page number', exact: true })
  await pageInput.fill('2'); await pageInput.press('Enter')
  await expect(dialog.locator('.pdf-text-layer')).toContainText('Large synthetic document')
  const elapsed = Date.now() - startedAt
  expect(ranges.length).toBeGreaterThan(0)
  expect(ranges.every(item => /^bytes \d+-\d+\/\d+$/.test(item.range))).toBe(true)
  expect(ranges.reduce((sum, item) => sum + item.bytes, 0)).toBeLessThan(file.bytes / 4)
  expect(transferred).toBeLessThan(file.bytes / 4)
  expect(await dialog.locator('canvas').count()).toBeLessThanOrEqual(2)
  const metrics = {
    generatedPdfBytes: file.bytes, firstIndexAndSourcePageMs: elapsed,
    receivedBodyBytes: transferred, rangeResponseBytes: ranges.reduce((sum, item) => sum + item.bytes, 0),
    rangeResponses: ranges.length, viewport: '1440 × 1000, development Chrome',
  }
  console.info('Large generated reader metrics:', JSON.stringify(metrics))
  await test.info().attach('large-generated-reader-metrics.json', { contentType: 'application/json', body: JSON.stringify(metrics, null, 2) })
  await dialog.screenshot({ path: test.info().outputPath('large-generated-reader.png') })
  await expect(dialog).toContainText(`Build ${exportId.slice(0,8)}`)

  phase = 'keyboard navigation'
  await dialog.getByRole('button', { name: 'Close review', exact: true }).focus()
  await page.keyboard.press('Control+0')
  await expect(dialog.getByRole('combobox', { name: 'Zoom', exact: true })).toHaveValue('fit-width')
  await page.keyboard.press('Control+=')
  await expect(dialog.getByRole('combobox', { name: 'Zoom', exact: true })).not.toHaveValue('fit-width')
  await page.keyboard.press('PageUp'); await expect(pageInput).toHaveValue('1')
  await page.keyboard.press('PageDown'); await expect(pageInput).toHaveValue('2')
  await page.keyboard.press('Control+f')
  const find = dialog.getByRole('searchbox', { name: 'Find text in generated PDF' })
  await expect(find).toBeFocused()
  await page.keyboard.press('Home')
  await expect(pageInput).toHaveValue('2')
  await page.keyboard.press('Escape')
  await expect(dialog).toHaveCount(0)
  const key = `export:${matterId}:${exportId}:combined.pdf`
  await expect.poll(async () => (await (await request.get(`${backend}/reader-position?key=${encodeURIComponent(key)}`, { headers })).json())?.page).toBe(2)

  // A later matter edit must not silently turn an immutable export into the
  // current working document, or overwrite its separately saved reading state.
  expect((await request.patch(`${backend}/documents/${document.id}`, { headers, data: { title: 'Changed after generated build' } })).ok()).toBeTruthy()
  phase = 'reload and restore'
  await page.reload()
  await page.getByRole('button', { name: 'Build bundle', exact: true }).click()
  await page.getByRole('button', {name:'Open earlier bundle',exact:true}).click()
  await expect(dialog).toContainText('Matter inputs changed since this export.')
  await expect(pageInput).toHaveValue('2')
  await expect(dialog.locator('.pdf-text-layer')).toContainText('Large synthetic document')
  await dialog.getByRole('button', { name: 'Close review', exact: true }).click()
  expect(errors).toEqual([])
})
