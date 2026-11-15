import { expect, type Locator } from '@playwright/test';
type Choice = string | { value?: string; label?: string; index?: number };
/** Exercise the visible Reka menu; never dispatch a synthetic native selection. */
export async function selectOption(trigger: Locator, choice: Choice) {
  await expect(trigger).toBeEnabled();
  const options: Array<{ value: string; label: string; disabled: boolean }> = JSON.parse(
    (await trigger.getAttribute('data-select-options')) || '[]',
  );
  const index =
    typeof choice === 'string'
      ? options.findIndex((o) => o.value === choice)
      : (choice.index ??
        options.findIndex((o) =>
          choice.value !== undefined ? o.value === choice.value : o.label === choice.label,
        ));
  expect(index, `Option ${JSON.stringify(choice)} exists`).toBeGreaterThanOrEqual(0);
  expect(options[index].disabled, 'Option is enabled').toBe(false);
  const menu = trigger.page().getByRole('listbox');
  await trigger.click();
  await expect(menu).toBeVisible();
  await menu.getByRole('option').nth(index).click();
  await expect(menu).toBeHidden();
  return [options[index].value];
}
export async function expectValue(
  locator: Locator,
  value: string | RegExp,
  options?: { timeout?: number },
  negate = false,
) {
  await expect(locator).toBeAttached();
  const assertion = negate ? expect(locator).not : expect(locator);
  if ((await locator.getAttribute('data-select-value')) !== null)
    await assertion.toHaveAttribute('data-select-value', value, options);
  else await assertion.toHaveValue(value, options);
}
export async function inputValue(locator: Locator) {
  return (await locator.getAttribute('data-select-value')) ?? (await locator.inputValue());
}
