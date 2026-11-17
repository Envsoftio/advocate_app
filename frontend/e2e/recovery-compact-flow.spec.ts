import { test, expect } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { enterLatestMatter, openWorkspaceNavigation, closeWorkspaceNavigation } from './navigation';
import { selectOption } from './select';
const base = `http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT || 8768}`;
const headers = { Authorization: 'Bearer e2e-isolated-token' };
test('FLOW11 compact edits, design matrix and diagnostics recovery', async ({
  page,
  request,
  context,
}, info) => {
  test.setTimeout(180000);
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  const matter = await (
    await request.post(`${base}/matters`, {
      headers,
      data: { name: 'FLOW11 synthetic recovery', court_profile: 'general-demo' },
    })
  ).json();
  const bytes = await readFile(resolve(import.meta.dirname, '../../samples/01_petition.pdf'));
  const [doc] = await (
    await request.post(`${base}/matters/${matter.id}/documents?included=true`, {
      headers,
      multipart: {
        files: { name: 'Synthetic recovery.pdf', mimeType: 'application/pdf', buffer: bytes },
      },
    })
  ).json();
  await page.addInitScript(() => {
    localStorage.setItem('courtbundle.sidebarCollapsed.v1', 'true');
    localStorage.setItem(
      'courtbundle.appearance.v1',
      JSON.stringify({ theme: 'light', scale: 100 }),
    );
  });
  await page.setViewportSize({ width: 1024, height: 768 });
  await page.goto('/');
  await enterLatestMatter(page);
  const navigate = async (name: string) => {
    await page.locator('.matter-rail').getByRole('button', { name, exact: true }).click();
    const target = (
      {
        Documents: '.preparation-workspace',
        'Index & bookmarks': '.bundle-structure',
        'Build bundle': '.build-workspace',
        'Preflight checks': '.preflight-workspace',
      } as Record<string, string>
    )[name];
    await expect(page.locator(target)).toBeVisible();
    await expect(
      page.locator(
        '.section-enter-active,.section-leave-active,.view-enter-active,.view-leave-active',
      ),
    ).toHaveCount(0);
    if (name === 'Documents') await expect(page.locator('.pdf-paper')).toBeVisible();
    if (name === 'Build bundle')
      await expect(page.locator('.build-settings-card')).toContainText('Custom bundle settings');
    if (name === 'Index & bookmarks')
      await expect(page.locator('.structure-heading')).not.toContainText('Recalculating');
    if (name === 'Preflight checks')
      await expect(
        page.locator('.preflight-workspace').getByLabel('Latest order included', { exact: true }),
      ).toBeEnabled();
  };
  const appearance = async (theme: string, scale: number) => {
    await openWorkspaceNavigation(page);
    await selectOption(page.getByLabel('Theme', { exact: true }), theme);
    await selectOption(page.getByLabel('Interface size', { exact: true }), String(scale));
    await closeWorkspaceNavigation(page);
  };
  const capture = async (name: string) => {
    await page.evaluate(() => document.fonts.ready);
    await page.screenshot({ path: info.outputPath(`${name}.png`), animations: 'disabled' });
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
    ).toBeTruthy();
  };
  await navigate('Documents');
  await capture('22A-documents');
  await page.getByRole('button', { name: 'Toggle documents panel', exact: true }).click();
  await page.getByRole('button', { name: 'Sort', exact: true }).click();
  const table = page.locator('.batch-review');
  await expect(table).toHaveAttribute('data-compact', 'true');
  await expect(table.getByRole('columnheader', { name: 'Document date', exact: true })).toHaveCount(
    0,
  );
  const title = table.getByRole('textbox', {
    name: 'Title for Synthetic recovery.pdf',
    exact: true,
  });
  await title.fill('Reviewed compact title');
  await table.getByRole('button', { name: 'More fields', exact: true }).click();
  await expect(
    table.getByRole('columnheader', { name: 'Document date', exact: true }),
  ).toBeVisible();
  await table.getByRole('button', { name: 'Fewer fields', exact: true }).click();
  await table.getByRole('button', { name: 'Columns', exact: true }).click();
  const columns = page.locator('.app-popover.batch-columns');
  await expect(columns.getByLabel('File name', { exact: true })).not.toBeChecked();
  await columns.getByLabel('File name', { exact: true }).check();
  await expect(table.getByRole('columnheader', { name: 'File name', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await table.getByRole('button', { name: 'Fewer fields', exact: true }).click();
  await expect(title).toHaveValue('Reviewed compact title');
  await capture('22D-batch');
  await table.getByRole('button', { name: 'Save changes', exact: true }).click();
  await expect(table).toContainText('All changes saved');
  const records = await (
    await request.get(`${base}/matters/${matter.id}/documents`, { headers })
  ).json();
  expect(records[0].title).toBe('Reviewed compact title');
  expect(records[0].revision).toBe(doc.revision);
  expect(
    await (await request.get(`${base}/documents/${doc.id}/original`, { headers })).body(),
  ).toEqual(bytes);
  await navigate('Preflight checks');
  const preflight = page.locator('.preflight-workspace');
  await preflight.getByLabel('Latest order included', { exact: true }).check();
  await preflight.getByRole('button', { name: 'Run checks', exact: true }).click();
  await expect(preflight.getByLabel('Latest order included', { exact: true })).toBeChecked();
  for (const theme of ['light', 'dark'])
    for (const scale of [90, 100, 110, 125]) {
      await appearance(theme, scale);
      for (const [screen, name] of [
        ['22E', 'Preflight checks'],
        ['22B', 'Index & bookmarks'],
        ['22C', 'Build bundle'],
        ['22F', 'Documents'],
      ]) {
        await navigate(name);
        await capture(`${screen}-${theme}-${scale}`);
        if (screen === '22B' && scale === 100) {
          const order = (await page.locator('.structure-order').boundingBox())!;
          const map = (await page.locator('.structure-map').boundingBox())!;
          expect(Math.round(order.width)).toBe(238);
          expect(
            await page
              .locator('.page-map-scroll')
              .evaluate((el) => el.scrollWidth <= el.clientWidth + 1),
          ).toBeTruthy();
          expect(Math.round(map.x)).toBe(342);
          expect(Math.round(map.y)).toBe(Math.round(order.y));
        }
      }
    }
  await page.setViewportSize({ width: 1440, height: 900 });
  await appearance('light', 100);
  await navigate('Documents');
  await page.route(`${base}/**`, (route) => route.abort('connectionrefused'));
  const notice = page.locator('.engine-notice');
  await expect(notice).toBeVisible({ timeout: 20000 });
  await capture('21A-unavailable');
  await notice.getByRole('button', { name: 'View diagnostics' }).click();
  const drawer = page.getByRole('dialog', { name: 'Processing diagnostics' });
  await expect(drawer).toBeVisible();
  expect(Math.round((await drawer.boundingBox())!.width)).toBe(520);
  await drawer.getByRole('button', { name: 'Copy diagnostics' }).click();
  await expect(drawer.getByRole('status')).toHaveText('Diagnostics copied');
  const report = await page.evaluate(() => navigator.clipboard.readText());
  expect(report).toContain('Engine state: disconnected');
  expect(report).not.toContain('e2e-isolated-token');
  expect(report).not.toContain(matter.name);
  await capture('21B-diagnostics');
  await page.keyboard.press('Escape');
  await expect(notice.getByRole('button', { name: 'View diagnostics' })).toBeFocused();
  await notice.getByRole('button', { name: 'View diagnostics' }).click();
  await drawer.getByRole('button', { name: 'Continue browsing' }).click();
  await page.setViewportSize({ width: 1024, height: 768 });
  await appearance('dark', 125);
  await notice.getByRole('button', { name: 'View diagnostics' }).click();
  await capture('21B-diagnostics-dark-125');
  await expect(drawer.getByRole('button', { name: 'Continue browsing' })).toBeInViewport();
  await drawer.getByRole('button', { name: 'Continue browsing' }).click();
  await page.unroute(`${base}/**`);
  const writes: string[] = [];
  page.on('request', (r) => {
    if (
      r.url().startsWith(base) &&
      !['GET', 'HEAD'].includes(r.method()) &&
      !r.url().includes('/reader-position')
    )
      writes.push(r.url());
  });
  await notice.getByRole('button', { name: 'Retry connection', exact: true }).click();
  await expect(notice).toHaveCount(0);
  expect(writes).toEqual([]);
  await expect(page.locator('.pdf-paper')).toBeVisible();
});
