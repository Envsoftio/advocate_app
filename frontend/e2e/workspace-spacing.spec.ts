import { test, expect } from '@playwright/test'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'

test('top-level pages fit narrow windows', async ({ page }) => {
  await page.goto('/')
  for (const width of [1440, 620, 390]) {
    await page.setViewportSize({width,height:900})
    for (const name of ['Court rules', 'About POC', 'Matters']) {
      if (width <= 900) await page.getByRole('button', {name:'Open navigation'}).click()
      await page.locator('.sidebar').getByRole('button', {name,exact:true}).click()
      await expect(page.getByRole('heading', {name:name === 'Court rules' ? 'Court rules & technical profiles' : name === 'About POC' ? 'Built for structured legal filing preparation.' : 'Your matters'})).toBeVisible()
      await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
    }
  }
})

test('workspace and export controls stay separated at desktop and narrow widths', async ({ page, request }) => {
  const headers = { Authorization: 'Bearer e2e-isolated-token' }
  const base = 'http://127.0.0.1:8768'
  const response = await request.post(`${base}/matters`, { headers, data: {
    name: 'Synthetic layout audit', petitioner: 'Sample Applicant', respondent: 'Sample Respondent',
    court_profile: 'general-demo',
  } })
  expect(response.ok()).toBeTruthy()
  const matter = await response.json()
  const upload = await request.post(`${base}/matters/${matter.id}/documents`, { headers, multipart: {
    files: { name: '01_petition.pdf', mimeType: 'application/pdf', buffer: await readFile(resolve(import.meta.dirname, '../../samples/01_petition.pdf')) },
  } })
  expect(upload.ok()).toBeTruthy()
  await page.goto('/')
  await page.getByRole('button', { name: 'Matters', exact: true }).click()
  await page.locator('.matter-home-card').filter({ hasText: 'Synthetic layout audit' }).click()
  for (const width of [1440, 900, 620, 390]) {
    await page.setViewportSize({ width, height: 900 })
    for (const name of ['Overview', 'Documents', 'Index & bookmarks', 'Build bundle', 'Preflight checks']) {
      await page.getByRole('navigation', { name: 'Matter preparation' }).getByRole('button', { name, exact: true }).click()
      await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
      if (name === 'Build bundle') {
        const gaps = await page.locator('.build-body').evaluate(el => {
          const box = (selector: string) => el.querySelector(selector)!.getBoundingClientRect()
          return [
            box('.checkbox-line + .btn').top - box('.checkbox-line').bottom,
            box('.checklist').top - box('.checkbox-line + .btn').bottom,
            box('.notice').top - box('.checklist').bottom,
          ]
        })
        for (const gap of gaps) expect(gap).toBeGreaterThanOrEqual(12)
        await expect(page.locator('.checklist')).toHaveCSS('grid-template-columns', width <= 520 ? /^\d+(\.\d+)?px$/ : /^\d+(\.\d+)?px \d+(\.\d+)?px$/)
      }
    }
  }
  await page.getByRole('navigation', { name: 'Matter preparation' }).getByRole('button', { name: 'Build bundle', exact: true }).click()
  await page.locator('.build-body').getByRole('button', { name: 'Build filing bundle', exact: true }).click()
  await expect(page.locator('.export-file').first()).toBeVisible({timeout: 60000})
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  const rowsFit = await page.locator('.export-file').evaluateAll(rows => rows.every(row => row.scrollWidth <= row.clientWidth))
  expect(rowsFit).toBe(true)
})
