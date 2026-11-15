import { describe, expect, it } from 'vitest';
import { createVNode, Fragment, createCommentVNode } from 'vue';
import { selectChoices, selectedChoice } from './selectOptions';
describe('declarative dropdown choices', () => {
  it('retains empty, numeric, boolean and null values through conditional/loop fragments', () => {
    const choices = selectChoices([
      createCommentVNode('v-if'),
      createVNode(Fragment, null, [
        createVNode('option', { value: '' }, 'All types'),
        createVNode('option', { value: 2 }, 'Part 2'),
        createVNode('option', { value: false }, 'Regular'),
        createVNode('option', { value: true }, 'Bold'),
        createVNode('option', { value: null }, 'Any Part'),
        createVNode('option', { disabled: '' }, 'Unavailable'),
        createVNode('option', { disabled: false }, 'Current'),
      ]),
    ]);
    expect(choices.map((item) => item.value)).toEqual([
      '',
      2,
      false,
      true,
      null,
      'Unavailable',
      'Current',
    ]);
    expect(choices.map((item) => item.disabled)).toEqual([
      false,
      false,
      false,
      false,
      false,
      true,
      false,
    ]);
    expect(selectedChoice(choices, '2')?.value).toBe(2);
    expect(selectedChoice(choices, null)?.label).toBe('Any Part');
    expect(selectedChoice(choices, '')?.label).toBe('All types');
    expect(selectedChoice(choices, 'missing')).toBeUndefined();
  });
});
