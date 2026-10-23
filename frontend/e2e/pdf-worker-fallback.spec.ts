import { expect, test } from '@playwright/test'
import { execFileSync } from 'node:child_process'
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { extname, resolve } from 'node:path'

const frontend = resolve(import.meta.dirname, '..')
const backend = 'http://127.0.0.1:8768'
const headers = { Authorization: 'Bearer e2e-isolated-token' }
let bundle = ''

test.beforeAll(() => {
  // Build separately so this regression cannot accidentally exercise Vite's
  // unbundled development worker, or replace a concurrent native build's dist.
  bundle = mkdtempSync(resolve(tmpdir(), 'courtbundle-worker-build-'))
  execFileSync(process.execPath, [resolve(frontend, 'node_modules/vite/bin/vite.js'),
    'build', '--outDir', bundle], {
    cwd: frontend,
    env: { ...process.env, VITE_API_BASE: backend, VITE_DEV_TOKEN: 'e2e-isolated-token' },
    stdio: 'pipe',
  })
})
test.afterAll(() => { if (bundle) rmSync(bundle, { recursive: true, force: true }) })

test('production PDF reader renders with the bundled main-thread fallback when Worker is unavailable', async ({ page, request }, info) => {
  const errors: string[] = []
  const network: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  page.on('requestfailed', request => network.push(`${request.method()} ${request.url()}: ${request.failure()?.errorText}`))
  page.on('response', response => { if (response.status() >= 400) network.push(`${response.status()} ${response.url()}`) })
  const workerName = readdirSync(resolve(bundle, 'assets')).find(name => /^pdfWorker-.*\.js$/.test(name))!
  expect(workerName).toBeTruthy()
  const workerSource = readFileSync(resolve(bundle, 'assets', workerName), 'utf8')
  expect(workerSource).toMatch(/export\s*\{[^}]*WorkerMessageHandler/)
  const appName = readdirSync(resolve(bundle, 'assets')).find(name => /^index-.*\.js$/.test(name))!
  const appSource = readFileSync(resolve(bundle, 'assets', appName), 'utf8')
  const cssName = readdirSync(resolve(bundle, 'assets')).find(name => /^index-.*\.css$/.test(name))!
  expect(appSource).toContain(backend)

  const matter = await (await request.post(`${backend}/matters`, { headers,
    data: { name: 'E2E Production Worker Fallback', court_profile: 'general-demo' } })).json()
  const imported = await request.post(`${backend}/matters/${matter.id}/documents`, { headers,
    multipart: { files: { name: 'worker-fallback-synthetic.pdf', mimeType: 'application/pdf',
      buffer: readFileSync(resolve(frontend, '../samples/01_petition.pdf')) } } })
  expect(imported.ok()).toBeTruthy()

  const origin = new URL(String(info.project.use.baseURL)).origin
  const contentTypes: Record<string, string> = {
    '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.woff2': 'font/woff2',
  }
  const served = new Set<string>()
  await page.route(`${origin}/assets/**`, async route => {
    const pathname = new URL(route.request().url()).pathname
    const file = resolve(bundle, pathname === '/' ? 'index.html' : `.${pathname}`)
    if (!file.startsWith(bundle + '/') || !existsSync(file)) { await route.abort(); return }
    served.add(pathname)
    await route.fulfill({ body: readFileSync(file), contentType: contentTypes[extname(file)] || 'application/octet-stream' })
  })
  // Keep the real loopback document response. Fulfilling the top-level HTML
  // synthetically makes Chrome classify its address space as public and reject
  // the local API. Replace only the app entry with the complete production JS
  // and CSS; the PDF worker is likewise the emitted production asset above.
  await page.route(`${origin}/src/main.ts`, route => route.fulfill({
    contentType: 'text/javascript',
    body: `const productionCss=document.createElement('link');productionCss.rel='stylesheet';productionCss.href='/assets/${cssName}';document.head.append(productionCss);\n${appSource}`,
  }))
  await page.addInitScript(() => {
    Object.defineProperty(window, 'Worker', { configurable: true, value: class {
      constructor() {
        ;(window as any).__blockedWorkerAttempts = ((window as any).__blockedWorkerAttempts || 0) + 1
        throw new Error('Synthetic worker refusal for the PDF.js fallback regression')
      }
    } })
  })
  await page.goto('/')
  await expect(page.locator('.doc-item').filter({ hasText: 'worker-fallback-synthetic' })).toBeVisible()
    .catch(cause => { throw new Error(`Production app connection: ${JSON.stringify(network)}; ${JSON.stringify(errors)}; ${cause}`) })
  await page.locator('.doc-item').filter({ hasText: 'worker-fallback-synthetic' }).click()
  await expect(page.getByRole('spinbutton', { name: 'Go to page', exact: true })).toBeEnabled()
  const surface = page.locator('.pdf-page-surface[data-page="1"]')
  await expect(surface).toHaveAttribute('aria-busy', 'false')
  await expect(surface.locator('.pdf-text-layer')).not.toHaveText('')
  // Fit page scales to the available reader height, which changes with the
  // desktop toolbar and viewport. A nontrivial raster plus text proves the
  // main-thread fallback rendered without pinning a layout-specific width.
  expect(await surface.locator('canvas').evaluate(canvas => (canvas as HTMLCanvasElement).width)).toBeGreaterThan(50)
  expect(await page.evaluate(() => (window as any).__blockedWorkerAttempts)).toBeGreaterThan(0)
  expect(served.has(`/assets/${workerName}`)).toBe(true)

  // Fallback stays usable for the next document lifecycle too.
  await page.reload()
  await expect(page.getByRole('spinbutton', { name: 'Go to page', exact: true })).toBeEnabled()
  await expect(page.locator('.pdf-text-layer')).not.toHaveText('')
  expect(errors).toEqual([])
})
