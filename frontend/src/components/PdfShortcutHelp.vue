<script setup lang="ts">
import { ref } from 'vue'
defineProps<{editingTools?:boolean}>()
const dialog=ref<HTMLDialogElement|null>(null)
const opener=ref<HTMLButtonElement|null>(null)
function close(){dialog.value?.close();opener.value?.focus()}
</script>
<template>
  <button ref="opener" class="text-tool" type="button" aria-label="PDF keyboard shortcuts" @click="dialog?.showModal()">Shortcuts</button>
  <dialog ref="dialog" class="pdf-shortcut-dialog" aria-label="PDF keyboard shortcuts" @cancel.prevent="close" @keydown.stop @click="($event.target===dialog)&&close()">
    <header><h2>PDF keyboard shortcuts</h2><button class="btn secondary small" autofocus @click="close">Close shortcuts</button></header>
    <dl>
      <dt>Ctrl / ⌘ F</dt><dd>Find text in this PDF</dd>
      <dt>Enter / Shift Enter</dt><dd>Next / previous match while searching</dd>
      <dt>Ctrl / ⌘ + or −</dt><dd>Zoom in / out</dd>
      <dt>Ctrl / ⌘ 0</dt><dd>Fit width</dd>
      <dt>Page Down / Page Up</dt><dd>Next / previous page</dd>
      <dt>Home / End</dt><dd>First / last page</dd>
      <template v-if="editingTools">
        <dt>V / E / T / H</dt><dd>Select / Edit text / Add text / Highlight</dd>
        <dt>Arrow keys</dt><dd>Move between tool tabs, page thumbnails or search results when focused</dd>
        <dt>Alt + ↑ / ↓</dt><dd>Move a focused thumbnail in page order</dd>
        <dt>Delete / Backspace</dt><dd>Remove a selected review mark, image overlay or reviewed link</dd>
        <dt>Drag</dt><dd>Move a selected mark or link; drag its corner handle to resize</dd>
        <dt>Escape</dt><dd>Exit focused reading, close a reader panel, or cancel the current draft</dd>
      </template>
      <dt>Tab / Shift Tab</dt><dd>Next / previous control</dd>
    </dl>
    <p>Page and tool shortcuts apply when you are not typing in a field. Escape closes this dialog.</p>
  </dialog>
</template>
<style scoped>
.pdf-shortcut-dialog{position:fixed;inset:0;margin:auto;width:min(550px,92vw);max-height:85vh;overflow:auto;border:1px solid var(--border);border-radius:12px;padding:20px;color:var(--text);background:var(--panel);box-shadow:0 20px 60px #0004}
.pdf-shortcut-dialog::backdrop{background:#10203899}.pdf-shortcut-dialog:not([open]){display:none}
header{display:flex;align-items:center;justify-content:space-between;gap:16px}h2{font-size:18px;margin:0}
dl{display:grid;grid-template-columns:minmax(125px,1fr) 2fr;gap:12px;margin:22px 0;font-size:13px;line-height:1.5}dt{font-weight:600}dd{margin:0}p{font-size:12px;color:var(--muted);line-height:1.5}
</style>
