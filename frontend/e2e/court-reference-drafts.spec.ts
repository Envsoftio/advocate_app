import { test, expect } from '@playwright/test';
import { selectOption } from './select';
import { enterLatestMatter, openWorkspaceNavigation } from './navigation';
const base = `http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT || 8768}`;
const headers = { Authorization: 'Bearer e2e-isolated-token' };
test('official court references are selectable drafts and do not mutate matter settings', async ({
  page,
  request,
}, info) => {
  const matter = await (
    await request.post(`${base}/matters`, {
      headers,
      data: { name: 'Synthetic court research', court_profile: 'supreme-court-source-draft' },
    })
  ).json();
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/');
  await enterLatestMatter(page);
  await openWorkspaceNavigation(page);
  await page
    .locator('.workspace-drawer')
    .getByRole('button', { name: 'Court rules', exact: true })
    .click();
  const guide = page.locator('.filing-guide');
  await expect(
    guide.getByRole('heading', { name: 'Supreme Court of India filing reference' }),
  ).toBeVisible();
  await expect(guide).toContainText('No advocate approval');
  await expect(guide).toContainText('50 MB');
  await expect(page.locator('.memo-helper')).toHaveCount(0);
  const link = guide.getByRole('button', { name: /SC-eFM 3.0 FAQ/ });
  const popup = page.waitForEvent('popup');
  await link.click();
  const opened = await popup;
  await expect.poll(() => opened.url()).toContain('efiling3.sci.gov.in/resources/FAQ');
  await opened.close();
  await selectOption(
    page.getByRole('combobox', { name: 'Reference court' }),
    'punjab-haryana-source-draft',
  );
  await expect(guide).toContainText('25 September 2026');
  await expect(guide).toContainText('could not be retrieved');
  await page.evaluate(() => {
    window.scrollTo(0, 0);
    document.querySelectorAll<HTMLElement>('body *').forEach((element) => {
      if (element.scrollTop) element.scrollTop = 0;
    });
  });
  await page.screenshot({
    path: info.outputPath('punjab-haryana-reference.png'),
    animations: 'disabled',
  });
  await selectOption(
    page.getByRole('combobox', { name: 'Reference court' }),
    'delhi-hc-illustrative',
  );
  await expect(guide).toContainText('300 MB');
  await expect(page.locator('.memo-helper')).toBeVisible();
  await selectOption(
    page.getByRole('combobox', { name: 'Reference court' }),
    'supreme-court-source-draft',
  );
  const saved = await (await request.get(`${base}/matters`, { headers })).json();
  expect(saved.find((m: any) => m.id === matter.id).court_profile).toBe(
    'supreme-court-source-draft',
  );
  await page.evaluate(() => {
    window.scrollTo(0, 0);
    document.querySelectorAll<HTMLElement>('body *').forEach((element) => {
      if (element.scrollTop) element.scrollTop = 0;
    });
  });
  await page.screenshot({
    path: info.outputPath('supreme-court-reference.png'),
    animations: 'disabled',
  });
});
