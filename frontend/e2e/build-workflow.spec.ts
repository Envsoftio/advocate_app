import { selectOption } from './select';
import { openNamedMatter } from './navigation';
import { enterLatestMatter } from './navigation';
import { test, expect } from '@playwright/test';
import { openWorkspaceNavigation } from './navigation';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { createHash } from 'node:crypto';
const base = `http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT || 8768}`;
const headers = { Authorization: 'Bearer e2e-isolated-token' };
const hash = (b: Buffer) => createHash('sha256').update(b).digest('hex');
async function fixture(request: any) {
  const matter = await (
    await request.post(`${base}/matters`, {
      headers,
      data: {
        name: 'Synthetic build review',
        case_number: 'DEMO 2481/2026',
        court_profile: 'general-demo',
      },
    })
  ).json();
  const buffer = await readFile(resolve(import.meta.dirname, '../../samples/01_petition.pdf'));
  const docs = [];
  for (const name of ['Petition.pdf', 'Reference notes.pdf']) {
    const r = await request.post(`${base}/matters/${matter.id}/documents`, {
      headers,
      multipart: { files: { name, mimeType: 'application/pdf', buffer } },
    });
    expect(r.ok()).toBeTruthy();
    docs.push((await r.json())[0]);
  }
  const membership = await (
    await request.get(`${base}/matters/${matter.id}/membership`, { headers })
  ).json();
  await request.put(`${base}/matters/${matter.id}/membership`, {
    headers,
    data: { included_ids: [docs[0].id], expected_snapshot: membership.input_snapshot },
  });
  return { matter, docs, buffer };
}
async function enter(page: any) {
  await page
    .locator('.matter-rail')
    .getByRole('button', { name: 'Build bundle', exact: true })
    .click();
  await expect(page.locator('.build-workspace')).toBeVisible();
  await expect(page.locator('.section-enter-active')).toHaveCount(0);
  await page.evaluate(() => document.fonts.ready);
}
async function capture(page: any, info: any, name: string) {
  const dismiss = page.locator('.toast').getByRole('button');
  if (await dismiss.isVisible()) {
    await dismiss.click();
    await expect(page.locator('.toast')).toBeHidden();
  }
  await page.screenshot({ path: info.outputPath(name + '.png'), animations: 'disabled' });
  if (name.includes('15A-') || name.includes('15A-settings')) return;
  for (const size of [
    { width: 1440, height: 900 },
    { width: 1024, height: 768 },
  ])
    for (const theme of ['light', 'dark'])
      for (const scale of [90, 100, 110, 125]) {
        await page.setViewportSize(size);
        await page.evaluate(
          ({ theme, scale }) => {
            document.documentElement.dataset.theme = theme;
            document.documentElement.dataset.effectiveTheme = theme;
            document.documentElement.style.setProperty('--ui-scale', String(scale / 100));
            document.documentElement.style.zoom = scale + '%';
          },
          { theme, scale },
        );
        expect(
          await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
        ).toBeTruthy();
        const dialog = page.locator('.generated-review[open]');
        if (await dialog.count()) {
          const paper = dialog.locator('.pdf-paper').first();
          await expect(paper).toBeVisible();
          const box = (await paper.boundingBox())!;
          expect(box.width).toBeGreaterThan(200);
          const area = (await dialog.locator('.shared-reader-viewport').boundingBox())!;
          expect(area.height).toBeGreaterThan(180);
        }
        await page.screenshot({
          path: info.outputPath(`${name}-${size.width}-${theme}-${scale}.png`),
          animations: 'disabled',
        });
      }
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.evaluate(() => {
    document.documentElement.dataset.theme = 'light';
    document.documentElement.dataset.effectiveTheme = 'light';
    document.documentElement.style.setProperty('--ui-scale', '1');
    document.documentElement.style.zoom = '100%';
  });
}
test('B01B real failure/retry, persisted output, generated review and matching export', async ({
  page,
  request,
}, info) => {
  test.setTimeout(240_000);
  const { matter, docs, buffer } = await fixture(request);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/');
  await enterLatestMatter(page);
  await expect(page.locator('.pdf-paper')).toBeVisible();
  await enter(page);
  const workspace = page.locator('.build-workspace');
  await expect(workspace).toContainText('1 included documents · 1 reference-only');
  for (const [selector, x, y, width, height] of [
    ['.build-settings-card', 264, 276, 826, 518],
    ['.build-summary', 1106, 276, 310, 518],
  ] as const) {
    const box = (await page.locator(selector).boundingBox())!;
    expect(Math.round(box.x)).toBe(x);
    expect(Math.round(box.y)).toBe(y);
    expect(Math.round(box.width)).toBe(width);
    expect(Math.round(box.height)).toBe(height);
  }
  await capture(page, info, '15A-settings-1440-light');
  await workspace.getByRole('button', { name: 'Plan volumes', exact: true }).click();
  const tool = page.locator('.volume-workspace');
  await tool.getByLabel('Maximum pages per volume').fill('2');
  await tool.getByRole('button', { name: 'Preview changes', exact: true }).click();
  await expect(tool).toContainText('No document was split');
  await expect(tool.getByRole('button', { name: 'Save choices', exact: true })).toBeDisabled();
  await tool.getByRole('button', { name: 'Back to build', exact: true }).click();
  await page
    .getByRole('dialog', { name: 'Unsaved volume choices' })
    .getByRole('button', { name: 'Discard changes' })
    .click();
  await request.patch(`${base}/matters/${matter.id}`, {
    headers,
    data: { max_pages_per_volume: 2 },
  });
  await page.reload();
  await enterLatestMatter(page);
  await enter(page);
  await expect(workspace.getByRole('button', { name: 'Plan volumes', exact: true })).toBeVisible();
  await workspace.getByRole('button', { name: 'Build bundle', exact: true }).click();
  await expect(workspace).toContainText('Build failed');
  await capture(page, info, '16A-failure');
  await page.reload();
  await enterLatestMatter(page);
  await enter(page);
  await expect(workspace).toContainText('Build failed');
  await workspace.getByRole('button', { name: 'Build settings', exact: true }).click();
  await workspace.getByRole('button', { name: 'Plan volumes', exact: true }).click();
  await tool.getByLabel('Maximum pages per volume').fill('300');
  await tool.getByRole('button', { name: 'Preview changes', exact: true }).click();
  await tool.getByRole('button', { name: 'Save choices', exact: true }).click();
  await expect(tool.getByRole('button', { name: 'Save choices', exact: true })).toBeHidden();
  await page.keyboard.press('Escape');
  // Delay delivery of a real job result so the actual queued/running UI is observable.
  let release!: () => void;
  const held = new Promise<void>((r) => (release = r));
  await page.route('**/jobs/*', async (route) => {
    if (route.request().method() === 'GET') {
      const response = await route.fetch();
      await held;
      await route.fulfill({ response });
    } else await route.continue();
  });
  await workspace.getByRole('button', { name: 'Build bundle', exact: true }).click();
  await expect(workspace).toContainText('Building bundle');
  await capture(page, info, '15B-progress');
  await expect(workspace).not.toContainText('A failed build');
  await expect(workspace.getByRole('list', { name: 'Build stages' })).toBeVisible();
  let cancelRequests = 0;
  page.on('request', (request) => {
    if (request.url().endsWith('/cancel')) cancelRequests++;
  });
  await workspace.getByRole('button', { name: 'Cancel build', exact: true }).click();
  const cancel = page.getByRole('alertdialog', { name: 'Cancel this build?' });
  await expect(cancel).toBeVisible();
  await expect(cancel.getByRole('button', { name: 'Keep building' })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(cancel).toBeHidden();
  expect(cancelRequests).toBe(0);
  await expect(workspace.getByRole('button', { name: 'Cancel build', exact: true })).toBeFocused();
  await workspace.getByRole('button', { name: 'Cancel build', exact: true }).click();
  await cancel.getByRole('button', { name: 'Cancel build', exact: true }).click();
  await expect.poll(() => cancelRequests).toBe(1);
  await expect(workspace).toContainText('Cancellation requested');
  await expect(workspace).not.toContainText('Build cancelled');
  release();
  await expect(workspace.getByRole('heading', { name: 'Bundle ready for review' })).toBeVisible();
  await page.unroute('**/jobs/*');
  const builds = await (
    await request.get(`${base}/matters/${matter.id}/exports`, { headers })
  ).json();
  const build = builds[0];
  await expect(workspace).toContainText(build.id);
  await expect(workspace.getByRole('row').filter({ hasText: 'combined.pdf' })).toContainText(
    String(build.pages),
  );
  await capture(page, info, '17A-complete');
  await workspace.getByRole('button', { name: 'Review bundle', exact: true }).click();
  const review = page.getByRole('dialog', { name: 'Review generated PDF' });
  await expect(review.locator('.pdf-paper').first()).toBeVisible();
  await expect(review).toContainText(build.id.slice(0, 8));
  await expect(review.getByRole('button', { name: 'Output checks', exact: true })).toBeEnabled();
  await expect(review.locator('.artifact-info-panel')).toHaveCount(0);
  await expect(review.getByLabel('Document version')).toHaveCount(0);
  await capture(page, info, '18A-review');
  await review.getByRole('button', { name: 'Export files', exact: true }).click();
  const exp = page.getByRole('dialog', { name: 'Export bundle files', exact: true });
  await selectOption(exp.getByLabel('File name'), 'combined.pdf');
  await capture(page, info, '18B-export');
  const exportPath = `**/matters/${matter.id}/exports/${build.id}/combined.pdf`;
  await page.route(exportPath, (route) =>
    route.fulfill({
      status: 503,
      contentType: 'application/json',
      body: JSON.stringify({ detail: 'Synthetic export temporarily unavailable' }),
    }),
  );
  await exp.getByRole('button', { name: 'Export files', exact: true }).click();
  await expect(exp).toContainText('Synthetic export temporarily unavailable');
  await expect(exp.getByRole('button', { name: 'Retry export', exact: true })).toBeEnabled();
  await page.unroute(exportPath);
  const downloaded = page.waitForEvent('download');
  await exp.getByRole('button', { name: 'Retry export', exact: true }).click();
  const download = await downloaded;
  const bytes = await readFile((await download.path())!);
  const source = await (
    await request.get(`${base}/matters/${matter.id}/exports/${build.id}/combined.pdf`, { headers })
  ).body();
  expect(hash(bytes)).toBe(hash(source));
  expect(hash(bytes)).toBe(build.files.find((f: any) => f.name === 'combined.pdf').sha256);
  await expect(exp).toContainText('Download requested');
  await exp.getByRole('button', { name: 'Close', exact: true }).click();
  await expect(review.getByRole('button', { name: 'Export files', exact: true })).toBeFocused();
  await review.getByRole('button', { name: 'Close review', exact: true }).click();
  await page.reload();
  await enterLatestMatter(page);
  await enter(page);
  await expect(workspace).toContainText(build.id);
  const original = await (
    await request.get(`${base}/documents/${docs[0].id}/original`, { headers })
  ).body();
  expect(hash(original)).toBe(hash(buffer));
  await request.patch(`${base}/documents/${docs[0].id}`, {
    headers,
    data: { title: 'Petition — updated working title' },
  });
  await page.reload();
  await enterLatestMatter(page);
  await enter(page);
  await expect(workspace.getByRole('heading', { name: 'Bundle is out of date' })).toBeVisible();
  await capture(page, info, '19A-stale');
  await workspace.getByRole('button', { name: 'Review changes', exact: true }).click();
  await expect(workspace).toContainText('title changed');
  await capture(page, info, '19B-changes');
  await workspace.getByRole('button', { name: 'Open earlier bundle', exact: true }).click();
  await expect(review.locator('.pdf-paper').first()).toBeVisible();
  await expect(review).toContainText('Earlier output remains unchanged');
  const railAlignment = async () =>
    page.evaluate(() => {
      const rail = document.querySelector('.matter-rail')!.getBoundingClientRect();
      const dialog = document.querySelector('.generated-review[open]')!.getBoundingClientRect();
      return {
        railRight: rail.right,
        dialogLeft: dialog.left,
        dialogRight: dialog.right,
        viewport: innerWidth,
      };
    });
  expect(
    Math.abs((await railAlignment()).railRight - (await railAlignment()).dialogLeft),
  ).toBeLessThanOrEqual(1);
  await capture(page, info, '18C-stale-review');
  await page.setViewportSize({ width: 860, height: 900 });
  await page.evaluate(() => {
    document.documentElement.style.setProperty('--ui-scale', '1.25');
    document.documentElement.style.zoom = '125%';
  });
  await expect(review.getByRole('button', { name: 'PDF bookmarks' })).toBeVisible();
  if (!(await review.locator('.artifact-navigation').isVisible()))
    await review.getByRole('button', { name: 'PDF bookmarks' }).click();
  await expect(review.locator('.artifact-navigation')).toBeVisible();
  const compact = await review.evaluate((element) => {
    const dialog = element.getBoundingClientRect(),
      navigation = element.querySelector('.artifact-navigation')!.getBoundingClientRect(),
      document = element.querySelector('.export-review-document')!.getBoundingClientRect();
    return {
      dialogRight: dialog.right,
      viewport: window.innerWidth,
      navigationRight: navigation.right,
      documentLeft: document.left,
      documentWidth: document.width,
    };
  });
  expect(compact.dialogRight).toBeLessThanOrEqual(compact.viewport + 1);
  expect(compact.documentWidth).toBeGreaterThan(300);
  expect(compact.navigationRight).toBeLessThanOrEqual(
    compact.documentLeft + compact.documentWidth + 1,
  );
  expect(
    Math.abs((await railAlignment()).railRight - (await railAlignment()).dialogLeft),
  ).toBeLessThanOrEqual(1);
  await page.screenshot({
    path: info.outputPath('18C-stale-review-compact.png'),
    animations: 'disabled',
  });
  await review.getByRole('button', { name: 'Close review', exact: true }).click();
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.evaluate(() => {
    document.documentElement.style.setProperty('--ui-scale', '1');
    document.documentElement.style.zoom = '100%';
  });
  await page.getByRole('button', { name: 'Collapse sidebar', exact: true }).click();
  await workspace.getByRole('button', { name: 'Open earlier bundle', exact: true }).click();
  await expect(review.locator('.pdf-paper').first()).toBeVisible();
  expect(
    Math.abs((await railAlignment()).railRight - (await railAlignment()).dialogLeft),
  ).toBeLessThanOrEqual(1);
  await review.getByRole('button', { name: 'Close review', exact: true }).click();
  await page.locator('.matter-rail').getByRole('button', { name: 'Storage', exact: true }).click();
  await expect(workspace).toContainText(build.id);
  await page.getByRole('button', { name: 'Expand sidebar', exact: true }).click();
  await capture(page, info, '20A-storage');
  await workspace.getByRole('button', { name: 'Review changes', exact: true }).click();
  await expect(workspace).toHaveAttribute('data-screen', 'changes');
  await expect(workspace).toContainText('title changed');
  await workspace.getByRole('button', { name: 'Back to storage', exact: true }).click();
  await expect(workspace).toHaveAttribute('data-screen', 'storage');
  await workspace.getByRole('button', { name: /Imported originals.*files/ }).click();
  await expect(workspace.locator('.stored-document-row')).toHaveCount(2);
  await expect(workspace.locator('.stored-documents')).toContainText('Reference-only');
  await workspace.getByRole('button', { name: 'View original', exact: true }).last().click();
  const originalReview = page.getByRole('dialog', { name: 'Review imported original' });
  await expect(originalReview.locator('.pdf-paper').first()).toBeVisible();
  await expect(originalReview.getByLabel('Document version')).toHaveCount(0);
  await originalReview.getByRole('button', { name: 'Close review', exact: true }).click();
  await workspace.getByRole('button', { name: /Working revisions.*current documents/ }).click();
  await expect(
    workspace.getByRole('heading', { name: 'Working revisions', exact: true }),
  ).toBeVisible();
  await workspace.getByRole('button', { name: /Archived documents.*retained/ }).click();
  await expect(workspace).toContainText('No archived documents');
  await workspace.getByRole('button', { name: /Generated bundles.*saved builds/ }).click();
  await workspace.getByRole('button', { name: 'Review Combined PDF', exact: true }).click();
  await expect(review).toContainText(build.id.slice(0, 8));
  await review.getByRole('button', { name: 'Save a copy', exact: true }).click();
  await exp.getByLabel('Release reviewed output').check();
  await expect(exp.getByRole('button', { name: 'Export files', exact: true })).toBeDisabled();
  await exp.getByLabel('Release reviewed output').uncheck();
  await expect(exp.getByRole('button', { name: 'Export files', exact: true })).toBeEnabled();
  await exp.getByRole('button', { name: 'Cancel', exact: true }).click();
  await review.getByRole('button', { name: 'Close review', exact: true }).click();

  expect(
    hash(
      await (
        await request.get(`${base}/matters/${matter.id}/exports/${build.id}/combined.pdf`, {
          headers,
        })
      ).body(),
    ),
  ).toBe(hash(source));
});

test('B01B settings geometry, keyboard, theme and scale matrix', async ({
  page,
  request,
}, info) => {
  await fixture(request);
  await page.goto('/');
  await enterLatestMatter(page);
  await expect(page.locator('.pdf-paper')).toBeVisible();
  await enter(page);
  for (const size of [
    { width: 1440, height: 900 },
    { width: 1024, height: 768 },
  ])
    for (const theme of ['light', 'dark'])
      for (const scale of [90, 100, 110, 125]) {
        await page.setViewportSize(size);
        await page.evaluate(
          ({ theme, scale }) => {
            document.documentElement.dataset.theme = theme;
            document.documentElement.dataset.effectiveTheme = theme;
            document.documentElement.style.setProperty('--ui-scale', String(scale / 100));
            document.documentElement.style.zoom = scale + '%';
          },
          { theme, scale },
        );
        await expect(page.locator('.build-settings-card')).toBeVisible();
        expect(
          await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
        ).toBeTruthy();
        await page
          .locator('.build-summary')
          .getByRole('button', { name: 'Plan volumes', exact: true })
          .click();
        await expect(page.locator('.volume-workspace')).toBeVisible();
        await page.keyboard.press('Escape');
        await expect(
          page.locator('.build-summary').getByRole('button', { name: 'Plan volumes', exact: true }),
        ).toBeFocused();
        await page.locator('.workspace-page').evaluate((el: HTMLElement) => (el.scrollTop = 0));
        await capture(page, info, `15A-${size.width}-${theme}-${scale}`);
      }
});

test('B01B late build response cannot replace another matter workspace', async ({
  page,
  request,
}) => {
  const second = await fixture(request);
  const name = `Synthetic late-result destination ${second.matter.id.slice(0, 8)}`;
  await request.patch(`${base}/matters/${second.matter.id}`, { headers, data: { name } });
  const first = await fixture(request);
  await page.goto('/');
  await enterLatestMatter(page);
  await expect(page.locator('.pdf-paper')).toBeVisible();
  await enter(page);
  let release!: () => void;
  const held = new Promise<void>((r) => (release = r));
  let fetched = 0;
  await page.route('**/jobs/*', async (route) => {
    if (route.request().method() !== 'GET') {
      await route.continue();
      return;
    }
    const response = await route.fetch();
    fetched++;
    await held;
    await route.fulfill({ response });
  });
  try {
    await page
      .locator('.build-workspace')
      .getByRole('button', { name: 'Build bundle', exact: true })
      .click();
    await expect.poll(() => fetched).toBeGreaterThan(0);
    const builds = await (
      await request.get(`${base}/matters/${first.matter.id}/exports`, { headers })
    ).json();
    expect(builds).toHaveLength(1);
    await openWorkspaceNavigation(page);
    await openNamedMatter(page, name);
    await enter(page);
    await expect(page.locator('.build-workspace')).toHaveAttribute('data-screen', 'settings');
    const lateResponse = page.waitForResponse(
      (response) => response.url().includes('/jobs/') && response.request().method() === 'GET',
    );
    release();
    await lateResponse;
    await page.unroute('**/jobs/*');
    await expect(page.locator('.build-workspace')).toHaveAttribute('data-screen', 'settings');
    await expect(page.locator('.build-workspace')).not.toContainText(builds[0].id);
    await expect(page.locator('.toast')).not.toContainText('Filing bundle generated successfully');
    expect(
      await (await request.get(`${base}/matters/${second.matter.id}/exports`, { headers })).json(),
    ).toEqual([]);
  } finally {
    release();
  }
});

test('saved-input retry keeps the failed settings and current-input build uses repaired settings', async ({
  page,
  request,
}) => {
  const { matter } = await fixture(request);
  await request.patch(`${base}/matters/${matter.id}`, {
    headers,
    data: { max_pages_per_volume: 2 },
  });
  await page.goto('/');
  await enterLatestMatter(page);
  await enter(page);
  const workspace = page.locator('.build-workspace');
  await workspace.getByRole('button', { name: 'Build bundle', exact: true }).click();
  await expect(workspace).toContainText('Build failed');
  const first = (
    await (await request.get(`${base}/matters/${matter.id}/jobs`, { headers })).json()
  )[0];
  expect(first.input_context.source_pages).toBe(3);
  await request.patch(`${base}/matters/${matter.id}`, {
    headers,
    data: { max_pages_per_volume: 300 },
  });
  await page.reload();
  await enterLatestMatter(page);
  await enter(page);
  await workspace.getByRole('button', { name: 'Retry saved inputs', exact: true }).click();
  await expect(workspace).toContainText('Build failed');
  await expect
    .poll(
      async () =>
        (await (await request.get(`${base}/matters/${matter.id}/jobs`, { headers })).json()).length,
    )
    .toBe(2);
  const retried = (
    await (await request.get(`${base}/matters/${matter.id}/jobs`, { headers })).json()
  )[0];
  expect(retried.input_context.snapshot_id).toBe(first.input_context.snapshot_id);
  await expect(workspace).toContainText('2-page volume cap');
  await workspace.getByRole('button', { name: 'Build current inputs', exact: true }).click();
  await expect(workspace.getByRole('heading', { name: 'Bundle ready for review' })).toBeVisible();
  const builds = await (
    await request.get(`${base}/matters/${matter.id}/exports`, { headers })
  ).json();
  expect(builds).toHaveLength(1);
  expect(builds[0].input_snapshot_id).not.toBe(first.input_context.snapshot_id);
  expect(builds[0].pages).toBe(4);
});

test('build recovery opens the exact affected revision and refuses a changed source', async ({
  page,
  request,
}) => {
  const { matter, docs } = await fixture(request);
  await request.patch(`${base}/documents/${docs[0].id}`, { headers, data: { title: 'தமிழ்' } });
  await page.goto('/');
  await enterLatestMatter(page);
  await enter(page);
  const workspace = page.locator('.build-workspace');
  await workspace.getByRole('button', { name: 'Build bundle', exact: true }).click();
  await expect(workspace).toContainText('Build failed');
  await workspace
    .getByRole('button', { name: 'Inspect affected document · Revision 1', exact: true })
    .click();
  await expect(page.locator('.pdf-paper')).toBeVisible();
  await expect(page.locator('.read-workspace')).toContainText('Revision 1');
  await request.post(`${base}/documents/${docs[0].id}/pages/rotate`, {
    headers,
    data: { page_index: 0, degrees: 90, expected_revision: 1 },
  });
  await enter(page);
  await workspace
    .getByRole('button', { name: 'Inspect affected document · Revision 1', exact: true })
    .click();
  await expect(page.locator('.toast')).toContainText('No substitute page was opened');
  await expect(workspace).toBeVisible();
});
