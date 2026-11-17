import { expect, test } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { enterLatestMatter } from './navigation';
const base = `http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT || 8768}`;
const headers = { Authorization: 'Bearer e2e-isolated-token' };
test('FLOW12/13 properties retain drafts and save through the real API', async ({
  page,
  request,
}, info) => {
  const matter = await (
    await request.post(`${base}/matters`, {
      headers,
      data: { name: 'Synthetic compact editor', court_profile: 'general-demo' },
    })
  ).json();
  const bytes = await readFile(resolve(import.meta.dirname, '../../samples/01_petition.pdf'));
  const upload = await request.post(`${base}/matters/${matter.id}/documents?included=true`, {
    headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf', buffer: bytes } },
  });
  expect(upload.ok()).toBe(true);
  const doc = (await upload.json())[0];
  expect(
    (
      await request.post(`${base}/documents/${doc.id}/annotations`, {
        headers,
        data: {
          kind: 'comment',
          page_index: 0,
          expected_revision: doc.revision,
          rect_x: 0.15,
          rect_y: 0.2,
          rect_width: 0.06,
          rect_height: 0.04,
          body: 'Review date',
          color: 'yellow',
          output_scope: 'private',
        },
      })
    ).ok(),
  ).toBe(true);
  await page.setViewportSize({ width: 1024, height: 768 });
  await page.goto('/');
  await enterLatestMatter(page);
  await expect(page.locator('.pdf-paper')).toBeVisible();
  await page.screenshot({ animations: 'disabled', path: info.outputPath('ED36-read.png') });
  await page.getByRole('tab', { name: 'Edit', exact: true }).click();
  const panel = page.getByLabel('PDF editing controls');
  await expect(panel).toBeHidden();
  await page.screenshot({ animations: 'disabled', path: info.outputPath('ED37-edit.png') });
  const toggle = page.getByRole('button', { name: 'Toggle context inspector' });
  await toggle.click();
  await expect(panel).toBeVisible();
  await toggle.click();
  await page.locator('.pdf-review-comment').click();
  const body = panel.getByRole('textbox', { name: 'Comment text' });
  await body.fill('Retained compact review');
  const widthBefore = (await page.locator('.viewer-page-column').boundingBox())!.width;
  await body.press('Escape');
  await expect(panel).toBeHidden();
  await expect(toggle).toBeFocused();
  expect((await page.locator('.viewer-page-column').boundingBox())!.width).toBeCloseTo(
    widthBefore,
    0,
  );
  await toggle.click();
  await expect(body).toHaveValue('Retained compact review');
  await page.screenshot({ animations: 'disabled', path: info.outputPath('ED38B-properties.png') });
  await panel.getByRole('button', { name: 'Update mark' }).click();
  await expect
    .poll(
      async () =>
        (
          await (await request.get(`${base}/matters/${matter.id}/annotations`, { headers })).json()
        )[0]?.body,
    )
    .toBe('Retained compact review');
  expect(
    await (await request.get(`${base}/documents/${doc.id}/original`, { headers })).body(),
  ).toEqual(bytes);
  for (const width of [1024, 1440]) {
    await page.setViewportSize({ width, height: width === 1024 ? 768 : 900 });
    for (const theme of ['light', 'dark'])
      for (const scale of [90, 100, 110, 125]) {
        await page.evaluate(
          ({ scale, theme }) =>
            localStorage.setItem('courtbundle.appearance.v1', JSON.stringify({ theme, scale })),
          { scale, theme },
        );
        await page.reload();
        await enterLatestMatter(page);
        await expect(page.locator('.pdf-paper')).toBeVisible();
        await page.screenshot({
          animations: 'disabled',
          path: info.outputPath(`ED40-read-${width}-${theme}-${scale}.png`),
        });
        await page.getByRole('tab', { name: 'Edit', exact: true }).click();
        await page.getByRole('button', { name: 'Edit text', exact: true }).click();
        await expect(panel).toBeVisible();
        if (width === 1024 && scale === 100)
          expect((await page.locator('.viewer-page-column').boundingBox())!.width).toBeGreaterThan(
            700,
          );
        await page.locator('.pdf-text-run').first().click();
        await expect(
          panel.getByRole('textbox', { name: 'Replacement', exact: true }),
        ).toBeVisible();
        await page.screenshot({
          animations: 'disabled',
          path: info.outputPath(`ED41-text-${width}-${theme}-${scale}.png`),
        });
        await page.getByRole('tab', { name: 'Annotate', exact: true }).click();
        await page.screenshot({
          animations: 'disabled',
          path: info.outputPath(`ED43-annotate-${width}-${theme}-${scale}.png`),
        });
        await page.getByRole('tab', { name: 'Pages', exact: true }).click();
        await page.getByRole('button', { name: 'Organize pages', exact: true }).click();
        await expect(
          page.locator('.organizer-thumbnail canvas.thumbnail-ready').first(),
        ).toBeVisible();
        await page.getByRole('checkbox', { name: 'Select page 1', exact: true }).check();
        await page.screenshot({
          animations: 'disabled',
          path: info.outputPath(`ED42-pages-${width}-${theme}-${scale}.png`),
        });
        await page.getByRole('button', { name: 'Close page organizer' }).click();
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
          true,
        );
      }
  }
});
