<script setup lang="ts">
import { ref } from 'vue';
import { PopoverRoot, PopoverTrigger, PopoverPortal, PopoverContent } from 'reka-ui';
withDefaults(
  defineProps<{
    label: string;
    align?: 'start' | 'center' | 'end';
    closeOnSelect?: boolean;
    contentClass?: string;
  }>(),
  { align: 'end', closeOnSelect: false, contentClass: '' },
);
const open = defineModel<boolean>('open', { default: false });
const anchor = ref<HTMLElement | null>(null);
const portalTarget = ref<HTMLElement | string>('body');
const inline = ref(false);
function updateOpen(value: boolean) {
  if (value) {
    inline.value = !!anchor.value?.closest('dialog');
    // The portal must outlive any workspace that an action can remove.
    portalTarget.value = document.querySelector<HTMLElement>('.design-shell') || document.body;
  }
  open.value = value;
}
function close() {
  open.value = false;
}
function select(event: MouseEvent, enabled: boolean) {
  const button = (event.target as HTMLElement).closest('button');
  if (enabled && button && !button.disabled && button.getAttribute('role') !== 'combobox') close();
}
defineExpose({
  close,
  trigger: () => anchor.value?.querySelector<HTMLButtonElement>('button') ?? null,
});
</script>
<template>
  <span ref="anchor" class="app-popover-anchor">
    <PopoverRoot :open="open" @update:open="updateOpen" :modal="false">
      <PopoverTrigger as-child><slot name="trigger" :open="open" /></PopoverTrigger>
      <PopoverPortal :to="portalTarget" :disabled="inline">
        <PopoverContent
          position-strategy="absolute"
          :class="['app-popover', contentClass]"
          :aria-label="label"
          :align="align"
          :side-offset="6"
          :collision-padding="12"
          @click="select($event, closeOnSelect)"
          @escape-key-down.stop
        >
          <slot :close="close" />
        </PopoverContent>
      </PopoverPortal>
    </PopoverRoot>
  </span>
</template>
