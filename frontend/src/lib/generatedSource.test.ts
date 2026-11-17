import { expect, it } from 'vitest';
import { generatedSourcePage } from './generatedSource';

const row = {
  id: 'source',
  title: 'Synthetic source',
  volume: 2,
  start_page: 3,
  end_page: 5,
  combined_start_page: 14,
  combined_end_page: 16,
  source_start: 8,
};

it('maps split and selected supplemental source runs in combined and standalone output', () => {
  expect(generatedSourcePage(row, 'combined.pdf', 14)).toBe(9);
  expect(generatedSourcePage(row, 'combined.pdf', 16)).toBe(11);
  expect(generatedSourcePage(row, 'volume-02.pdf', 4)).toBe(10);
  expect(generatedSourcePage({ ...row, source_start: 22 }, 'combined.pdf', 14)).toBe(23);
});

it('does not invent destinations for front matter, other artifacts or invalid pages', () => {
  expect(generatedSourcePage(row, 'combined.pdf', 13)).toBeNull();
  expect(generatedSourcePage(row, 'combined.pdf', 17)).toBeNull();
  expect(generatedSourcePage(row, 'volume-01.pdf', 3)).toBeNull();
  expect(generatedSourcePage(row, 'part-02.pdf', 3)).toBeNull();
  expect(generatedSourcePage(undefined, 'combined.pdf', 1)).toBeNull();
  expect(generatedSourcePage(row, 'combined.pdf', 14.5)).toBeNull();
  expect(generatedSourcePage({ ...row, source_start: undefined }, 'combined.pdf', 14)).toBe(1);
});
