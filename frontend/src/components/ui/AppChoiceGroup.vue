<script setup lang="ts" generic="T extends string">
import { ToggleGroupRoot } from 'reka-ui'

// Filters and reader modes must always keep one choice selected.
// as-child preserves the caller's layout element and all existing CSS selectors.
const value = defineModel<T>({ required: true })
function update(next: unknown) {
  if (typeof next === 'string' && next) value.value = next as T
}
</script>

<template>
  <ToggleGroupRoot as-child type="single" orientation="horizontal" :model-value="value" @update:model-value="update">
    <slot />
  </ToggleGroupRoot>
</template>
