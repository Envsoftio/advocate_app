import { test, expect } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { enterLatestMatter, openWorkspaceNavigation, closeWorkspaceNavigation } from './navigation';
const base = 'http://127.0.0.1:8768',
  headers = { Authorization: 'Bearer e2e-isolated-token' };

test('shared popovers dismiss outside, retain controls, restore focus and fit dark compact reader', async ({
  page,
  request,
}, info) => {
  const matter = await (
    await request.post(`${base}/matters`, {
      headers,
      data: { name: 'Synthetic popup review', court_profile: 'general-demo' },
    })
  ).json();
  const buffer = await readFile(resolve(import.meta.dirname, '../../samples/01_petition.pdf'));
  await request.post(`${base}/matters/${matter.id}/documents?included=true`, {
    headers,
    multipart: { files: { name: 'Synthetic petition.pdf', mimeType: 'application/pdf', buffer } },
  });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/');
  await enterLatestMatter(page);
  await expect(page.locator('.pdf-paper')).toBeVisible();
  for (const width of [1440, 1024])
    for (const theme of ['light', 'dark'])
      for (const scale of [100, 125]) {
        await page.setViewportSize({ width, height: width === 1440 ? 900 : 768 });
        await openWorkspaceNavigation(page);
        await page.getByRole('combobox', { name: 'Theme', exact: true }).selectOption(theme);
        await page.getByRole('combobox', { name: 'Interface size' }).selectOption(String(scale));
        await closeWorkspaceNavigation(page);
        const trigger = page.getByRole('button', { name: 'View options', exact: true });
        await trigger.click();
        const popup = page.locator('.app-popover');
        await expect(popup).toBeVisible();
        await expect(popup.getByLabel('Reading mode')).toBeVisible();
        const box = (await popup.boundingBox())!;
        expect(box.x).toBeGreaterThanOrEqual(0);
        expect(box.x + box.width).toBeLessThanOrEqual(width);
        expect(box.y + box.height).toBeLessThanOrEqual(width === 1440 ? 900 : 768);
        const triggerBox = (await trigger.boundingBox())!;
        expect(Math.abs(box.y - (triggerBox.y + triggerBox.height))).toBeLessThan(12);
        await page.screenshot({
          path: info.outputPath(`view-${width}-${theme}-${scale}.png`),
          animations: 'disabled',
        });
        await page.locator('.pdf-text-layer').click({ position: { x: 25, y: 25 } });
        await expect(popup).toBeHidden();
        await trigger.click();
        await page.keyboard.press('Escape');
        await expect(popup).toBeHidden();
        await expect(trigger).toBeFocused();
        await trigger.click();
        await popup.getByLabel('Reading mode').selectOption('continuous');
        await expect(popup).toBeHidden();
        await trigger.click();
        await expect(popup.getByLabel('Reading mode')).toHaveValue('continuous');
        await popup.getByRole('button', { name: 'PDF keyboard shortcuts', exact: true }).click();
        const help = page.getByRole('dialog', { name: 'PDF keyboard shortcuts', exact: true });
        await expect(help).toBeVisible();
        await expect(popup).toBeHidden();
        await help.getByRole('heading', { name: 'PDF keyboard shortcuts', exact: true }).click();
        await expect(help).toBeVisible();
        await page.mouse.click(8, 8);
        await expect(help).toBeHidden();
        await expect(trigger).toBeFocused();
        await trigger.click();
        await popup.getByLabel('Reading mode').selectOption('single');
      }
  await openWorkspaceNavigation(page);
  await page.getByRole('combobox', { name: 'Interface size' }).selectOption('100');
  await closeWorkspaceNavigation(page);
  if (await page.getByRole('button', { name: 'Open editor', exact: true }).isVisible())
    await page.getByRole('button', { name: 'Open editor', exact: true }).click();

  await page.setViewportSize({ width: 1440, height: 900 });
  const display = page.getByRole('group', { name: 'Page display', exact: true });
  const single = display.getByRole('button', { name: 'Single page', exact: true });
  const continuous = display.getByRole('button', { name: 'Continuous', exact: true });
  await single.focus();
  await page.keyboard.press('ArrowRight');
  await expect(continuous).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(continuous).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('.continuous-reader')).toBeVisible();
  await page.keyboard.press('Space');
  await expect(continuous).toHaveAttribute('aria-pressed', 'true');
  await page.keyboard.press('Home');
  await expect(single).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(single).toHaveAttribute('aria-pressed', 'true');
  await page.screenshot({
    path: info.outputPath('reader-choice-1440-dark.png'),
    animations: 'disabled',
  });
  await page.setViewportSize({ width: 1024, height: 768 });
  await page.getByRole('button', { name: 'More document actions', exact: true }).click();
  await expect(
    page.locator('.app-popover').getByRole('button', { name: 'Save working copy' }),
  ).toBeVisible();
  await page.getByRole('button', { name: 'View options', exact: true }).click();
  await expect(page.locator('.app-popover')).toHaveCount(1);
  await page.keyboard.press('Escape');
  await page.getByRole('tab', { name: 'Annotate', exact: true }).click();
  await page.getByRole('button', { name: 'More annotation tools' }).click();
  await page.locator('.app-popover').getByRole('button', { name: 'Ellipse', exact: true }).click();
  await expect(page.locator('.app-popover')).toBeHidden();
  await page
    .locator('.matter-rail')
    .getByRole('button', { name: 'Documents', exact: true })
    .click();
  if (!(await page.getByRole('button', { name: 'More document list actions' }).isVisible()))
    await page.getByRole('button', { name: 'Toggle documents panel' }).click();
  await page.getByRole('button', { name: 'More document list actions' }).click();
  await page
    .locator('.app-popover')
    .getByRole('button', { name: 'Open table', exact: true })
    .click();
  const table = page.locator('.document-register');
  await expect(table).toBeVisible();
  await table.getByRole('button', { name: 'Filters', exact: true }).click();
  await page.locator('.app-popover').getByLabel('Filter by Part').selectOption('1');
  await expect(page.locator('.app-popover')).toBeVisible();
  await table.getByRole('heading', { name: 'Batch review' }).click();
  await expect(page.locator('.app-popover')).toBeHidden();
  await table.getByRole('button', { name: 'Columns', exact: true }).click();
  await page.locator('.app-popover').getByLabel('OCR', { exact: true }).uncheck();
  await expect(page.locator('.app-popover')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(table.getByRole('button', { name: 'Columns', exact: true })).toBeFocused();
  await table.getByRole('button', { name: 'Actions for Synthetic petition', exact: true }).click();
  await expect(page.locator('.app-popover').getByLabel('Hierarchy')).toBeVisible();
  const rowBox = (await page.locator('.app-popover').boundingBox())!,
    tableBox = (await table.boundingBox())!;
  expect(rowBox.x).toBeGreaterThanOrEqual(tableBox.x);
  expect(rowBox.x + rowBox.width).toBeLessThanOrEqual(tableBox.x + tableBox.width);
  await page.screenshot({
    path: info.outputPath('row-actions-1024-dark.png'),
    animations: 'disabled',
  });
  await table.getByRole('heading', { name: 'Batch review' }).click();
  await expect(page.locator('.app-popover')).toBeHidden();
});
