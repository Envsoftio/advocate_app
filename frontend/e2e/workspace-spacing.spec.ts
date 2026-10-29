import { enterLatestMatter } from './navigation'
import { openBuildTool, submitBundleBuild, openOutputTools } from './navigation'
import { openWorkspaceNavigation } from './navigation'
import { test, expect } from '@playwright/test'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'

test('top-level pages fit narrow windows', async ({ page }, info) => {
  await page.goto('/');await enterLatestMatter(page)
  for (const width of [1440, 620, 390]) {
    await page.setViewportSize({width,height:900})
    for (const name of ['Court rules', 'About MatterDraft', 'Matters']) {
      await openWorkspaceNavigation(page)
      await page.locator('.workspace-drawer').getByRole('button', {name,exact:true}).click()
      await expect(page.getByRole('heading', {name:name === 'Court rules' ? 'Court rules & technical profiles' : name === 'About MatterDraft' ? 'Built for structured legal filing preparation.' : 'Matters'})).toBeVisible()
      await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
      if (name === 'About MatterDraft') {
        await page.screenshot({path:info.outputPath(`about-matterdraft-${width}.png`),animations:'disabled'})
      }
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
  const upload = await request.post(`${base}/matters/${matter.id}/documents?included=true`, { headers, multipart: {
    files: { name: '01_petition.pdf', mimeType: 'application/pdf', buffer: await readFile(resolve(import.meta.dirname, '../../samples/01_petition.pdf')) },
  } })
  expect(upload.ok()).toBeTruthy()
  await page.goto('/');await enterLatestMatter(page)
  await openWorkspaceNavigation(page)
  await page.locator('.workspace-drawer').getByRole('button', { name: 'Matters', exact: true }).click()
  await page.locator('.matter-home-card').filter({ hasText: 'Synthetic layout audit' }).click()
  for (const width of [1440, 900, 620, 390]) {
    await page.setViewportSize({ width, height: 900 })
    for (const name of ['Overview', 'Documents', 'Index & bookmarks', 'Build bundle', 'Preflight checks']) {
      await page.getByRole('navigation', { name: 'Workspace navigation rail' }).getByRole('button', { name, exact: true }).click()
      await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
      if (name === 'Build bundle') {
        const settings=page.locator('.build-settings-card')
        await expect(settings).toBeVisible()
        expect(await settings.evaluate(el=>el.scrollWidth<=el.clientWidth)).toBe(true)
        const gap=await page.locator('.build-settings-grid').evaluate(el=>parseFloat(getComputedStyle(el).gap))
        expect(gap).toBeGreaterThanOrEqual(16)
        const summary=page.locator('.build-summary')
        expect(await summary.evaluate(el=>el.scrollWidth<=el.clientWidth)).toBe(true)
      }
    }
  }
  await page.getByRole('navigation', { name: 'Workspace navigation rail' }).getByRole('button', { name: 'Build bundle', exact: true }).click()
  await submitBundleBuild(page)
  await expect(page.locator('.generated-files')).toBeVisible({timeout: 60000})
  await openOutputTools(page)
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  const rowsFit = await page.locator('.export-file').evaluateAll(rows => rows.every(row => row.scrollWidth <= row.clientWidth))
  expect(rowsFit).toBe(true)
})
