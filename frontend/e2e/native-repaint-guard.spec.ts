import { expect, test, type Locator, type Page } from '@playwright/test';

const base = `http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT || 8768}`;
const headers = { Authorization: 'Bearer e2e-isolated-token' };

async function paintedColorCount(page: Page, content: Locator): Promise<number> {
  const screenshot = await content.screenshot({ animations: 'disabled' });
  return page.evaluate(async (base64) => {
    const image = new Image();
    image.src = `data:image/png;base64,${base64}`;
    await image.decode();
    const canvas = document.createElement('canvas');
    canvas.width = image.width;
    canvas.height = image.height;
    const context = canvas.getContext('2d')!;
    context.drawImage(image, 0, 0);
    const pixels = context.getImageData(0, 0, canvas.width, canvas.height).data;
    const colors = new Set<string>();
    for (let y = 0; y < canvas.height; y += 8) {
      for (let x = 0; x < canvas.width; x += 8) {
        const offset = (y * canvas.width + x) * 4;
        colors.add(`${pixels[offset]},${pixels[offset + 1]},${pixels[offset + 2]}`);
      }
    }
    return colors.size;
  }, screenshot.toString('base64'));
}

test('native workspace transition guard retains painted content', async ({ page, request }) => {
  const created = await request.post(`${base}/matters`, {
    headers,
    data: { name: 'Synthetic native paint guard', court_profile: 'general-demo' },
  });
  expect(created.ok()).toBe(true);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/');
  await page.evaluate(() => (document.documentElement.dataset.nativeWebview = 'mac'));
  await page
    .locator('.matter-home-card')
    .filter({ hasText: 'Synthetic native paint guard' })
    .click();
  const overview = page.getByRole('region', { name: 'Matter overview' });
  await expect(
    overview.getByRole('heading', { name: 'Synthetic native paint guard' }),
  ).toBeVisible();
  await expect(page.locator('.view-enter-active,.section-enter-active')).toHaveCount(0);
  expect(await paintedColorCount(page, overview)).toBeGreaterThan(20);
  await page.locator('.matter-rail').getByRole('button', { name: 'Documents' }).click();
  const documents = page.locator('.documents-workspace');
  await expect(documents.getByRole('heading', { name: 'Documents' })).toBeVisible();
  await expect(page.locator('.view-enter-active,.section-enter-active')).toHaveCount(0);
  expect(await paintedColorCount(page, documents)).toBeGreaterThan(20);
});
