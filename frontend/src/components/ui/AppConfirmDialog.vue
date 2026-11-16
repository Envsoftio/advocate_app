<script setup lang="ts">
import {
  AlertDialogRoot,
  AlertDialogTrigger,
  AlertDialogPortal,
  AlertDialogOverlay,
  AlertDialogContent,
  AlertDialogTitle,
  AlertDialogDescription,
  AlertDialogCancel,
  AlertDialogAction,
} from 'reka-ui';
defineProps<{ title: string; description: string; confirmLabel: string; cancelLabel: string }>();
const open = defineModel<boolean>('open', { default: false });
const emit = defineEmits<{ confirm: [] }>();
</script>
<template>
  <AlertDialogRoot v-model:open="open">
    <AlertDialogTrigger as-child><slot /></AlertDialogTrigger>
    <AlertDialogPortal>
      <AlertDialogOverlay class="build-confirm-overlay" />
      <AlertDialogContent class="build-confirm-dialog build-card" @escape-key-down.stop>
        <AlertDialogTitle as="h2">{{ title }}</AlertDialogTitle>
        <AlertDialogDescription as="p">{{ description }}</AlertDialogDescription>
        <footer class="build-actions">
          <AlertDialogCancel as-child
            ><button class="membership-button secondary">{{
              cancelLabel
            }}</button></AlertDialogCancel
          >
          <AlertDialogAction as-child
            ><button class="membership-button primary" @click="emit('confirm')">{{
              confirmLabel
            }}</button></AlertDialogAction
          >
        </footer>
      </AlertDialogContent>
    </AlertDialogPortal>
  </AlertDialogRoot>
</template>
