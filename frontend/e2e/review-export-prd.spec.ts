import { test, expect } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { openNamedMatter } from './navigation';
import { selectOption } from './select';
const base = `http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT || 8768}`;
const headers = { Authorization: 'Bearer e2e-isolated-token' };

test('review PRD: real comparison, retained revision restore, archive and immutable export', async ({
  page,
  request,
}, info) => {
  const matter = await (
    await request.post(`${base}/matters`, {
      headers,
      data: { name: 'Synthetic review PRD', court_profile: 'general-demo' },
    })
  ).json();
  const root = `${base}/matters/${matter.id}`;
  const original = await readFile(resolve(import.meta.dirname, '../../samples/01_petition.pdf'));
  const doc = (
    await (
      await request.post(`${root}/documents?included=true`, {
        headers,
        multipart: {
          files: { name: 'Synthetic petition.pdf', mimeType: 'application/pdf', buffer: original },
        },
      })
    ).json()
  )[0];
  const job = await (await request.post(`${root}/build`, { headers })).json();
  await expect
    .poll(
      async () => (await (await request.get(`${base}/jobs/${job.id}`, { headers })).json()).status,
    )
    .toBe('completed');
  const prior = await (
    await request.get(`${root}/exports/${job.id}/combined.pdf`, { headers })
  ).body();
  expect(
    (
      await request.post(`${base}/documents/${doc.id}/pages/reorder`, {
        headers,
        data: { order: [2, 0, 1] },
      })
    ).ok(),
  ).toBeTruthy();
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/');
  await openNamedMatter(page, matter.name);
  await page
    .locator('.matter-rail')
    .getByRole('button', { name: 'Build bundle', exact: true })
    .click();
  const workspace = page.locator('.build-workspace');
  await workspace.getByRole('button', { name: 'Review changes', exact: true }).click();
  await workspace.getByRole('button', { name: 'Compare PDF pages', exact: true }).click();
  const comparison = page.getByRole('dialog', { name: 'Compare PDF pages', exact: true });
  await expect(comparison.locator('.pdf-paper')).toHaveCount(2);
  await expect(comparison.locator('.pdf-paper').first()).toBeVisible();
  await comparison.getByLabel('Current source page', { exact: true }).fill('2');
  await comparison.getByRole('button', { name: 'Compare selected pages', exact: true }).click();
  await expect(comparison).toContainText('Working revision 2 · source 2');
  await expect(comparison).toContainText('source 1 · combined 2');
  await comparison
    .getByRole('checkbox', { name: 'Ignore generated numbering in text differences' })
    .check();
  await expect(comparison).toContainText('0 text changes');
  await comparison.getByRole('checkbox', { name: 'Sync scrolling' }).check();
  await selectOption(comparison.getByLabel('Previous build · source 1 · combined 2 zoom'), '125');
  await selectOption(comparison.getByLabel('Working revision 2 · source 2 zoom'), '125');
  const scrollers = comparison.locator('.shared-reader-viewport');
  await scrollers.first().evaluate((el) => {
    el.scrollTop = 200;
  });
  await expect.poll(() => scrollers.last().evaluate((el) => el.scrollTop)).toBeGreaterThan(100);
  await selectOption(
    comparison.getByLabel('Previous build · source 1 · combined 2 zoom'),
    'fit-page',
  );
  await selectOption(comparison.getByLabel('Working revision 2 · source 2 zoom'), 'fit-page');

  for (const [width, theme] of [
    [1440, 'light'],
    [1024, 'dark'],
  ] as const) {
    await page.setViewportSize({ width, height: 900 });
    await page.evaluate((theme) => {
      document.documentElement.dataset.theme = theme;
      document.documentElement.dataset.effectiveTheme = theme;
    }, theme);
    await expect(comparison.locator('.pdf-paper').last()).toBeVisible();
    await page.screenshot({
      path: info.outputPath(`comparison-${width}-${theme}.png`),
      animations: 'disabled',
    });
  }
  await comparison.getByRole('button', { name: 'Close comparison', exact: true }).click();
  await expect(
    workspace.getByRole('button', { name: 'Compare PDF pages', exact: true }),
  ).toBeFocused();
  await page.locator('.matter-rail').getByRole('button', { name: 'Storage', exact: true }).click();
  await workspace.getByRole('button', { name: /Working revisions.*current documents/ }).click();
  await workspace.getByRole('button', { name: 'Revision history', exact: true }).click();
  const history = page.getByRole('dialog', { name: 'Retained working revisions', exact: true });
  await selectOption(history.getByLabel('Saved revision'), '1');
  await expect(history.locator('.pdf-paper')).toBeVisible();
  await history.getByLabel('Preview source page').fill('2');
  await history.getByRole('checkbox').check();
  await page.screenshot({
    path: info.outputPath('revision-history-dark.png'),
    animations: 'disabled',
  });
  await history.getByRole('button', { name: 'Restore revision', exact: true }).click();
  const confirmation = page.getByRole('alertdialog', { name: 'Restore retained revision?' });
  await confirmation.getByRole('button', { name: 'Keep current revision' }).click();
  await expect(history).toBeVisible();
  await history.getByRole('button', { name: 'Restore revision', exact: true }).click();
  await confirmation.getByRole('button', { name: 'Restore as new revision' }).click();
  await expect(history).toBeHidden();
  await expect(workspace.locator('.stored-document-row')).toContainText('Revision 3');
  await workspace.getByRole('button', { name: /Generated bundles.*saved builds/ }).click();
  await workspace.getByRole('button', { name: 'Archive build', exact: true }).click();
  const archive = page.getByRole('alertdialog', { name: 'Archive saved build?' });
  await archive.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(workspace.locator('.generated-files')).toContainText(job.id);
  await workspace.getByRole('button', { name: 'Archive build', exact: true }).click();
  await archive.getByRole('button', { name: 'Archive build', exact: true }).click();
  await expect(workspace.locator('.generated-files')).toHaveCount(0);
  await workspace.getByRole('button', { name: /Archived builds.*preserved builds/ }).click();
  await expect(workspace.locator('.generated-files')).toContainText(job.id);
  await workspace.getByRole('button', { name: 'Review Combined PDF', exact: true }).click();
  const review = page.getByRole('dialog', { name: 'Review generated PDF' });
  const input = review.getByRole('spinbutton', { name: 'PDF page number' });
  await expect(review.locator('.pdf-paper').first()).toBeVisible();
  await input.fill('3');
  await input.press('Enter');
  await review.getByRole('button', { name: 'Return to index', exact: true }).click();
  await expect(input).toHaveValue('1');
  await review.getByRole('button', { name: 'View options', exact: true }).click();
  await page.getByRole('button', { name: 'Back to previous reading position' }).click();
  await expect(input).toHaveValue('3');
  await review.getByRole('button', { name: 'Forward to next reading position' }).click();
  await expect(input).toHaveValue('1');
  await review.getByRole('button', { name: 'Save a copy', exact: true }).click();
  const exp = page.getByRole('dialog', { name: 'Export bundle files' });
  await exp.getByRole('checkbox', { name: 'Release reviewed output' }).check();
  await expect(exp).toContainText('archived');
  await expect(exp.getByRole('button', { name: 'Export files', exact: true })).toBeDisabled();
  await exp.getByRole('button', { name: 'Cancel', exact: true }).click();
  await review.getByRole('button', { name: 'Close review', exact: true }).click();
  await workspace.getByRole('button', { name: 'Restore build', exact: true }).click();
  await page
    .getByRole('alertdialog', { name: 'Restore saved build?' })
    .getByRole('button', { name: 'Restore build', exact: true })
    .click();
  await workspace.getByRole('button', { name: /Generated bundles.*saved builds/ }).click();
  await expect(workspace.locator('.generated-files')).toContainText(job.id);
  expect(
    await (await request.get(`${root}/exports/${job.id}/combined.pdf`, { headers })).body(),
  ).toEqual(prior);
  expect(
    await (await request.get(`${base}/documents/${doc.id}/original`, { headers })).body(),
  ).toEqual(original);
});
