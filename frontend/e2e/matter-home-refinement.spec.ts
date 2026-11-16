import { expect, test } from '@playwright/test';
import { selectOption } from './select';

const base = `http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT || 8768}`;
const headers = { Authorization: 'Bearer e2e-isolated-token' };

test('Matters layout fits, reflows and retains its controls', async ({ page, request }, info) => {
  for (const name of [
    'Synthetic filing',
    'A deliberately long synthetic matter name for the keyboard and table width check',
    'Synthetic appeal',
  ]) {
    const result = await request.post(`${base}/matters`, {
      headers,
      data: { name, case_type: 'Synthetic demonstration', court_profile: 'general-demo' },
    });
    expect(result.ok()).toBe(true);
  }
  for (const [width, height] of [
    [1280, 800],
    [1440, 900],
    [1920, 1080],
    [1280, 500],
    [640, 800],
  ]) {
    await page.setViewportSize({ width, height });
    if (!page.url().endsWith('/')) await page.goto('/');
    else await page.reload();
    await expect(page.locator('.matter-table tbody tr')).toHaveCount(3);
    const metrics = await page.evaluate(() => {
      const rail = document.querySelector('.matter-rail')!;
      const rows = [...rail.querySelectorAll('button')];
      const sort = document.querySelector<HTMLElement>('[aria-label="Sort matters"]')!;
      const sortLabel = sort.querySelector<HTMLElement>('.app-select-label')!;
      const heading = document.querySelector('.matter-home h1')!;
      const table = document.querySelector('.matter-table-scroll')!;
      const activity = document.querySelector('.matter-recent')!;
      return {
        railWidth: rail.getBoundingClientRect().width,
        railScrollWidth: rail.scrollWidth,
        railClientWidth: rail.clientWidth,
        iconLefts: rows.map((row) => row.querySelector('svg')!.getBoundingClientRect().left),
        labelLefts: rows.map((row) => row.querySelector('span')!.getBoundingClientRect().left),
        sortLabelFits: sortLabel.scrollWidth <= sortLabel.clientWidth,
        edges: [heading, table, activity].map((el) => el.getBoundingClientRect().left),
        pageWidth: document.documentElement.scrollWidth,
      };
    });
    expect(metrics.railWidth).toBe(width <= 760 ? 64 : 240);
    expect(metrics.railScrollWidth).toBeLessThanOrEqual(metrics.railClientWidth);
    expect(new Set(metrics.iconLefts).size).toBe(1);
    expect(new Set(metrics.labelLefts).size).toBe(1);
    expect(metrics.sortLabelFits).toBe(true);
    expect(new Set(metrics.edges).size).toBe(1);
    expect(metrics.pageWidth).toBeLessThanOrEqual(width);
    if (width === 640) {
      await expect(
        page.locator('.matter-rail').getByRole('button', { name: 'Collapse sidebar' }),
      ).toBeInViewport();
    }
    await page.screenshot({ path: info.outputPath(`matters-${width}x${height}.png`) });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.reload();
  await expect(page.locator('.matter-table tbody tr')).toHaveCount(3);
  const measureContrast = () =>
    page.evaluate(() => {
      const ratio = (foreground: string, background: string) => {
        const luminance = (color: string) => {
          const channels = color
            .match(/[\d.]+/g)!
            .slice(0, 3)
            .map(Number);
          const linear = channels.map((value) => {
            const fraction = value / 255;
            return fraction <= 0.04045 ? fraction / 12.92 : ((fraction + 0.055) / 1.055) ** 2.4;
          });
          return linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722;
        };
        const values = [luminance(foreground), luminance(background)].sort((a, b) => b - a);
        return (values[0] + 0.05) / (values[1] + 0.05);
      };
      const sampled = (foreground: string, background: string) => {
        const fg = getComputedStyle(document.querySelector(foreground)!).color;
        const bg = getComputedStyle(document.querySelector(background)!).backgroundColor;
        return ratio(fg, bg);
      };
      return {
        heading: sampled('.matter-home h1', '.matter-home'),
        description: sampled('.flow-heading p', '.matter-home'),
        nav: sampled('.matter-rail button:not([aria-current])', '.matter-rail'),
        navIcon: sampled('.matter-rail button:not([aria-current]) svg', '.matter-rail'),
        selectedNav: sampled(
          '.matter-rail button[aria-current]',
          '.matter-rail button[aria-current]',
        ),
        selectedNavIcon: sampled(
          '.matter-rail button[aria-current] svg',
          '.matter-rail button[aria-current]',
        ),
        header: sampled('.matter-table th', '.matter-table th'),
        badge: sampled('.matter-table .flow-status', '.matter-table .flow-status'),
      };
    });
  const lightContrast = await measureContrast();
  expect(lightContrast.description).toBeGreaterThanOrEqual(4.5);
  expect(lightContrast.header).toBeGreaterThanOrEqual(4.5);
  expect(lightContrast.navIcon).toBeGreaterThanOrEqual(3);
  expect(lightContrast.selectedNavIcon).toBeGreaterThanOrEqual(3);
  await page.getByRole('button', { name: 'Workspace settings' }).click();
  await selectOption(page.getByRole('combobox', { name: 'Theme' }), 'dark');
  await selectOption(page.getByRole('combobox', { name: 'Interface size' }), '125');
  await page.getByRole('button', { name: 'Close workspace navigation' }).click();
  await page.screenshot({ path: info.outputPath('matters-dark-125.png') });
  const contrast = await measureContrast();
  expect(contrast.heading).toBeGreaterThanOrEqual(3);
  for (const key of ['description', 'nav', 'selectedNav', 'header', 'badge'] as const)
    expect(contrast[key], `${key} contrast`).toBeGreaterThanOrEqual(4.5);
  expect(contrast.navIcon).toBeGreaterThanOrEqual(3);
  expect(contrast.selectedNavIcon).toBeGreaterThanOrEqual(3);
  await page.getByRole('button', { name: 'Open sample matter' }).scrollIntoViewIfNeeded();
  await expect(page.getByRole('button', { name: 'Open sample matter' })).toBeInViewport();
  await page.getByRole('button', { name: 'Compact', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Compact', exact: true })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
  await selectOption(page.getByRole('combobox', { name: 'Sort matters' }), 'name');
  await expect(page.locator('.matter-home-card').first()).toContainText('A deliberately long');
  await page.locator('.matter-rail').getByRole('button', { name: 'Collapse sidebar' }).click();
  await expect(page.locator('.matter-rail')).toHaveCSS('width', '64px');
  await page.screenshot({ path: info.outputPath('matters-collapsed.png') });
  await page.locator('.matter-rail').getByRole('button', { name: 'Expand sidebar' }).click();
  await page.getByRole('textbox', { name: 'Search matters' }).fill('no matching matter');
  await expect(page.getByRole('heading', { name: 'No matching matters' })).toBeVisible();
  await page.screenshot({ path: info.outputPath('matters-empty-search.png') });
  await page.getByRole('textbox', { name: 'Search matters' }).fill('');
  await page.locator('.matter-home-card').first().click();
  await expect(page.getByRole('region', { name: 'Matter overview' })).toBeVisible();
});
