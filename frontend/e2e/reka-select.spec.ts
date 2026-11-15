import { test, expect } from '@playwright/test';
import { selectOption, expectValue } from './select';
import { openWorkspaceNavigation } from './navigation';

test('dropdown menus use theme tokens, fit scaled viewports and preserve keyboard choices', async ({
  page,
}, info) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto('/');
  for (const [width, theme, scale] of [
    [1440, 'light', 100],
    [1024, 'dark', 125],
  ] as const) {
    await page.setViewportSize({ width, height: width === 1440 ? 900 : 768 });
    await openWorkspaceNavigation(page);
    await selectOption(page.getByRole('combobox', { name: 'Theme', exact: true }), theme);
    await selectOption(page.getByRole('combobox', { name: 'Interface size' }), String(scale));
    const trigger = page.getByRole('combobox', { name: 'Theme', exact: true });
    await trigger.focus();
    await page.keyboard.press('ArrowDown');
    const menu = page.getByRole('listbox');
    await expect(menu).toBeVisible();
    await expect(
      menu.getByRole('option', { name: theme === 'light' ? 'Light' : 'Dark', exact: true }),
    ).toHaveAttribute('aria-selected', 'true');
    const box = (await menu.boundingBox())!;
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(width);
    expect(box.y + box.height).toBeLessThanOrEqual(width === 1440 ? 900 : 768);
    const colors = await menu.evaluate((el) => {
      const probe = document.createElement('span');
      probe.style.background = 'var(--cb-surface)';
      el.append(probe);
      const result = [
        getComputedStyle(el).backgroundColor,
        getComputedStyle(probe).backgroundColor,
      ];
      probe.remove();
      return result;
    });
    expect(colors[0]).toBe(colors[1]);
    await page.screenshot({
      path: info.outputPath(`dropdown-${width}-${theme}-${scale}.png`),
      animations: 'disabled',
    });
    await page.keyboard.press('Escape');
    await expect(menu).toBeHidden();
    await expect(trigger).toBeFocused();
    await expectValue(trigger, theme);
    await trigger.click();
    await expect(
      menu.getByRole('option', { name: theme === 'light' ? 'Light' : 'Dark', exact: true }),
    ).toBeFocused();
    await page.keyboard.press('Home');
    await expect(menu.getByRole('option', { name: 'System', exact: true })).toBeFocused();
    await page.keyboard.press('ArrowDown');
    await expect(menu.getByRole('option', { name: 'Light', exact: true })).toBeFocused();
    await page.keyboard.press('Enter');
    await expectValue(trigger, 'light');
    await trigger.click();
    await expect(menu.getByRole('option', { name: 'Light', exact: true })).toBeFocused();
    await page.keyboard.press('d');
    await expect(menu.getByRole('option', { name: 'Dark', exact: true })).toBeFocused();
    await page.keyboard.press('Enter');
    await expectValue(trigger, 'dark');
    await expect(page.locator('select:visible')).toHaveCount(0);
  }
  expect(errors).toEqual([]);
});
