import { openBuildTool, submitBundleBuild, openOutputTools } from './navigation'
import { includeAllDocuments, openWorkspaceNavigation } from './navigation'
import { expect, test } from '@playwright/test'
import { execFileSync } from 'node:child_process'
import { existsSync, mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { resolve } from 'node:path'

test('protected PDF import handles wrong password and cancellation, then exports an encrypted PDF', async ({ page }) => {
  const folder = mkdtempSync(resolve(tmpdir(), 'matterdraft-pdf09-'))
  const locked = resolve(folder, 'synthetic-protected.pdf')
  const root = resolve(import.meta.dirname, '../../backend')
  const python = process.env.MATTERDRAFT_TEST_PYTHON || [
    resolve(root, '.venv312/bin/python'), resolve(root, '.venv/bin/python'),
  ].find(existsSync) || 'python3'
  try {
    execFileSync(python, ['-c', `import pymupdf,sys
with pymupdf.open(sys.argv[1]) as pdf:
 pdf.save(sys.argv[2],encryption=pymupdf.PDF_ENCRYPT_AES_256,owner_pw='synthetic owner',user_pw='synthetic input',permissions=4095)
`, resolve(import.meta.dirname, '../../samples/01_petition.pdf'), locked])

    await page.goto('/')
  await openWorkspaceNavigation(page)
    await page.locator('.sidebar').getByRole('button', { name: 'New matter' }).click()
    const create = page.getByRole('dialog', { name: 'Create matter' })
    await create.getByPlaceholder('Example: ABC Pvt Ltd v. XYZ').fill('PDF-09 Browser Synthetic')
    await create.getByRole('button', { name: 'Create matter' }).click()
    await expect(page.locator('.application-matter').filter({hasText:'PDF-09 Browser Synthetic'})).toBeVisible()

    await page.locator('input[accept=".pdf,.jpg,.jpeg,.png"]').setInputFiles(locked)
    const prompt = page.getByRole('dialog', { name: 'Open protected PDF' })
    await expect(prompt).toBeVisible()
    await prompt.getByLabel('PDF password', { exact: true }).fill('wrong')
    await prompt.getByRole('button', { name: 'Open and import' }).click()
    await expect(prompt.getByRole('alert')).toContainText('Incorrect PDF password')
    await prompt.getByRole('button', { name: 'Cancel import' }).click()
    await expect(page.locator('.doc-item')).toHaveCount(0)
    await expect(page.locator('input[accept=".pdf,.jpg,.jpeg,.png"]')).toHaveValue('')

    await page.locator('input[accept=".pdf,.jpg,.jpeg,.png"]').setInputFiles(locked)
    await prompt.getByLabel('PDF password', { exact: true }).fill('synthetic input')
    await prompt.getByRole('button', { name: 'Open and import' }).click()
    await expect(page.locator('.doc-item')).toHaveCount(1)
    await includeAllDocuments(page)

    await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click()
    await submitBundleBuild(page)
    await expect(page.getByRole('heading',{name:'Bundle ready for review'})).toBeVisible({ timeout: 90_000 })
    await openOutputTools(page)
  await page.getByRole('button', { name: 'Save password-protected combined.pdf' }).click()
    const exportPrompt = page.getByRole('dialog', { name: 'Save protected PDF' })
    await exportPrompt.getByLabel('PDF password', { exact: true }).fill('synthetic output')
    await exportPrompt.getByLabel('Confirm PDF password', { exact: true }).fill('synthetic output')
    const download = page.waitForEvent('download')
    await exportPrompt.getByRole('button', { name: 'Save protected PDF' }).click()
    expect((await download).suggestedFilename()).toBe('combined-protected.pdf')
    await expect(exportPrompt).not.toBeVisible()
  } finally {
    rmSync(folder, { recursive: true, force: true })
  }
})
