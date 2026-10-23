<script setup lang="ts">
import { ref, watch } from 'vue'
import { api } from '../lib/api'
import { friendlyError } from '../lib/format'

interface Row {
  id: string
  part: number
  label: string
  title: string
  start_page: number
  end_page: number
  combined_start_page: number
  combined_end_page: number
  separator_page: number | null
  annexure_mode: 'none' | 'label_stamp' | 'separator'
  bates_first: string | null
  bates_last: string | null
  separator_bates: string | null
}
interface Volume {
  number: number
  index_pages: number
  page_count: number
  combined_start_page: number
  combined_end_page: number
  rows: Row[]
  bates_labels: Array<string | null>
}
interface Preview {
  volumes: Volume[]
  combined_pages: number
  part_extracts: Array<{part:number;documents:number;pages:number}>
  manual_bookmarks: Array<{id:string,title:string,document_title:string,document_page:number,resolved:boolean,volume:number|null,volume_page:number|null,combined_page:number|null}>
  numbering: string
  bates_policy: {enabled:boolean;include_index:boolean;include_separator:boolean;restart_per_volume:boolean}
}
const props = defineProps<{
  matterId: string
  maxPages: number
  keepGroups: boolean
  refreshKey: number
}>()
const emit = defineEmits<{ openDocument: [id: string], bookmarkDocument: [id: string] }>()
const preview = ref<Preview | null>(null)
const loading = ref(false)
const error = ref('')
let sequence = 0

async function refresh() {
  const call = ++sequence
  if (!Number.isInteger(props.maxPages) || props.maxPages < 2 || props.maxPages > 2000) {
    preview.value = null
    error.value = 'Enter a volume page cap from 2 to 2000.'
    return
  }
  loading.value = true
  error.value = ''
  try {
    const result = await api<Preview>(`/matters/${props.matterId}/bundle-preview`, 'POST', {
      max_pages_per_volume: props.maxPages,
      keep_groups_intact: props.keepGroups,
    })
    if (call === sequence) preview.value = result
  } catch (err) {
    if (call === sequence) { preview.value = null; error.value = friendlyError(err) }
  } finally { if (call === sequence) loading.value = false }
}
watch(() => [props.matterId, props.maxPages, props.keepGroups, props.refreshKey],
  () => { void refresh() }, {immediate:true})
</script>

<template>
  <section class="panel bundle-preview" aria-label="Index and page number preview">
    <div class="panel-header"><div><h2>Review index & page numbers</h2><span>Current plan before building</span></div>
      <button class="btn secondary small" :disabled="loading" @click="refresh">Refresh</button></div>
    <div class="bundle-preview-body">
      <p class="hint">Volume page numbers are printed on the PDFs and restart at 1. Combined page numbers are physical positions in combined.pdf. Index pages count in both.</p>
      <p v-if="loading" role="status">Calculating page map…</p>
      <p v-if="error" role="alert" class="error-msg">{{error}}</p>
      <div v-if="preview && !loading">
        <p v-if="!preview.volumes.length" class="hint">Import documents to see index rows and page numbers.</p>
        <p v-else class="bundle-preview-summary">{{preview.volumes.length}} volume(s) · {{preview.volumes.reduce((sum, volume) => sum + volume.index_pages, 0)}} index page(s) · {{preview.combined_pages}} combined pages</p>
        <p v-if="preview.part_extracts.length" class="hint">Technical Part PDFs: {{preview.part_extracts.map(item => `Part ${item.part} (${item.documents} documents, ${item.pages} source pages)`).join(' · ')}}. They omit volume index and separator pages.</p>
        <p v-if="preview.bates_policy.enabled" class="hint">Bates: {{preview.bates_policy.restart_per_volume ? 'restarts per volume' : 'continuous across volumes'}} · index pages {{preview.bates_policy.include_index ? 'included' : 'excluded'}} · separator pages {{preview.bates_policy.include_separator ? 'included' : 'excluded'}}. Physical volume page numbers remain separate.</p>
        <div v-for="volume in preview.volumes" :key="volume.number" class="bundle-preview-volume">
          <h3>Volume {{volume.number}} <small>{{volume.page_count}} pages · {{volume.index_pages}} index page(s)</small></h3>
          <p class="hint">Index: printed volume pages 1–{{volume.index_pages}} · combined starts at page {{volume.combined_start_page}} and ends at page {{volume.combined_end_page}}</p>
          <div class="bundle-preview-scroll"><table><thead><tr><th scope="col">Index entry / bookmark</th><th scope="col">Printed volume pages</th><th scope="col">Combined PDF pages</th><th v-if="preview.bates_policy.enabled" scope="col">Bates on source</th></tr></thead>
            <tbody><tr v-for="row in volume.rows" :key="row.id"><td><button class="bundle-preview-doc" :aria-label="'Open '+row.title+' working copy'" @click="emit('openDocument',row.id)">Part {{row.part}} · {{row.label ? row.label+' · ' : '' }}{{row.title}}</button><button class="bundle-preview-bookmark" :aria-label="'Bookmark '+row.title+' first page'" @click="emit('bookmarkDocument',row.id)"><span>+ Bookmark</span></button><small v-if="row.separator_page">Separator: printed page {{row.separator_page}}<span v-if="row.separator_bates"> · {{row.separator_bates}}</span></small><small v-else-if="row.annexure_mode==='label_stamp'">Confirmed label stamp on source {{row.start_page===row.end_page ? 'page' : 'page(s)'}}</small></td><td>{{row.start_page}}–{{row.end_page}}</td><td>{{row.combined_start_page}}–{{row.combined_end_page}}</td><td v-if="preview.bates_policy.enabled">{{row.bates_first}}<span v-if="row.bates_last!==row.bates_first">–{{row.bates_last}}</span></td></tr></tbody></table></div>
        </div>
        <div v-if="preview.manual_bookmarks.length" class="bundle-preview-manual"><h3>Manual bookmark destinations</h3>
          <ul><li v-for="bookmark in preview.manual_bookmarks" :key="bookmark.id"><strong>{{bookmark.title}}</strong> · {{bookmark.document_title}}, source page {{bookmark.document_page}} — <span v-if="bookmark.resolved">Volume {{bookmark.volume}}, printed page {{bookmark.volume_page}} · combined page {{bookmark.combined_page}}</span><strong v-else>Needs review; omitted from export</strong></li></ul>
        </div>
        <p class="hint">{{preview.numbering}} These are physical PDF pages, including index pages. Review the generated PDFs before use; the selected profile does not establish court approval.</p>
      </div>
    </div>
  </section>
</template>
