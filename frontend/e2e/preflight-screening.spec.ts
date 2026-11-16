import { test, expect } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { resolve } from 'node:path';
import { enterLatestMatter } from './navigation';
const base = `http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT || 8768}`;
const headers = { Authorization: 'Bearer e2e-isolated-token' };
test('Preflight screens real pages, retains originals and rejects stale classifications', async ({
  page,
  request,
}, info) => {
  test.setTimeout(120000);
  const matter = await (
    await request.post(`${base}/matters`, {
      headers,
      data: { name: 'Synthetic page screening', court_profile: 'general-demo' },
    })
  ).json();
  const bytes = execFileSync(resolve(import.meta.dirname, '../../backend/.venv312/bin/python'), [
    '-c',
    `import pymupdf,sys
source=pymupdf.open();p=source.new_page(width=300,height=200);p.insert_text((20,60),'SYNTHETIC OCR REVIEW',fontsize=18)
image=p.get_pixmap(matrix=pymupdf.Matrix(2,2)).tobytes('png')
pdf=pymupdf.open();pdf.new_page(width=300,height=200);p=pdf.new_page(width=300,height=200);p.insert_image(p.rect,stream=image)
sys.stdout.buffer.write(pdf.tobytes())`,
  ]);
  const doc = (
    await (
      await request.post(`${base}/matters/${matter.id}/documents?included=true`, {
        headers,
        multipart: { files: { name: 'Screening.pdf', mimeType: 'application/pdf', buffer: bytes } },
      })
    ).json()
  )[0];
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
  const screening = page.getByRole('region', { name: 'Blank page and OCR screening' });
  await expect(screening).toContainText('Not run for current inputs');
  await expect(screening.getByRole('button', { name: 'Screen pages' })).toBeEnabled();
  await screening.getByRole('button', { name: 'Screen pages' }).click();
  await expect(screening).toContainText('Current results available', { timeout: 60000 });
  await expect(panel).toContainText('possible blank page');
  await expect(panel.getByRole('button', { name: 'Review source 1', exact: true })).toBeEnabled();
  const findings = await (
    await request.get(`${base}/matters/${matter.id}/validate`, { headers })
  ).json();
  expect(
    findings.some(
      (f: any) =>
        f.code === 'PAGE_SCREENING_COMPLETED' &&
        f.message.includes('OCR measurements available for 1'),
    ),
  ).toBeTruthy();
  expect(
    (await (await request.get(`${base}/documents/${doc.id}/original`, { headers })).body()).equals(
      bytes,
    ),
  ).toBeTruthy();
  expect(
    (await (await request.get(`${base}/documents/${doc.id}/pdf`, { headers })).body()).equals(
      bytes,
    ),
  ).toBeTruthy();
  for (const [width, height, theme, scale] of [
    [1440, 900, 'light', 1],
    [1024, 768, 'dark', 1.25],
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
    await page.evaluate(() => document.fonts.ready);
    await page.evaluate(() => {
      window.scrollTo(0, 0);
      document.querySelectorAll<HTMLElement>('body *').forEach((element) => {
        if (element.scrollTop) element.scrollTop = 0;
      });
    });
    await page.waitForTimeout(200);
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
    ).toBeTruthy();
    await page.screenshot({
      path: info.outputPath(`screening-${theme}.png`),
      animations: 'disabled',
    });
  }
  await page.reload();
  await enterLatestMatter(page);
  await open();
  await expect(screening).toContainText('Current results available');
  await request.patch(`${base}/documents/${doc.id}`, {
    headers,
    data: { title: 'Changed source title' },
  });
  await panel.getByRole('button', { name: 'Run checks', exact: true }).click();
  await expect(screening).toContainText('Not run for current inputs');
  await expect(
    panel.locator('.preflight-finding strong').filter({ hasText: /^possible blank page$/ }),
  ).toHaveCount(0);
});
