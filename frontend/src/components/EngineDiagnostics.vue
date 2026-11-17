<script setup lang="ts">
import { computed, ref, watch, nextTick } from 'vue';
import {
  DialogRoot,
  DialogTrigger,
  DialogPortal,
  DialogOverlay,
  DialogContent,
  DialogTitle,
  DialogDescription,
  DialogClose,
} from 'reka-ui';
import { Unplug, X } from 'lucide-vue-next';
import { engineState, engineReason, lastEngineResponse } from '../lib/engineConnection';
import { inTauri } from '../lib/api';
const emit = defineEmits<{ retry: []; review: [] }>();
const anchor = ref<HTMLElement | null>(null);
const inline = ref(false);
function updateOpen(value: boolean) {
  inline.value = !!anchor.value?.closest('dialog');
  open.value = value;
}
async function reviewLatest() {
  open.value = false;
  await nextTick();
  emit('review');
}
const open = ref(false);
const copied = ref('');
const lastResponse = computed(() =>
  lastEngineResponse.value
    ? new Date(lastEngineResponse.value).toLocaleString()
    : 'No successful response recorded in this window',
);
const report = computed(() =>
  [
    'MatterDraft Studio — processing diagnostics',
    `Engine state: ${engineState.value}`,
    `Last successful response: ${lastResponse.value}`,
    `Runtime: ${inTauri() ? 'Desktop' : 'Browser'}`,
    engineReason.value,
    'Saves, OCR, page transformations, Preflight and builds are paused while disconnected.',
    'Retry verifies saved inputs; operations are never automatically replayed.',
  ].join('\n'),
);
async function copy() {
  try {
    await navigator.clipboard.writeText(report.value);
    copied.value = 'Diagnostics copied';
  } catch {
    copied.value = 'Clipboard unavailable. Select and copy the diagnostic text below.';
  }
}
watch(open, () => (copied.value = ''));
watch(engineState, (state) => {
  if (state === 'ready') open.value = false;
});
</script>
<template>
  <span ref="anchor" class="engine-diagnostics-anchor"
    ><DialogRoot :open="open" @update:open="updateOpen">
      <DialogTrigger as-child
        ><button class="btn secondary small">View diagnostics</button></DialogTrigger
      >
      <DialogPortal :disabled="inline">
        <DialogOverlay class="engine-diagnostics-scrim" />
        <DialogContent class="engine-diagnostics-drawer" @escape-key-down.stop>
          <header
            ><Unplug :size="20" /><DialogTitle as="h2">Processing diagnostics</DialogTitle
            ><DialogClose as-child
              ><button class="icon-btn" aria-label="Close diagnostics"
                ><X :size="18" /></button></DialogClose
          ></header>
          <DialogDescription class="engine-diagnostics-response"
            >Last response: {{ lastResponse }}</DialogDescription
          >
          <div class="engine-diagnostics-body">
            <section v-if="$slots.default"><h3>Current reader</h3><slot /></section>
            <section
              ><h3>Available</h3
              ><p
                >Browse metadata already loaded in this window. Read cached PDF pages and search
                available text.</p
              ><p
                >Other pages and saved bundle details may need the engine; the whole PDF and search
                index are not cached.</p
              ></section
            >
            <section
              ><h3>Temporarily unavailable</h3
              ><ul
                ><li>Saving changes and creating new previews</li
                ><li>OCR and page transformations</li
                ><li>Preflight processing and bundle building</li></ul
              ></section
            >
            <p class="engine-diagnostics-retained"
              >Saved preparation work is retained. Unsaved drafts remain in this window. Reconnect,
              review the saved inputs, then explicitly save retained drafts.</p
            >
            <p>{{
              inTauri()
                ? 'If retry fails, quit and reopen MatterDraft to restart the local engine. Unsaved drafts cannot survive closing this window.'
                : 'If retry fails, restart backend/dev.sh, then retry the connection.'
            }}</p>
            <details
              ><summary>Diagnostic text</summary><pre>{{ report }}</pre>
            </details>
            <p role="status">{{ copied }}</p>
          </div>
          <footer
            ><button v-if="engineState === 'changed'" class="btn primary" @click="reviewLatest"
              >Review latest state</button
            ><button
              v-else
              class="btn primary"
              :disabled="engineState === 'checking'"
              @click="$emit('retry')"
              >{{ engineState === 'checking' ? 'Checking…' : 'Retry connection' }}</button
            ><button class="btn secondary" @click="copy">Copy diagnostics</button
            ><DialogClose as-child
              ><button class="btn secondary">Continue browsing</button></DialogClose
            ></footer
          >
        </DialogContent>
      </DialogPortal>
    </DialogRoot></span
  >
</template>
