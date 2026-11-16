import { test, expect } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { enterLatestMatter } from './navigation';
const base = `http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT || 8768}`;
const headers = { Authorization: 'Bearer e2e-isolated-token' };
test('FLOW08 current checks, persisted review, stale reset, failure recovery and design states', async ({
  page,
  request,
}, info) => {
  const matter = await (
    await request.post(`${base}/matters`, {
      headers,
      data: { name: 'FLOW08 Synthetic preflight', court_profile: 'general-demo' },
    })
  ).json();
  const bytes = await readFile(resolve(import.meta.dirname, '../../samples/01_petition.pdf'));
  const doc = (
    await (
      await request.post(`${base}/matters/${matter.id}/documents?included=true`, {
        headers,
        multipart: {
          files: { name: 'Synthetic petition.pdf', mimeType: 'application/pdf', buffer: bytes },
        },
      })
    ).json()
  )[0];
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/');
  await enterLatestMatter(page);
  const open = async () => {
    await page
      .locator('.matter-rail')
      .getByRole('button', { name: 'Preflight checks', exact: true })
      .click();
  };
  await open();
  const panel = page.locator('.preflight-workspace');
  await expect(panel.getByLabel('Latest order included', { exact: true })).toBeEnabled();
  await expect(panel).toContainText('document date missing');
  const capture = async (name: string) => {
    await expect(page.locator('.section-enter-active')).toHaveCount(0);
    await page.evaluate(() => document.fonts.ready);
    await page.waitForTimeout(200); // Let Chrome repaint after viewport/zoom changes.
    await page.screenshot({ path: info.outputPath(name + '.png'), animations: 'disabled' });
  };
  await capture('14A-current-findings');
  for (const label of [
    'Latest order included',
    'Annexure sequence',
    'Document titles',
    'Pagination',
  ]) {
    const input = panel.getByLabel(label, { exact: true });
    await expect(input).toBeEnabled();
    await input.check();
    await expect(input).toBeChecked();
  }
  const acknowledge = panel.getByRole('button', {
    name: 'Acknowledge warning: document date missing',
  });
  await expect(acknowledge).toBeEnabled();
  await acknowledge.click();
  await expect(acknowledge).toHaveAttribute('aria-pressed', 'true');
  await expect(panel).toContainText('4 acknowledged');
  await panel.getByRole('button', { name: 'Run checks', exact: true }).click();
  await expect(panel.getByLabel('Latest order included', { exact: true })).toBeChecked();
  await capture('14B-reviewed');
  await page.reload();
  await enterLatestMatter(page);
  await open();
  await expect(panel.getByLabel('Pagination', { exact: true })).toBeChecked();
  await request.patch(`${base}/documents/${doc.id}`, {
    headers,
    data: { title: 'Changed current title' },
  });
  await panel.getByRole('button', { name: 'Run checks', exact: true }).click();
  await expect(panel.getByLabel('Pagination', { exact: true })).not.toBeChecked();
  await expect(panel).toContainText('4 remaining');
  await expect(acknowledge).toHaveAttribute('aria-pressed', 'false');
  await capture('14C-new-inputs');
  for (const [width, height, theme, scale] of [
    [1440, 900, 'dark', 1],
    [1024, 768, 'light', 1],
    [1024, 768, 'dark', 1.25],
    [640, 800, 'light', 1],
  ] as const) {
    await page.setViewportSize({ width, height });
    await page.evaluate(
      ({ theme, scale }) => {
        document.documentElement.dataset.theme = theme;
        document.documentElement.dataset.effectiveTheme = theme;
        document.documentElement.style.setProperty('--ui-scale', String(scale));
        document.documentElement.style.zoom = String(scale);
      },
      { theme, scale },
    );
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
    ).toBeTruthy();
    await capture(`preflight-${width}-${theme}-${scale}`);
  }
  await page.route('**/matters/*/validate', (route) =>
    route.fulfill({
      status: 503,
      body: '{"detail":"Synthetic unavailable check"}',
      contentType: 'application/json',
    }),
  );
  await panel.getByRole('button', { name: 'Run checks', exact: true }).click();
  await expect(panel.getByRole('button', { name: 'Build settings', exact: true })).toBeDisabled();
  await expect(panel).not.toContainText('No technical blockers');
  await page.unroute('**/matters/*/validate');
  await panel.getByRole('button', { name: 'Run checks', exact: true }).click();
  await expect(panel.getByRole('button', { name: 'Build settings', exact: true })).toBeEnabled();
  const original = await (
    await request.get(`${base}/documents/${doc.id}/original`, { headers })
  ).body();
  expect(original.equals(bytes)).toBeTruthy();
});

test('FLOW08 build blockers expose working repair actions and never appear passed', async ({
  page,
  request,
}, info) => {
  const matter = await (
    await request.post(`${base}/matters`, {
      headers,
      data: { name: 'FLOW08 Synthetic blockers', court_profile: 'general-demo' },
    })
  ).json();
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/');
  await enterLatestMatter(page);
  const open = async () =>
    page
      .locator('.matter-rail')
      .getByRole('button', { name: 'Preflight checks', exact: true })
      .click();
  await open();
  const panel = page.locator('.preflight-workspace');
  await expect(panel).toContainText('no documents');
  await expect(panel.getByRole('button', { name: 'Build settings', exact: true })).toBeDisabled();
  await panel.getByRole('button', { name: 'Choose documents', exact: true }).click();
  await expect(page.locator('.bundle-membership')).toBeVisible();
  // Reload closes the empty membership editor; import only this synthetic fixture.
  const bytes = await readFile(resolve(import.meta.dirname, '../../samples/01_petition.pdf'));
  await request.post(`${base}/matters/${matter.id}/documents?included=true`, {
    headers,
    multipart: {
      files: { name: 'Oversized synthetic.pdf', mimeType: 'application/pdf', buffer: bytes },
    },
  });
  await request.patch(`${base}/matters/${matter.id}`, {
    headers,
    data: { max_pages_per_volume: 2 },
  });
  await page.reload();
  await enterLatestMatter(page);
  await open();
  await expect(panel).toContainText('page map unavailable');
  await expect(panel.getByRole('button', { name: 'Build settings', exact: true })).toBeDisabled();
  await page.screenshot({ path: info.outputPath('14A-blocked.png'), animations: 'disabled' });
  await panel.getByRole('button', { name: 'Review volumes', exact: true }).click();
  await expect(page.getByRole('heading', { name: /Plan volumes/ })).toBeVisible();
  await page.getByRole('button', { name: 'Back to Preflight', exact: true }).click();
  await expect(panel).toBeVisible();
  await request.patch(`${base}/matters/${matter.id}`, {
    headers,
    data: { max_pages_per_volume: 300 },
  });
  await page.reload();
  await enterLatestMatter(page);
  await open();
  await expect(panel.getByRole('button', { name: 'Build settings', exact: true })).toBeEnabled();
  const checkbox = panel.getByLabel('Pagination', { exact: true });
  await expect(checkbox).toBeEnabled();
  await checkbox.focus();
  await page.keyboard.press('Space');
  await expect(checkbox).toBeChecked();
});
