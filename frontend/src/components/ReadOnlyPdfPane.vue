<script setup lang="ts">
import { ref, watch } from 'vue';
import type { DocumentInitParameters } from 'pdfjs-dist/types/src/display/api';
import { usePdfDocument } from '../lib/pdfReader';
import PdfReaderViewport from './PdfReaderViewport.vue';
import AppSelect from './ui/AppSelect.vue';
const props = defineProps<{
  identity: string;
  source: () => Promise<DocumentInitParameters>;
  page: number;
  title: string;
}>();
const emit = defineEmits<{ scrollRatio: [number] }>();
const viewport = ref<InstanceType<typeof PdfReaderViewport> | null>(null);
function reportScroll() {
  const el = viewport.value?.getScroller();
  if (el) emit('scrollRatio', el.scrollTop / Math.max(1, el.scrollHeight - el.clientHeight));
}
function setScrollRatio(ratio: number) {
  const el = viewport.value?.getScroller();
  if (!el) return;
  const target = ratio * Math.max(0, el.scrollHeight - el.clientHeight);
  if (Math.abs(el.scrollTop - target) > 1) el.scrollTop = target;
}
defineExpose({ setScrollRatio });
const owner = usePdfDocument();
const { pdf, loading, error } = owner;
const zoom = ref('fit-page');
const renderError = ref('');
async function load() {
  renderError.value = '';
  await owner.open(props.source);
}
watch(() => props.identity, load, { immediate: true });
</script>
<template>
  <section class="retained-pdf-pane" :aria-label="title">
    <header
      ><strong>{{ title }}</strong
      ><AppSelect v-model="zoom" :aria-label="`${title} zoom`"
        ><option value="fit-page">Fit page</option><option value="fit-width">Fit width</option
        ><option value="75">75%</option><option value="100">100%</option
        ><option value="125">125%</option></AppSelect
      ></header
    >
    <p v-if="loading" role="status">Opening retained PDF…</p>
    <p v-if="error || renderError" role="alert"
      >{{ error || renderError }} <button class="text-tool" @click="load">Retry PDF</button></p
    >
    <PdfReaderViewport
      v-if="pdf"
      ref="viewport"
      @position="reportScroll"
      :pdf="pdf"
      :page="page"
      :zoom="zoom"
      mode="single"
      :interactive="false"
      @error="renderError = $event"
    />
  </section>
</template>
