import { test, expect } from '@playwright/test'

test('court rules identifies applied checks and keeps dark theme reference text legible', async ({ page }) => {
  await page.goto('/')
  await page.getByRole('button', { name: 'New matter' }).click()
  await expect(page.getByRole('combobox', { name: 'Court profile' })).toHaveValue('general-demo')
  await page.getByRole('dialog', { name: 'Create matter' }).getByRole('button', { name: 'Cancel' }).click()
  await page.getByRole('combobox', { name: 'Theme' }).selectOption('dark')
  await page.locator('.sidebar').getByRole('button', { name: 'Court rules' }).click()
  await expect(page.getByRole('heading', { name: 'Court rules & technical profiles' })).toBeVisible()
  await expect(page.locator('.profile-scope')).toContainText('searchable-page warnings and the export file size cap')
  await expect(page.locator('.guide-eyebrow')).toContainText('REFERENCE ONLY')
  const contrast = async (selector: string) => page.locator(selector).first().evaluate(el => {
    const rgb = (color: string) => color.match(/[\d.]+/g)!.slice(0, 3).map(Number)
    const luminance = (color: string) => {
      const channels = rgb(color).map(value => {
        const linear = value / 255
        return linear <= .04045 ? linear / 12.92 : ((linear + .055) / 1.055) ** 2.4
      })
      return channels[0]! * .2126 + channels[1]! * .7152 + channels[2]! * .0722
    }
    const foreground = luminance(getComputedStyle(el).color)
    const background = luminance(getComputedStyle(el).backgroundColor)
    return (Math.max(foreground, background) + .05) / (Math.min(foreground, background) + .05)
  })
  for (const selector of ['.template-row strong span', '.rules-page > .notice.amber', '.profile-scope', '.conflict-box']) {
    expect(await contrast(selector), selector).toBeGreaterThan(4.5)
  }
  await expect(page.locator('.conflict-box')).toHaveCSS('border-left-width', '4px')
  await expect(page.locator('.conflict-box p').first()).toHaveCSS('color', 'rgb(227, 236, 247)')
  await page.setViewportSize({ width: 390, height: 900 })
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  await page.getByRole('button', { name: 'Open navigation' }).click()
  await page.getByRole('combobox', { name: 'Theme' }).selectOption('system')
  await page.emulateMedia({ colorScheme: 'dark' })
  await expect(page.locator('.template-row strong span').first()).toHaveCSS('color', 'rgb(226, 237, 251)')
})
