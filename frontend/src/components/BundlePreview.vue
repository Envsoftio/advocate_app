<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import {CheckCircle2,TriangleAlert,RefreshCw} from 'lucide-vue-next'
import type {MatterDocument} from '../lib/types'
import { api } from '../lib/api'
import { friendlyError } from '../lib/format'

interface Row {
  volume: number
  id: string
  part: number
  label: string
  title: string
  source_start: number
  source_end: number
  total_pages: number
  printed_start: string
  printed_end: string
  revision: number
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
  cover_pages: number
  index_pages: number
  page_count: number
  combined_start_page: number
  combined_end_page: number
  rows: Row[]
  bates_labels: Array<string | null>
}
interface Preview {
  input_snapshot: string
  settings_snapshot: {max_pages_per_volume:number,keep_groups_intact:boolean}
  volumes: Volume[]
  combined_pages: number
  part_extracts: Array<{part:number;documents:number;pages:number}>
  manual_bookmarks: Array<{id:string,title:string,document_title:string,document_page:number,resolved:boolean,volume:number|null,volume_page:number|null,combined_page:number|null}>
  numbering: string
  bates_policy: {enabled:boolean;include_index:boolean;include_separator:boolean;restart_per_volume:boolean}
}
const props = defineProps<{
  documents?: MatterDocument[]
  recalculateKey?: number
  inputsPending?: boolean
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
const selected=ref('')
const state=computed(()=>props.inputsPending?'Pending':loading.value?'Recalculating':error.value?'Error':preview.value?'Current':'Pending')
const rows=computed(()=>preview.value?.volumes.flatMap(v=>v.rows)||props.documents?.filter(d=>d.bundle_included).map(d=>({...d,source_start:0,source_end:d.page_count,total_pages:d.page_count,volume:null,printed_start:'',printed_end:'',combined_start_page:null,combined_end_page:null}))||[])
onBeforeUnmount(()=>{sequence++})

async function refresh() {
  const call = ++sequence
  preview.value = null
  if (props.inputsPending) { loading.value=false; error.value=''; return }
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
watch(() => [props.matterId, props.maxPages, props.keepGroups, props.refreshKey, props.inputsPending,props.recalculateKey],
  () => { void refresh() }, {immediate:true})
</script>

<template>
<section class="page-map" aria-label="Index and page number preview">
 <header><div><h2>Page mapping</h2><p>{{state==='Current'?'Printed references and physical destinations reflect current accepted inputs.':'Source counts are current. Printed and combined positions require recalculation.'}}</p></div><span :class="['map-state',state.toLowerCase()]" role="status"><CheckCircle2 v-if="state==='Current'" :size="14"/><RefreshCw v-else-if="loading" :size="14"/><TriangleAlert v-else :size="14"/>{{state}}</span></header>
 <p v-if="error" role="alert">{{error}} <button class="text-tool" @click="refresh">Retry map</button></p>
 <div class="page-map-scroll"><table><thead><tr><th>Document</th><th>Source</th><th>Printed volume</th><th>Combined PDF</th><th>Bookmark target</th><th>Status</th></tr></thead><tbody>
 <tr v-for="row in rows" :key="`${row.id}:${row.source_start}`" :class="{selected:selected===row.id}" @click="selected=row.id">
 <td><button class="map-document" :aria-label="'Open '+row.title+' working copy'" @click="emit('openDocument',row.id)">{{row.label?row.label+' · ':''}}{{row.title}}</button></td><td>{{row.source_start+1}}–{{row.source_end}}</td><td>{{state==='Current'?`${row.printed_start}–${row.printed_end}`:state}}</td><td>{{state==='Current'?`${row.combined_start_page}–${row.combined_end_page}`:state}}</td><td><button class="map-source" :aria-label="'Bookmark '+row.title+' first page'" @click="emit('bookmarkDocument',row.id)">Source 1 <span>＋</span></button></td><td><span :class="['map-state',state.toLowerCase()]"><CheckCircle2 v-if="state==='Current'" :size="13"/><TriangleAlert v-else :size="13"/>{{state}}</span></td>
 </tr></tbody></table></div>
 <p v-if="preview?.part_extracts.length" class="placement-note">Technical Part PDFs: {{preview.part_extracts.map(p=>`Part ${p.part} (${p.pages} source pages)`).join(' · ')}}</p>
 <footer><strong>Page mapping {{state.toLowerCase()}}</strong><p v-if="inputsPending">Save current inputs and finish processing before recalculating.</p><p v-else-if="preview">{{preview.volumes.length}} volume(s) · {{preview.combined_pages}} physical pages. Printed labels are separate from physical destinations.</p><p>Earlier builds retain their frozen maps. Imported originals remain unchanged.</p></footer>
 <details v-if="preview" class="page-map-details"><summary>Volume, Bates and manual destination details</summary><div v-for="volume in preview.volumes" :key="volume.number" class="bundle-preview-volume"><h3>Volume {{volume.number}}</h3><p>{{volume.cover_pages}} cover + {{volume.index_pages}} index · {{volume.page_count}} physical pages · combined {{volume.combined_start_page}}–{{volume.combined_end_page}}</p><p v-for="row in volume.rows" :key="row.id">{{row.title}} · revision {{row.revision}}<template v-if="row.separator_page"> · separator at physical {{row.separator_page}}</template><template v-if="row.bates_first"> · Bates {{row.bates_first}}–{{row.bates_last}}</template></p></div><p v-for="b in preview.manual_bookmarks" :key="b.id">{{b.title}} · {{b.document_title}}, source {{b.document_page}} — {{b.resolved?`Volume ${b.volume}, physical ${b.volume_page}; combined ${b.combined_page}`:'Needs resolution; omitted from export'}}</p></details>
</section>
</template>
