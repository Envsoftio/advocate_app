import { expectValue } from './select';
import { expect, type Locator } from '@playwright/test';

export async function fillDate(field: Locator, iso: string) {
  const [year, month, day] = iso.split('-');
  for (const [part, value] of [
    ['day', day],
    ['month', month],
    ['year', year],
  ]) {
    const segment = field.locator(`[data-reka-date-field-segment="${part}"]`);
    await segment.focus();
    await segment.pressSequentially(value);
  }
}
export async function expectDate(field: Locator, iso: string) {
  // Reka's non-interactive hidden form value preserves the API's ISO representation.
  await expectValue(field.locator('input[aria-hidden="true"]'), iso);
}
