import { defineConfig } from '@playwright/test'
import { existsSync, mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { resolve } from 'node:path'

const root = resolve(import.meta.dirname, '..')
const python = process.env.COURTBUNDLE_TEST_PYTHON || [
  resolve(root, 'backend/.venv312/bin/python'),
  resolve(root, 'backend/.venv/bin/python'),
].find(existsSync) || 'python3'
const dataDir = mkdtempSync(resolve(tmpdir(), 'courtbundle-e2e-'))
const vitePort = Number(process.env.COURTBUNDLE_E2E_VITE_PORT || 1420)
process.env.COURTBUNDLE_E2E_DATA_DIR = dataDir

export default defineConfig({
  testDir: './e2e',
  timeout: 120_000,
  expect: { timeout: 15_000 },
  workers: 1,
  retries: 0,
  reporter: 'list',
  globalTeardown: './e2e/teardown.ts',
  use: { baseURL: `http://127.0.0.1:${vitePort}`, browserName: 'chromium', channel: 'chrome', acceptDownloads: true },
  webServer: [
    {
      command: `"${python}" launcher.py --port 8768 --data-dir "${dataDir}"`,
      cwd: resolve(root, 'backend'),
      url: 'http://127.0.0.1:8768/health',
      reuseExistingServer: false,
      timeout: 30_000,
      env: { COURTBUNDLE_API_TOKEN: 'e2e-isolated-token', COURTBUNDLE_E2E_VITE_PORT: String(vitePort),
        COURTBUNDLE_REQUIRE_VAULT: process.env.COURTBUNDLE_E2E_REQUIRE_VAULT === '1' ? '1' : '0' },
      // Authenticated health returns 401 until the test supplies its token.
      ignoreHTTPSErrors: false,
    },
    {
      command: `npm run dev -- --port ${vitePort}`,
      cwd: resolve(root, 'frontend'),
      url: `http://127.0.0.1:${vitePort}`,
      reuseExistingServer: false,
      timeout: 30_000,
      env: { VITE_API_BASE: 'http://127.0.0.1:8768', VITE_DEV_TOKEN: 'e2e-isolated-token' },
    },
  ],
})
