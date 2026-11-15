<script setup lang="ts">
import { computed, ref, useAttrs, watch } from 'vue';
import { parseDate, type DateValue } from '@internationalized/date';
import { CalendarDays, ChevronLeft, ChevronRight } from 'lucide-vue-next';
import {
  DateFieldRoot,
  DateFieldInput,
  CalendarRoot,
  CalendarHeader,
  CalendarHeading,
  CalendarPrev,
  CalendarNext,
  CalendarGrid,
  CalendarGridHead,
  CalendarGridBody,
  CalendarGridRow,
  CalendarHeadCell,
  CalendarCell,
  CalendarCellTrigger,
} from 'reka-ui';
import AppPopover from './AppPopover.vue';

defineOptions({ inheritAttrs: false });
const props = withDefaults(defineProps<{ disabled?: boolean; label?: string }>(), {
  disabled: false,
  label: 'Document date',
});
const value = defineModel<string>({ required: true });
const emit = defineEmits<{ input: [event: Event] }>();
const attrs = useAttrs();
const open = ref(false);
const selected = computed(() => {
  try {
    return value.value ? parseDate(value.value) : undefined;
  } catch {
    return undefined;
  }
});
const name = computed(() => String(attrs['aria-label'] || props.label));
watch(
  () => props.disabled,
  (disabled) => {
    if (disabled) open.value = false;
  },
);
function update(next: string, event: Event) {
  if (props.disabled) return;
  value.value = next;
  emit('input', event);
}
function updateField(date: DateValue | undefined) {
  update(date?.toString() ?? '', new Event('input', { bubbles: true }));
}
function select(date: DateValue | undefined) {
  if (!date || props.disabled) return;
  update(date.toString(), new Event('input', { bubbles: true }));
  open.value = false;
}
function clear() {
  update('', new Event('input', { bubbles: true }));
  open.value = false;
}
</script>

<template>
  <span class="app-date-input">
    <DateFieldRoot
      v-bind="attrs"
      v-slot="{ segments }"
      class="app-date-field"
      :aria-label="name"
      :model-value="selected ?? null"
      locale="en-GB"
      granularity="day"
      :disabled="disabled"
      @update:model-value="updateField"
    >
      <DateFieldInput
        v-for="segment in segments"
        :key="segment.part"
        :part="segment.part"
        class="app-date-segment"
        >{{ segment.value }}</DateFieldInput
      >
    </DateFieldRoot>
    <AppPopover v-model:open="open" :label="`${name} calendar`" content-class="app-date-popover">
      <template #trigger>
        <button
          type="button"
          class="app-date-trigger"
          :aria-label="`Choose ${name.toLowerCase()}`"
          :disabled="disabled"
          ><CalendarDays :size="15"
        /></button>
      </template>
      <CalendarRoot
        v-slot="{ grid, weekDays }"
        :model-value="selected"
        locale="en-GB"
        :calendar-label="name"
        prevent-deselect
        fixed-weeks
        :disabled="disabled"
        class="app-calendar"
        @update:model-value="select"
      >
        <CalendarHeader class="app-calendar-header">
          <CalendarPrev class="icon-btn small" aria-label="Previous month"
            ><ChevronLeft :size="16"
          /></CalendarPrev>
          <CalendarHeading />
          <CalendarNext class="icon-btn small" aria-label="Next month"
            ><ChevronRight :size="16"
          /></CalendarNext>
        </CalendarHeader>
        <CalendarGrid v-for="month in grid" :key="month.value.toString()" class="app-calendar-grid">
          <CalendarGridHead
            ><CalendarGridRow>
              <CalendarHeadCell v-for="(day, index) in weekDays" :key="index">{{
                day
              }}</CalendarHeadCell>
            </CalendarGridRow></CalendarGridHead
          >
          <CalendarGridBody>
            <CalendarGridRow v-for="(week, index) in month.rows" :key="index">
              <CalendarCell v-for="date in week" :key="date.toString()" :date="date">
                <CalendarCellTrigger :day="date" :month="month.value" class="app-calendar-day" />
              </CalendarCell>
            </CalendarGridRow>
          </CalendarGridBody>
        </CalendarGrid>
      </CalendarRoot>
      <button
        type="button"
        class="text-tool app-calendar-clear"
        :disabled="disabled || !value"
        @click="clear"
        >Clear date</button
      >
    </AppPopover>
  </span>
</template>

<style>
.app-date-input {
  display: inline-grid;
  grid-template-columns: minmax(0, 1fr);
  position: relative;
  width: 100%;
  min-width: 0;
}
.app-date-field {
  display: flex;
  align-items: center;
  width: 100%;
  min-width: 0;
  height: 36px;
  padding: 0 30px 0 10px;
  border: 1px solid var(--cb-control-border);
  border-radius: var(--cb-radius-input);
  background: var(--cb-surface);
  color: var(--cb-text);
  font:
    var(--cb-type-field-weight) var(--cb-type-field-size) var(--cb-font-ui),
    sans-serif;
}
.app-date-field > input[aria-hidden='true'] {
  display: none;
}
.app-date-field:focus-within {
  outline: 2px solid var(--cb-focus);
  outline-offset: 1px;
}
.app-date-field[data-disabled] {
  opacity: 0.55;
}
.app-date-segment {
  border-radius: 2px;
  outline: none;
  caret-color: transparent;
}
.app-date-segment:focus {
  background: var(--cb-selected);
  color: var(--cb-text);
}
.app-date-segment[data-placeholder] {
  color: var(--cb-subtle);
}
.batch-table .app-date-field {
  height: 30px;
  border-color: transparent;
  background: transparent;
  padding-left: 2px;
  font-size: 13px;
}
.batch-table .app-date-field:focus-within {
  background: var(--cb-surface);
  outline-offset: -2px;
}
.app-date-input > .app-popover-anchor {
  position: absolute;
  right: 4px;
  top: 50%;
  transform: translateY(-50%);
}
.app-date-input .app-date-trigger {
  display: grid;
  place-items: center;
  width: 25px;
  height: 25px;
  padding: 0;
  border: 0;
  border-radius: 4px;
  background: transparent;
  color: var(--cb-muted);
  -webkit-text-fill-color: currentColor;
  cursor: pointer;
}
.app-date-trigger svg {
  color: inherit;
  stroke: currentColor;
  filter: none;
}
.app-date-trigger:disabled {
  opacity: 0.5;
  cursor: default;
}
.app-popover.app-date-popover {
  width: 264px;
  min-width: 0 !important;
  padding: 8px !important;
  color: var(--cb-text);
}
.app-calendar-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 4px;
  font-weight: 600;
}
.app-calendar-grid {
  width: 100%;
  table-layout: fixed;
  border-collapse: collapse;
  font-size: 12px;
}
.app-calendar-grid th {
  padding: 4px 0;
  color: var(--cb-muted);
  font-weight: 500;
  text-align: center;
}
.app-calendar-grid td {
  padding: 1px;
  text-align: center;
}
.app-calendar-day {
  width: 100%;
  height: 28px;
  border: 0;
  border-radius: 4px;
  background: transparent;
  color: var(--cb-text);
  font: inherit;
  cursor: pointer;
}
.app-calendar-day:hover {
  background: var(--cb-surface2);
}
.app-calendar-day[data-outside-view] {
  opacity: 0.45;
}
.app-calendar-day[data-today] {
  box-shadow: inset 0 0 0 1px var(--cb-control-border);
}
.app-date-popover .app-calendar-day[data-selected],
.app-date-popover .app-calendar-day[data-selected]:hover {
  background: var(--cb-accent);
  color: var(--cb-onAccent);
}
.app-calendar-day:focus-visible,
.app-date-trigger:focus-visible {
  outline: 2px solid var(--cb-accent);
  outline-offset: 1px;
}
.app-date-popover > * {
  flex-shrink: 0;
}
.app-date-popover > .app-calendar-clear {
  margin-top: 0;
  min-height: 28px;
  padding: 4px 8px;
  width: 100%;
}
</style>
