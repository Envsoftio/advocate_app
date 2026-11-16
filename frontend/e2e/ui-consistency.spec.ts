import { expect, test } from '@playwright/test';
import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';

const base = `http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT || 8768}`;
const headers = { Authorization: 'Bearer e2e-isolated-token' };

test('workspace pages retain readable type and bounded layout', async ({ page, request }, info) => {
  const result = await request.post(`${base}/matters`, {
    headers,
    data: {
      name: 'Synthetic consistency review with a long matter name',
      case_type: 'Synthetic demonstration',
      court_profile: 'general-demo',
    },
  });
  expect(result.ok()).toBe(true);
  const matter = await result.json();
  const upload = await request.post(`${base}/matters/${matter.id}/documents?included=true`, {
    headers,
    multipart: {
      files: {
        name: '01_petition.pdf',
        mimeType: 'application/pdf',
        buffer: await readFile(resolve(import.meta.dirname, '../../samples/01_petition.pdf')),
      },
    },
  });
  expect(upload.ok()).toBe(true);
  const audit: Array<Record<string, unknown>> = [];
  for (const config of [
    { width: 1440, height: 900, theme: 'light', scale: 100 },
    { width: 1024, height: 768, theme: 'dark', scale: 125 },
    { width: 640, height: 800, theme: 'light', scale: 100 },
  ] as const) {
    await page.setViewportSize({ width: config.width, height: config.height });
    await page.goto('/');
    await page.evaluate(({ theme, scale }) => {
      localStorage.setItem('courtbundle.appearance.v1', JSON.stringify({ theme, scale }));
      localStorage.setItem('courtbundle.sidebarCollapsed.v1', 'false');
    }, config);
    await page.reload();
    await expect(page.locator('.matter-home-card').first()).toBeVisible();
    const screens = [
      'matters',
      'overview',
      'import',
      'documents',
      'index',
      'validation',
      'build',
      'storage',
      'rules',
      'about',
    ];
    const selector: Record<string, string> = {
      matters: '.matter-home',
      overview: '.matter-overview',
      import: '.import-workspace',
      documents: '.documents-workspace',
      index: '.bundle-structure',
      validation: '.preflight-workspace',
      build: '.build-workspace',
      storage: '.matter-storage',
      rules: '.rules-page',
      about: '.about-page',
    };
    for (const screen of screens) {
      if (screen === 'overview') await page.locator('.matter-home-card').first().click();
      else if (screen === 'import') {
        await page
          .locator('.matter-overview')
          .getByRole('button', { name: 'Import documents' })
          .click();
      } else if (screen === 'rules' || screen === 'about') {
        await page.getByRole('button', { name: 'Workspace settings' }).click();
        await page
          .locator('.workspace-drawer')
          .getByRole('button', { name: screen === 'rules' ? 'Court rules' : 'About MatterDraft' })
          .click();
      } else if (screen !== 'matters') {
        const name: Record<string, string> = {
          documents: 'Documents',
          index: 'Index & bookmarks',
          validation: 'Preflight checks',
          build: 'Build bundle',
          storage: 'Storage',
        };
        await page.locator('.matter-rail').getByRole('button', { name: name[screen] }).click();
      }
      await expect(page.locator(selector[screen])).toBeVisible();
      await expect(page.locator('.section-enter-active,.section-leave-active')).toHaveCount(0);
      await expect(page.locator('.inspector-enter-active,.inspector-leave-active')).toHaveCount(0);
      if (screen === 'index') {
        await expect(page.locator('.structure-heading')).not.toContainText('Recalculating');
      }
      const measurements = await page.evaluate(() => {
        const shell = document.querySelector<HTMLElement>('.design-shell')!;
        const rail = document.querySelector<HTMLElement>('.matter-rail')!;
        const main = document.querySelector<HTMLElement>('.app-main')!;
        const region = document.querySelector<HTMLElement>(
          '.workspace-page, .matter-home, .rules-page, .about-page',
        )!;
        const headings = [...region.querySelectorAll<HTMLElement>('h1, h2')]
          .filter(
            (node) => node.getBoundingClientRect().width && node.getBoundingClientRect().height,
          )
          .slice(0, 6)
          .map((node) => ({
            text: node.textContent?.trim().slice(0, 50),
            size: getComputedStyle(node).fontSize,
            line: getComputedStyle(node).lineHeight,
            x: Math.round(node.getBoundingClientRect().x),
          }));
        return {
          pageWidth: document.documentElement.scrollWidth,
          viewport: innerWidth,
          shellWidth: shell.getBoundingClientRect().width,
          railWidth: rail.getBoundingClientRect().width,
          railOverflow: rail.scrollWidth - rail.clientWidth,
          railChildren: [...rail.children].map((node) => ({
            className: node.className,
            width: (node as HTMLElement).scrollWidth,
            client: (node as HTMLElement).clientWidth,
          })),
          mainOverflow: main.scrollWidth - main.clientWidth,
          regionOverflow: region.scrollWidth - region.clientWidth,
          headings,
        };
      });
      audit.push({ screen, ...config, ...measurements });
      expect(measurements.pageWidth, `${screen} page width`).toBeLessThanOrEqual(config.width);
      const compactProcessingRail =
        config.width <= 1024 && ['index', 'validation', 'build', 'storage'].includes(screen);
      expect(
        measurements.railOverflow,
        `${screen} sidebar overflow: ${JSON.stringify(measurements.railChildren)}`,
      ).toBeLessThanOrEqual(compactProcessingRail ? 1 : 0);
      if (compactProcessingRail) {
        expect(measurements.railWidth, `${screen} compact rail`).toBe(
          config.width === 1024 ? 80 : 64,
        );
        await expect(page.locator('.matter-rail')).toHaveCSS('overflow-x', 'hidden');
      }
      if (screen !== 'import') {
        expect(measurements.mainOverflow, `${screen} shell overflow`).toBeLessThanOrEqual(0);
      } else {
        const tableScroll = await page.locator('.import-table-scroll').evaluate((node) => ({
          scroll: node.scrollWidth,
          client: node.clientWidth,
          overflow: getComputedStyle(node).overflowX,
        }));
        expect(tableScroll.overflow).toBe('auto');
        expect(tableScroll.scroll).toBeGreaterThanOrEqual(tableScroll.client);
      }
      expect(measurements.regionOverflow, `${screen} page overflow`).toBeLessThanOrEqual(0);
      if (['overview', 'rules', 'about'].includes(screen)) {
        expect(measurements.headings[0]?.size).toBe('28px');
      }
      if (config.width === 1024 && screen === 'overview') {
        const columns = await page
          .locator('.overview-metrics')
          .evaluate((node) => getComputedStyle(node).gridTemplateColumns.split(' ').length);
        expect(columns).toBe(3);
      }
      if (config.width <= 1024 && ['rules', 'about'].includes(screen)) {
        const grid = screen === 'rules' ? '.rules-grid' : '.feature-strip';
        const columns = await page
          .locator(grid)
          .evaluate((node) => getComputedStyle(node).gridTemplateColumns.split(' ').length);
        expect(columns).toBe(1);
      }
      if (config.width === 1024 && screen === 'index') {
        const columns = await page.evaluate(() => [
          document.querySelector('.structure-order')!.getBoundingClientRect().x,
          document.querySelector('.structure-map')!.getBoundingClientRect().x,
        ]);
        expect(columns[0]).toBe(columns[1]);
      }
      await page.screenshot({
        path: info.outputPath(`${screen}-${config.width}-${config.theme}-${config.scale}.png`),
      });
      if (config.width === 1024 && screen === 'documents') {
        await page.getByRole('button', { name: 'Collapse documents' }).click();
        await expect(page.locator('.document-navigator')).toBeHidden();
        await page.screenshot({ path: info.outputPath('documents-preview-1024-dark-125.png') });
      }
      if (config.width !== 640 && screen === 'documents') {
        await page.getByRole('button', { name: 'Open editor' }).click();
        await expect(page.locator('.viewer-root')).toBeVisible();
        await expect(
          page.locator('.thumbnail-rail canvas[data-rendered="true"]').first(),
        ).toBeVisible();
        expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
          config.width,
        );
        await page.screenshot({
          path: info.outputPath(`editor-${config.width}-${config.theme}-${config.scale}.png`),
        });
        await page.locator('.matter-rail').getByRole('button', { name: 'Documents' }).click();
        await expect(page.locator('.preparation-workspace')).toBeVisible();
      }
    }
  }
  await writeFile(info.outputPath('metrics.json'), JSON.stringify(audit, null, 2));
});
