<script setup lang="ts">
import { computed, ref, onMounted, onBeforeUnmount, nextTick } from 'vue';
import { api, retainedPdfSource } from '../lib/api';
import { friendlyError, prettyDate, bytesLabel } from '../lib/format';
import type { MatterDocument } from '../lib/types';
import AppSelect from './ui/AppSelect.vue';
import AppConfirmDialog from './ui/AppConfirmDialog.vue';
import ReadOnlyPdfPane from './ReadOnlyPdfPane.vue';
const props = defineProps<{ document: MatterDocument }>();
const emit = defineEmits<{ close: []; restored: [] }>();
type Revision = {
  revision: number;
  created_at: string;
  page_count: number;
  available: boolean;
  sha256: string | null;
  bytes: number | null;
  current: boolean;
};
const revisions = ref<Revision[]>([]),
  selected = ref(''),
  page = ref(1),
  error = ref(''),
  pending = ref(false),
  ack = ref(false);
const item = computed(() => revisions.value.find((r) => String(r.revision) === selected.value));
const dialog = ref<HTMLDialogElement | null>(null);
let returnFocus: HTMLElement | null = null,
  disposed = false;
async function load() {
  pending.value = true;
  error.value = '';
  try {
    const rows = await api<Revision[]>(`/documents/${props.document.id}/revisions`);
    if (!disposed) {
      revisions.value = rows;
      selected.value = String(rows[0]?.revision || '');
      page.value = 1;
    }
  } catch (err) {
    if (!disposed) error.value = friendlyError(err);
  } finally {
    if (!disposed) pending.value = false;
  }
}
async function restore() {
  const revision = item.value;
  if (!revision?.sha256 || pending.value || !ack.value) return;
  pending.value = true;
  error.value = '';
  try {
    await api(`/documents/${props.document.id}/revisions/${revision.revision}/restore`, 'POST', {
      expected_revision: props.document.revision,
      expected_sha256: revision.sha256,
      acknowledge_signature_risk: true,
    });
    emit('restored');
    emit('close');
  } catch (err) {
    error.value = friendlyError(err);
  } finally {
    pending.value = false;
  }
}
function close() {
  if (!pending.value) emit('close');
}
onMounted(() => {
  returnFocus = document.activeElement as HTMLElement;
  dialog.value?.showModal();
  void load();
});
onBeforeUnmount(() => {
  disposed = true;
  dialog.value?.close();
  void nextTick(() => returnFocus?.isConnected && returnFocus.focus());
});
</script>
<template>
  <dialog
    ref="dialog"
    class="revision-history-dialog"
    aria-label="Retained working revisions"
    @cancel.prevent="close"
  >
    <header
      ><div
        ><h2>Retained working revisions</h2
        ><p>{{ document.title }} · Current revision {{ document.revision }}</p></div
      ><button class="membership-button secondary" :disabled="pending" @click="close"
        >Close history</button
      ></header
    >
    <p v-if="error" role="alert"
      >{{ error }}
      <button class="text-tool" :disabled="pending" @click="load">Reload history</button></p
    >
    <p v-if="pending" role="status">Loading or restoring revision…</p>
    <div class="revision-history-body"
      ><aside
        ><label
          >Saved revision<AppSelect
            v-model="selected"
            :disabled="pending"
            @change="
              page = 1;
              ack = false;
            "
            ><option v-for="r in revisions" :key="r.revision" :value="String(r.revision)"
              >Revision {{ r.revision }} · {{ prettyDate(r.created_at)
              }}{{ r.current ? ' · current' : '' }}{{ r.available ? '' : ' · unavailable' }}</option
            ></AppSelect
          ></label
        >
        <template v-if="item"
          ><p>{{ item.page_count }} pages · {{ bytesLabel(item.bytes || 0) }}</p
          ><label
            >Preview source page<input
              v-model.number="page"
              type="number"
              min="1"
              :max="item.page_count" /></label
          ><p
            >This is retained PDF content, not a historical snapshot of document metadata or
            editable overlays.</p
          ><p
            >Restoring creates a new working revision. Original and earlier build bytes remain
            unchanged. Links, marks and bookmarks need review afterward.</p
          ><label
            ><input v-model="ack" type="checkbox" :disabled="pending || item.current" /> I reviewed
            this revision and understand that restoring can reintroduce previously removed content
            and affect signature validity.</label
          >
          <AppConfirmDialog
            :portal-to="dialog || undefined"
            title="Restore retained revision?"
            description="Create a new working copy from this PDF? Current edits stay in history. Recheck redactions, signatures, page links and generated output before sharing."
            confirm-label="Restore as new revision"
            cancel-label="Keep current revision"
            @confirm="restore"
            ><button
              class="membership-button primary"
              :disabled="pending || !ack || item.current || !item.available"
              >Restore revision</button
            ></AppConfirmDialog
          ></template
        > </aside
      ><ReadOnlyPdfPane
        v-if="item?.available && item.sha256"
        :identity="`${document.id}:${item.revision}`"
        :source="() => retainedPdfSource(document.id, item!.revision, item!.sha256!)"
        :page="Math.max(1, Math.min(item.page_count, page || 1))"
        :title="`Retained revision ${item.revision} · read only`"
      /><p v-else>Retained PDF unavailable. Reload history or select another revision.</p></div
    >
  </dialog>
</template>
