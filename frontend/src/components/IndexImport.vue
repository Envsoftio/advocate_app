<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { api } from '../lib/api'
import { friendlyError } from '../lib/format'
import { parseIndexCsv } from '../lib/indexImport'
import type { ManualBookmark, MatterDocument } from '../lib/types'

type Row = {title:string; sourceName:string; documentId:string; page:number; reviewed:boolean}
const props = defineProps<{matterId:string; documents:MatterDocument[]; disabled:boolean}>()
const emit = defineEmits<{modified:[]}>()
const rows = ref<Row[]>([])
const error = ref('')
const message = ref('')
const busy = ref(false)
const duplicates = computed(() => {
  const seen = new Set<string>()
  const found = new Set<number>()
  rows.value.forEach((row,index) => {
    const key = `${row.title.trim().toLowerCase()}|${row.documentId}|${row.page}`
    if (seen.has(key)) found.add(index)
    seen.add(key)
  })
  return found
})
const ready = computed(() => rows.value.length > 0 && duplicates.value.size === 0 && rows.value.every(row => {
  const doc = props.documents.find(item => item.id === row.documentId)
  return row.title.trim().length > 0 && row.title.length <= 200 && !!doc &&
    Number.isInteger(row.page) && row.page >= 1 && row.page <= doc.page_count && row.reviewed
}))
watch(() => props.matterId, () => {rows.value=[]; error.value=''; message.value=''})

async function selectFile(event:Event) {
  const file = (event.target as HTMLInputElement).files?.[0]
  rows.value = []
  error.value = ''
  message.value = ''
  if (!file) return
  if (file.size > 256_000) {error.value = 'Choose a CSV or TSV file under 256 KB.'; return}
  try {
    const parsed = parseIndexCsv(await file.text())
    rows.value = parsed.map(entry => {
      const matches = props.documents.filter(doc =>
        doc.title.trim().toLowerCase() === entry.document.trim().toLowerCase() ||
        doc.original_name.trim().toLowerCase() === entry.document.trim().toLowerCase())
      return {title:entry.title, sourceName:entry.document,
        documentId:matches.length === 1 ? matches[0].id : '',
        page: /^\d+$/.test(entry.page) ? Number(entry.page) : 0, reviewed:false}
    })
  } catch (err) {error.value = friendlyError(err)}
  (event.target as HTMLInputElement).value = ''
}
async function importRows() {
  if (!ready.value || busy.value || props.disabled) return
  busy.value = true
  error.value = ''
  message.value = ''
  try {
    const currentDocs = await api<MatterDocument[]>(`/matters/${props.matterId}/documents`)
    if (rows.value.some(row => {
      const shown = props.documents.find(doc => doc.id === row.documentId)
      const current = currentDocs.find(doc => doc.id === row.documentId)
      return !shown || !current || shown.revision !== current.revision || row.page > current.page_count
    })) throw new Error('A document changed while reviewing the index. Refresh and recheck the rows.')
    const current = await api<ManualBookmark[]>(`/matters/${props.matterId}/bookmarks`)
    if (current.length + rows.value.length > 500) throw new Error('This matter already has too many bookmarks for this import.')
    if (rows.value.some(row => current.some(item =>
      item.title.trim().toLowerCase() === row.title.trim().toLowerCase() &&
      item.document_id === row.documentId && item.page_index === row.page - 1))) {
      throw new Error('A reviewed row already exists as a bookmark. Remove the duplicate row before importing.')
    }
    const entries = [...current.map(({id,title,document_id,page_index,target_revision,parent_id}) =>
      ({id,title,document_id,page_index,target_revision,parent_id})),
      ...rows.value.map(row => ({id:crypto.randomUUID().replace(/-/g,''), title:row.title.trim(),
        document_id:row.documentId, page_index:row.page-1,
        target_revision:currentDocs.find(doc => doc.id === row.documentId)!.revision,
        parent_id:null}))]
    await api(`/matters/${props.matterId}/bookmarks`, 'PUT', {entries})
    message.value = `${rows.value.length} reviewed row(s) added as manual bookmarks.`
    rows.value = []
    emit('modified')
  } catch (err) {error.value = friendlyError(err)}
  finally {busy.value = false}
}
</script>

<template>
  <section class="panel index-import" aria-label="Import reviewed index rows">
    <div class="panel-header"><div><h2>Import index rows</h2><span>CSV or TSV → reviewed manual bookmarks</span></div></div>
    <div class="index-import-body">
      <p class="hint">Use columns <code>title,document,page</code>. Page is the page inside a source document, not its printed bundle page. Each row needs a confirmed document and page. Imported text is never treated as a verified legal index.</p>
      <label class="field">Choose index file<input type="file" accept=".csv,.tsv,text/csv,text/tab-separated-values" :disabled="busy || disabled" @change="selectFile"/></label>
      <div v-if="rows.length" class="index-import-rows"><div v-for="(row,index) in rows" :key="index" class="index-import-row">
        <label class="field">Title<input v-model="row.title" maxlength="200"/></label>
        <label class="field">Source document <small v-if="!row.documentId">CSV: {{row.sourceName || 'blank'}}</small><select v-model="row.documentId"><option value="">Choose document</option><option v-for="doc in documents" :key="doc.id" :value="doc.id">{{doc.title}} · {{doc.page_count}} pages</option></select></label>
        <label class="field">Source page<input v-model.number="row.page" type="number" min="1" :max="documents.find(doc => doc.id === row.documentId)?.page_count || 1"/></label>
        <label class="checkbox-line"><input v-model="row.reviewed" type="checkbox"/> Checked target</label>
        <p v-if="duplicates.has(index)" class="error-msg">Duplicate of an earlier row; resolve before import.</p>
      </div></div>
      <div v-if="rows.length" class="filing-review-actions"><button class="btn primary small" :disabled="!ready || busy || disabled" @click="importRows">Add {{rows.length}} reviewed bookmark(s)</button><button class="btn secondary small" @click="rows=[]">Cancel</button></div>
      <p v-if="error" class="error-msg" role="alert">{{error}}</p><p v-if="message" role="status">{{message}}</p>
    </div>
  </section>
</template>
