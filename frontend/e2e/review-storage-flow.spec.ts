import { test, expect } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { openNamedMatter } from './navigation';
import { selectOption } from './select';

const base = `http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT || 8768}`;
const headers = { Authorization: 'Bearer e2e-isolated-token' };

test('FLOW10 storage without builds, archive restore and selected prior-build routing', async ({
  page,
  request,
}, info) => {
  const matter = await (
    await request.post(`${base}/matters`, {
      headers,
      data: {
        name: 'FLOW10 synthetic retained files',
        court_profile: 'general-demo',
      },
    })
  ).json();
  const root = `${base}/matters/${matter.id}`;
  const buffer = await readFile(resolve(import.meta.dirname, '../../samples/01_petition.pdf'));
  const docs = [];
  for (const name of ['Included source.pdf', 'Reference only.pdf']) {
    const response = await request.post(`${root}/documents`, {
      headers,
      multipart: {
        files: { name, mimeType: 'application/pdf', buffer },
      },
    });
    expect(response.ok()).toBeTruthy();
    docs.push((await response.json())[0]);
  }
  const membership = await (await request.get(`${root}/membership`, { headers })).json();
  expect(
    (
      await request.put(`${root}/membership`, {
        headers,
        data: {
          included_ids: [docs[0].id],
          expected_snapshot: membership.input_snapshot,
        },
      })
    ).ok(),
  ).toBeTruthy();
  await page.goto('/');
  await openNamedMatter(page, matter.name);
  const storage = async () => {
    await page
      .locator('.matter-rail')
      .getByRole('button', { name: 'Storage', exact: true })
      .click();
    await expect(page.locator('.build-workspace')).toHaveAttribute('data-screen', 'storage');
  };
  await storage();
  const workspace = page.locator('.build-workspace');
  await expect(workspace).toContainText('No saved builds');
  const originals = workspace.getByRole('button', { name: /Imported originals.*files/ });
  await originals.focus();
  await page.keyboard.press('Enter');
  await expect(workspace.locator('.stored-document-row')).toHaveCount(2);
  await expect(workspace.locator('.stored-documents')).toContainText('Reference-only');
  for (const [width, theme] of [
    [1440, 'light'],
    [1024, 'dark'],
    [640, 'light'],
  ] as const) {
    await page.setViewportSize({ width, height: 900 });
    await page.evaluate((theme) => {
      document.documentElement.dataset.theme = theme;
      document.documentElement.dataset.effectiveTheme = theme;
    }, theme);
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
    ).toBeTruthy();
    const content = await workspace.locator('.stored-documents').boundingBox();
    expect(content!.width).toBeGreaterThan(400);
    await page.screenshot({
      path: info.outputPath(`originals-${width}-${theme}.png`),
      animations: 'disabled',
    });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
  expect((await request.delete(`${base}/documents/${docs[1].id}`, { headers })).ok()).toBeTruthy();
  await page.reload();
  await openNamedMatter(page, matter.name);
  await storage();
  await workspace.getByRole('button', { name: /Archived documents.*retained/ }).click();
  await expect(workspace.locator('.stored-document-row')).toHaveCount(1);
  await workspace.getByRole('button', { name: 'Restore', exact: true }).click();
  await expect(workspace).toContainText('No archived documents');
  const original = await request.get(`${base}/documents/${docs[1].id}/original`, { headers });
  expect(await original.body()).toEqual(buffer);
  const build = async () => {
    const response = await request.post(`${root}/build`, { headers });
    expect(response.ok()).toBeTruthy();
    const job = await response.json();
    await expect
      .poll(
        async () =>
          (await (await request.get(`${base}/jobs/${job.id}`, { headers })).json()).status,
      )
      .toBe('completed');
    return job.id as string;
  };
  const prior = await build();
  expect(
    (
      await request.patch(`${base}/documents/${docs[0].id}`, {
        headers,
        data: { title: 'Updated included source' },
      })
    ).ok(),
  ).toBeTruthy();
  const latest = await build();
  await page.reload();
  await openNamedMatter(page, matter.name);
  await storage();
  await selectOption(workspace.getByLabel('Saved build'), prior);
  await expect(workspace.locator('.generated-files')).toContainText(prior);
  await workspace.getByRole('button', { name: 'Review Combined PDF', exact: true }).click();
  const review = page.getByRole('dialog', { name: 'Review generated PDF' });
  await expect(review.locator('.pdf-paper').first()).toBeVisible();
  await expect(review).toContainText(prior.slice(0, 8));
  await expect(review).not.toContainText(latest.slice(0, 8));
  await review.getByRole('button', { name: 'Close review', exact: true }).click();
  await workspace.getByRole('button', { name: 'Review changes', exact: true }).click();
  await expect(workspace).toContainText('title changed');
  await workspace.getByRole('button', { name: 'Choose update policy', exact: true }).click();
  await expect(workspace).toHaveAttribute('data-screen', 'stale');
  await expect(workspace.locator('.stale-summary')).toContainText(prior.slice(0, 8));
});
