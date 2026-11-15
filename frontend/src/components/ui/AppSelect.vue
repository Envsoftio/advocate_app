<script setup lang="ts" generic="T">
import { computed, ref, useSlots, watch } from 'vue';
import { Check, ChevronDown } from 'lucide-vue-next';
import {
  SelectRoot,
  SelectTrigger,
  SelectPortal,
  SelectContent,
  SelectViewport,
  SelectItem,
  SelectItemText,
  SelectItemIndicator,
} from 'reka-ui';
import {
  selectChoices,
  selectedChoice,
  type SelectValue,
  type SelectChoice,
} from '../../lib/selectOptions';

defineOptions({ inheritAttrs: false });
const props = defineProps<{
  modelValue?: T;
  value?: T;
  disabled?: boolean;
  modelModifiers?: Record<string, boolean>;
}>();
const emit = defineEmits<{ 'update:modelValue': [value: T]; change: [event: Event] }>();
const slots = useSlots();
const choices = computed(() => selectChoices(slots.default?.() || []));
const selected = computed(() =>
  selectedChoice(
    choices.value,
    (props.modelValue !== undefined ? props.modelValue : props.value) as SelectValue | undefined,
  ),
);
const open = ref(false);
const trigger = ref<HTMLButtonElement>();
const portalTarget = ref<HTMLElement | string>('body');
const inline = ref(false);
function setOpen(next: boolean) {
  if (next) {
    inline.value = !!trigger.value?.closest('dialog');
    portalTarget.value = document.querySelector<HTMLElement>('.design-shell') || document.body;
  }
  open.value = next;
}
watch(
  () => props.disabled,
  (disabled) => {
    if (disabled) open.value = false;
  },
);
let pendingChoice: SelectChoice | undefined;
function choose(key: unknown) {
  const choice = choices.value.find((item) => item.key === key);
  if (!choice || choice.disabled || props.disabled) return;
  pendingChoice = choice;
  if (!open.value) {
    restoreFocus(new Event('closeAutoFocus'));
    return;
  }
  open.value = false;
}
function menuKey(event: KeyboardEvent) {
  event.stopPropagation();
  if (event.key === 'Escape') {
    event.preventDefault();
    open.value = false;
  }
}
function restoreFocus(event: Event) {
  event.preventDefault();
  trigger.value?.focus({ preventScroll: true });
  const choice = pendingChoice;
  pendingChoice = undefined;
  if (!choice || props.disabled) return;
  // Commit after the menu releases its focus scope so workflow guards record
  // the persistent trigger, never an option that is about to be removed.
  emit('update:modelValue', choice.value as T);
  // Existing guarded @change handlers read a real select target's string value.
  // Model updates above retain typed numeric/boolean/null option values.
  const target = document.createElement('select');
  target.add(new Option(choice.label, String(choice.value ?? ''), true, true));
  const changeEvent = new Event('change', { bubbles: true });
  Object.defineProperty(changeEvent, 'target', { value: target });
  Object.defineProperty(changeEvent, 'currentTarget', { value: target });
  emit('change', changeEvent);
}
</script>

<template>
  <SelectRoot
    :model-value="selected?.key ?? 'unselected'"
    :open="open"
    :disabled="disabled"
    @update:model-value="choose"
    @update:open="setOpen"
  >
    <SelectTrigger as-child>
      <button
        ref="trigger"
        v-bind="$attrs"
        type="button"
        class="app-select-trigger"
        :disabled="disabled"
        :data-select-value="String(selected?.value ?? '')"
        :data-select-options="
          JSON.stringify(
            choices.map((item) => ({
              value: String(item.value ?? ''),
              label: item.label,
              disabled: item.disabled,
            })),
          )
        "
      >
        <span class="app-select-label">{{ selected?.label || '\u00a0' }}</span><ChevronDown
          class="app-select-chevron"
          :size="14"
          aria-hidden="true"
        />
      </button>
    </SelectTrigger>
    <SelectPortal :to="portalTarget" :disabled="inline">
      <SelectContent
        class="app-select-content"
        position="popper"
        position-strategy="absolute"
        align="start"
        :side-offset="4"
        :collision-padding="12"
        @close-auto-focus="restoreFocus"
        @escape-key-down.stop
        @keydown="menuKey"
      >
        <SelectViewport class="app-select-viewport">
          <SelectItem
            v-for="choice in choices"
            :key="choice.key"
            :value="choice.key"
            :disabled="choice.disabled"
            :text-value="choice.label"
            :data-select-value="String(choice.value ?? '')"
            class="app-select-item"
          >
            <SelectItemText>{{ choice.label }}</SelectItemText>
            <SelectItemIndicator class="app-select-check"><Check :size="14" /></SelectItemIndicator>
          </SelectItem>
        </SelectViewport>
      </SelectContent>
    </SelectPortal>
  </SelectRoot>
</template>
