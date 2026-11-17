<script setup lang="ts">
import { ref, nextTick, onMounted, onBeforeUnmount } from 'vue';
import { api, pdfDocumentSource, exportPdfDocumentSource } from '../lib/api';
import { friendlyError } from '../lib/format';
import ReadOnlyPdfPane from './ReadOnlyPdfPane.vue';
const props = defineProps<{
  matterId: string;
  buildId: string;
  documentId: string;
  revision: number;
  title: string;
  initialPriorPage: number;
}>();
const syncScroll = ref(false);
const ignoreNumbering = ref(false);
const priorPane = ref<InstanceType<typeof ReadOnlyPdfPane> | null>(null);
const currentPane = ref<InstanceType<typeof ReadOnlyPdfPane> | null>(null);
const emit = defineEmits<{ close: [] }>();
const dialog = ref<HTMLDialogElement | null>(null);
const priorPage = ref(props.initialPriorPage),
  currentPage = ref(1);
const pending = ref(false),
  error = ref('');
type Result = {
  prior_physical_page: number;
  prior_source_page: number;
  current_source_page: number;
  current_page_count: number;
  prior_source_ranges: Array<{ first: number; last: number }>;
  differences: Array<{ kind: string; before: string; after: string }>;
  rendered_equal: boolean;
  has_text: boolean;
  truncated: boolean;
  note: string;
};
const result = ref<Result | null>(null);
let sequence = 0;
let returnFocus: HTMLElement | null = null;
async function compare() {
  const call = ++sequence;
  pending.value = true;
  error.value = '';
  result.value = null;
  try {
    const data = await api<Result>(
      `/matters/${props.matterId}/exports/${props.buildId}/compare-pages`,
      'POST',
      {
        ignore_generated_numbering: ignoreNumbering.value,
        document_id: props.documentId,
        expected_revision: props.revision,
        prior_source_page: priorPage.value,
        current_source_page: currentPage.value,
      },
    );
    if (call === sequence) result.value = data;
  } catch (err) {
    if (call === sequence) error.value = friendlyError(err);
  } finally {
    if (call === sequence) pending.value = false;
  }
}
onMounted(() => {
  returnFocus = document.activeElement as HTMLElement;
  dialog.value?.showModal();
  void compare();
});
onBeforeUnmount(() => {
  sequence++;
  dialog.value?.close();
  void nextTick(() => returnFocus?.isConnected && returnFocus.focus());
});
</script>
<template>
  <dialog
    ref="dialog"
    class="pdf-comparison-dialog"
    aria-label="Compare PDF pages"
    @cancel.prevent="emit('close')"
  >
    <header
      ><div
        ><h2>Compare PDF pages</h2
        ><p>{{ title }} · Build {{ buildId.slice(0, 8) }} → working revision {{ revision }}</p></div
      ><button class="membership-button secondary" @click="emit('close')"
        >Close comparison</button
      ></header
    >
    <form class="comparison-page-controls" @submit.prevent="compare"
      ><label
        >Source page at build<input
          v-model.number="priorPage"
          type="number"
          min="1"
          required /></label
      ><label
        >Current source page<input
          v-model.number="currentPage"
          type="number"
          min="1"
          required /></label
      ><button class="membership-button primary" :disabled="pending">{{
        pending ? 'Comparing…' : 'Compare selected pages'
      }}</button
      ><label class="comparison-option"
        ><input v-model="syncScroll" type="checkbox" />Sync scrolling</label
      ><label class="comparison-option"
        ><input v-model="ignoreNumbering" type="checkbox" @change="compare" />Ignore generated
        numbering in text differences</label
      ><span>Choose each source page independently.</span></form
    >
    <p v-if="error" role="alert"
      >{{ error }} <button class="text-tool" @click="compare">Retry comparison</button></p
    >
    <div v-if="result" class="pdf-comparison-grid">
      <ReadOnlyPdfPane
        ref="priorPane"
        @scroll-ratio="syncScroll && currentPane?.setScrollRatio($event)"
        :identity="`${buildId}:combined`"
        :source="() => exportPdfDocumentSource(matterId, buildId, 'combined.pdf')"
        :page="result.prior_physical_page"
        :title="`Previous build · source ${result.prior_source_page} · combined ${result.prior_physical_page}`"
      />
      <ReadOnlyPdfPane
        ref="currentPane"
        @scroll-ratio="syncScroll && priorPane?.setScrollRatio($event)"
        :identity="`${documentId}:${revision}`"
        :source="() => pdfDocumentSource(documentId, revision)"
        :page="result.current_source_page"
        :title="`Working revision ${revision} · source ${result.current_source_page}`"
      />
      <aside class="comparison-differences"
        ><h3>Differences</h3
        ><p>{{
          result.rendered_equal
            ? 'Rendered pages match at the checked resolution.'
            : 'Rendered pages differ.'
        }}</p
        ><p
          >{{ result.differences.length }} text changes{{
            result.truncated ? ' · limited results' : ''
          }}</p
        ><p v-if="!result.has_text"
          >No searchable text on either page. Compare the rendered scans.</p
        ><section v-for="(difference, index) in result.differences" :key="index"
          ><h4>{{ index + 1 }} · {{ difference.kind }}</h4
          ><small>PREVIOUS</small><pre>{{ difference.before || 'No text' }}</pre
          ><small>CURRENT</small><pre>{{ difference.after || 'No text' }}</pre></section
        ><p>{{ result.note }}</p
        ><p
          >Source pages retained in build:
          {{ result.prior_source_ranges.map((r) => `${r.first}–${r.last}`).join(', ') }}</p
        ></aside
      >
    </div>
    <footer>Comparison review · Both inputs are preserved · Read only</footer>
  </dialog>
</template>
