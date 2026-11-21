import { selectOption } from './select';
import { fillDate, expectDate } from './date-field';
import { test, expect } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { enterLatestMatter, openWorkspaceNavigation, closeWorkspaceNavigation } from './navigation';
const base = `http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT || 8768}`,
  headers = { Authorization: 'Bearer e2e-isolated-token' };

test('Reka calendars keep ISO dates, explicit confirmation, blank values and keyboard dismissal', async ({
  page,
  request,
}, info) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const matter = await (
    await request.post(`${base}/matters`, {
      headers,
      data: { name: 'Synthetic calendar review', court_profile: 'general-demo' },
    })
  ).json();
  const buffer = await readFile(resolve(import.meta.dirname, '../../samples/01_petition.pdf'));
  const [doc] = await (
    await request.post(`${base}/matters/${matter.id}/documents?included=true`, {
      headers,
      multipart: { files: { name: 'Calendar fixture.pdf', mimeType: 'application/pdf', buffer } },
    })
  ).json();
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/');
  await enterLatestMatter(page);
  await expect(page.locator('.pdf-paper')).toBeVisible();
  await page
    .locator('.matter-rail')
    .getByRole('button', { name: 'Documents', exact: true })
    .click();
  if (!(await page.getByLabel('Document date', { exact: true }).isVisible()))
    await page.getByRole('button', { name: 'Document details', exact: true }).click();
  const input = page.getByLabel('Document date', { exact: true });
  await expect(input).toBeVisible();
  await expect(input).toHaveAttribute('role', 'group');
  await expect(input.locator('input')).toHaveAttribute('aria-hidden', 'true');
  await expect(input.locator('input')).toBeHidden();
  await input.locator('[data-reka-date-field-segment=day]').click();
  await expect(page.locator('.app-date-popover')).toHaveCount(0);
  await fillDate(input, '2024-02-28');
  const trigger = page.getByRole('button', { name: 'Choose document date', exact: true });
  await trigger.click();
  const popup = page.locator('.app-date-popover');
  await expect(popup).toBeVisible();
  await expect(popup).toContainText('February 2024');
  const selected = popup.locator('[data-selected]');
  await selected.focus();
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Enter');
  await expectDate(input, '2024-02-29');
  await expect(popup).toBeHidden();
  await expect(trigger).toBeFocused();
  await page.getByRole('button', { name: 'Save document details' }).click();
  const read = async () =>
    (await (await request.get(`${base}/matters/${matter.id}/documents`, { headers })).json())[0];
  await expect.poll(async () => (await read()).document_date).toBe('2024-02-29');
  expect((await read()).date_status).toBe('confirmed');
  expect((await read()).source_sha256).toBe(doc.source_sha256);
  for (const [width, theme, scale] of [
    [1440, 'light', 100],
    [1024, 'dark', 125],
  ] as const) {
    await page.setViewportSize({ width, height: width === 1440 ? 900 : 768 });
    await openWorkspaceNavigation(page);
    await selectOption(page.getByRole('combobox', { name: 'Theme', exact: true }), theme);
    await selectOption(page.getByRole('combobox', { name: 'Interface size' }), String(scale));
    await closeWorkspaceNavigation(page);
    if (width === 1024) {
      await expect(input).toBeHidden();
      await page.getByRole('button', { name: 'Document details', exact: true }).click();
    }
    await expect(input).toBeVisible();
    for (const name of ['Document type', 'Part']) {
      const select = page
        .locator('#document-details-panel')
        .getByRole('combobox', { name, exact: true });
      const color = await select.evaluate((el) => getComputedStyle(el).color);
      await expect(select.locator('.app-select-label')).toHaveCSS('color', color);
      await select.click();
      const option = page.getByRole('option', { selected: true });
      await expect(option.locator('.app-select-item-label')).toHaveCSS(
        'color',
        await option.evaluate((el) => getComputedStyle(el).color),
      );
      if (name === 'Document type')
        await page.screenshot({
          path: info.outputPath(`details-dropdown-${width}-${theme}-${scale}.png`),
          animations: 'disabled',
        });
      await page.keyboard.press('Escape');
    }
    const iconColor = await trigger.locator('svg').evaluate((el) => ({
      stroke: getComputedStyle(el).stroke,
      expected: getComputedStyle(el).getPropertyValue('--cb-muted').trim(),
    }));
    const expectedColor = await trigger.evaluate((el, color) => {
      const probe = document.createElement('span');
      probe.style.color = color;
      el.append(probe);
      const result = getComputedStyle(probe).color;
      probe.remove();
      return result;
    }, iconColor.expected);
    expect(iconColor.stroke).toBe(expectedColor);
    await trigger.click();
    await expect(popup).toBeVisible();
    const box = (await popup.boundingBox())!;
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(width);
    expect(box.y + box.height).toBeLessThanOrEqual(width === 1440 ? 900 : 768);
    await page.screenshot({
      path: info.outputPath(`calendar-${width}-${theme}-${scale}.png`),
      animations: 'disabled',
    });
    await popup.getByRole('button', { name: 'Next month', exact: true }).click();
    await expect(popup).toContainText('March 2024');
    await page.keyboard.press('Escape');
    await expect(popup).toBeHidden();
    await expect(trigger).toBeFocused();
    await expectDate(input, '2024-02-29');
  }
  await trigger.click();
  await popup.getByRole('button', { name: 'Clear date', exact: true }).click();
  await expectDate(input, '');
  await page.getByRole('button', { name: 'Save document details' }).click();
  await expect.poll(async () => (await read()).document_date).toBe('');
  expect((await read()).date_status).toBe('unknown');
  await page.setViewportSize({ width: 1440, height: 900 });
  await openWorkspaceNavigation(page);
  await selectOption(page.getByRole('combobox', { name: 'Interface size' }), '100');
  await closeWorkspaceNavigation(page);
  if (!(await page.getByRole('button', { name: 'Sort', exact: true }).isVisible()))
    await page.getByRole('button', { name: 'Toggle documents panel', exact: true }).click();
  await page.getByRole('button', { name: 'Sort', exact: true }).click();
  const rowInput = page.getByLabel('Date for Calendar fixture.pdf', { exact: true });
  await fillDate(rowInput, '1999-12-31');
  await page
    .getByRole('button', { name: 'Choose date for calendar fixture.pdf', exact: true })
    .click();
  await expect(popup).toContainText('December 1999');
  await popup.getByRole('button', { name: 'Next month', exact: true }).click();
  await popup
    .locator('[data-reka-calendar-cell-trigger]:not([data-outside-view])')
    .filter({ hasText: /^1$/ })
    .click();
  await expectDate(rowInput, '2000-01-01');
  await page.getByRole('button', { name: 'Save changes', exact: true }).click();
  await expect.poll(async () => (await read()).document_date).toBe('2000-01-01');
  expect((await read()).source_sha256).toBe(doc.source_sha256);
  await expect(page.locator('.batch-savebar .saved')).toHaveText('All changes saved');
  await page.getByRole('button', { name: 'More batch actions', exact: true }).click();
  await page.getByRole('button', { name: 'Close table', exact: true }).click();
  await page.getByRole('button', { name: 'Open editor', exact: true }).click();
  await page.getByRole('button', { name: 'More document actions', exact: true }).click();
  await page.getByRole('tab', { name: 'Stamps', exact: true }).click();
  await page.getByRole('button', { name: 'Date', exact: true }).click();
  const stampDate = page.getByLabel('Date shown', { exact: true });
  await fillDate(stampDate, '2024-02-28');
  await page.getByRole('button', { name: 'Choose date shown', exact: true }).click();
  await popup.locator('[data-value="2024-02-29"]').click();
  await expectDate(stampDate, '2024-02-29');
  expect((await read()).document_date).toBe('2000-01-01');
  expect(errors).toEqual([]);
});
