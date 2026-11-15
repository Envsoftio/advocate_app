import { Fragment, Text, type VNode, type VNodeChild } from 'vue';
export type SelectValue = string | number | boolean | null;
export interface SelectChoice {
  key: string;
  value: SelectValue;
  label: string;
  disabled: boolean;
}
function text(children: VNodeChild | VNodeChild[]): string {
  if (Array.isArray(children)) return children.map(text).join('');
  if (typeof children === 'string' || typeof children === 'number') return String(children);
  if (children && typeof children === 'object' && 'children' in children)
    return text(children.children as VNodeChild);
  return '';
}
// Keep existing declarative option lists, including v-for/v-if fragments and typed values.
// These VNodes are configuration only: no native option/select menu is rendered.
export function selectChoices(nodes: VNode[]): SelectChoice[] {
  const result: SelectChoice[] = [];
  function visit(nodes: VNode[]) {
    for (const node of nodes) {
      if (node.type === Fragment) visit(node.children as VNode[]);
      else if (node.type === 'option') {
        const label = text(node.children as VNodeChild)
          .replace(/\s+/g, ' ')
          .trim();
        result.push({
          key: `option-${result.length}`,
          value: node.props && 'value' in node.props ? node.props.value : label,
          label,
          disabled: node.props?.disabled != null && node.props.disabled !== false,
        });
      } else if (node.type !== Text && Array.isArray(node.children))
        visit(node.children as VNode[]);
    }
  }
  visit(nodes);
  return result;
}
export function selectedChoice(choices: SelectChoice[], value: SelectValue | undefined) {
  return choices.find(
    (choice) =>
      choice.value === value ||
      (value != null && choice.value != null && String(choice.value) === String(value)),
  );
}
