<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import {CheckCircle2,TriangleAlert,RefreshCw} from 'lucide-vue-next'
import type {ManualBookmark,MatterDocument} from '../lib/types'
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
  suppliedPlan?: Preview | null
  externalPlan?: boolean
  documents?: MatterDocument[]
  recalculateKey?: number
  inputsPending?: boolean
  matterId: string
  maxPages: number
  keepGroups: boolean
  refreshKey: number
}>()
const emit = defineEmits<{ openDocument: [id: string], bookmarkDocument: [id: string], reviewBookmark: [id: string, removal: boolean], stateChanged: [state:string] }>()
const preview = ref<Preview | null>(null)
const loading = ref(false)
const error = ref('')
let sequence = 0
const selected=ref('')
const bookmarks=ref<ManualBookmark[]>([])
const unresolved=computed(()=>preview.value?.manual_bookmarks.filter(b=>!b.resolved)||[])
const unresolvedSelection=ref('')
const unresolvedEntry=computed(()=>unresolved.value.find(b=>b.id===unresolvedSelection.value)||unresolved.value[0])
const unresolvedDocument=computed(()=>bookmarks.value.find(b=>b.id===unresolvedEntry.value?.id)?.document_id)
const selectedRow=computed(()=>rows.value.find(r=>`${r.id}:${r.source_start}`===selected.value)||rows.value[0])
function rowUnresolved(id:string){return unresolved.value.some(b=>bookmarks.value.find(item=>item.id===b.id)?.document_id===id)}
const state=computed(()=>props.inputsPending?'Pending':loading.value?'Recalculating':error.value?'Error':preview.value?'Current':'Pending')
watch(state,value=>emit('stateChanged',value),{immediate:true})
const rows=computed(()=>preview.value?.volumes.flatMap(v=>v.rows)||props.documents?.filter(d=>d.bundle_included).map(d=>({...d,source_start:0,source_end:d.page_count,total_pages:d.page_count,volume:null,printed_start:'',printed_end:'',combined_start_page:null,combined_end_page:null}))||[])
onBeforeUnmount(()=>{sequence++})

async function refresh() {
  const call = ++sequence
  if (props.externalPlan) { preview.value=props.suppliedPlan||null; loading.value=false; error.value=''; return }
  preview.value = null
  bookmarks.value = []
  if (props.inputsPending) { loading.value=false; error.value=''; return }
  if (!Number.isInteger(props.maxPages) || props.maxPages < 2 || props.maxPages > 2000) {
    preview.value = null
    error.value = 'Enter a volume page cap from 2 to 2000.'
    return
  }
  loading.value = true
  error.value = ''
  try {
    const [result, savedBookmarks] = await Promise.all([api<Preview>(`/matters/${props.matterId}/bundle-preview`, 'POST', {
      max_pages_per_volume: props.maxPages,
      keep_groups_intact: props.keepGroups,
    }), api<ManualBookmark[]>(`/matters/${props.matterId}/bookmarks`)])
    if (call === sequence) { preview.value = result; bookmarks.value = savedBookmarks }
  } catch (err) {
    if (call === sequence) { preview.value = null; error.value = friendlyError(err) }
  } finally { if (call === sequence) loading.value = false }
}
watch(() => [props.matterId, props.maxPages, props.keepGroups, props.refreshKey, props.inputsPending,props.recalculateKey,props.suppliedPlan],
  () => { void refresh() }, {immediate:true})
</script>

<template>
<section class="page-map" aria-label="Index and page number preview">
 <header><div><h2>Page mapping</h2><p>{{state==='Current'?'Source = working-copy document pages':'Source counts are current. Printed and combined positions require recalculation.'}}</p></div><span :class="['map-state',unresolved.length?'pending':state.toLowerCase()]" role="status"><CheckCircle2 v-if="state==='Current'&&!unresolved.length" :size="14"/><RefreshCw v-else-if="loading" :size="14"/><TriangleAlert v-else :size="14"/>{{unresolved.length?`${unresolved.length} unresolved`:state}}</span></header>
 <p v-if="error" role="alert">{{error}} <button class="text-tool" @click="refresh">Retry map</button></p>
 <div class="page-map-scroll"><table><thead><tr><th>Document</th><th>Source</th><th>Printed volume</th><th>Combined PDF</th><th>Bookmark target</th><th>Status</th></tr></thead><tbody>
 <tr v-for="row in rows" :key="`${row.id}:${row.source_start}`" :class="{selected:selectedRow===row}" @click="selected=`${row.id}:${row.source_start}`" @focusin="selected=`${row.id}:${row.source_start}`">
 <td><span v-if="externalPlan">{{row.title}}</span><button v-else class="map-document" :aria-label="'Open '+row.title+' working copy'" @click="emit('openDocument',row.id)">{{row.label?row.label+' · ':''}}{{row.title}}</button></td><td>{{row.source_start+1}}–{{row.source_end}}</td><td>{{state==='Current'?`${row.printed_start}–${row.printed_end}`:state}}</td><td>{{state==='Current'?`${row.combined_start_page}–${row.combined_end_page}`:state}}</td><td><span v-if="rowUnresolved(row.id)" class="map-unresolved-target">Needs resolution</span><span v-else-if="externalPlan">Source {{row.source_start+1}}</span><button v-else class="map-source" :aria-label="'Bookmark '+row.title+' first page'" @click="emit('bookmarkDocument',row.id)">Source 1 <span>＋</span></button></td><td><span :class="['map-state',rowUnresolved(row.id)?'pending':state.toLowerCase()]"><CheckCircle2 v-if="state==='Current'&&!rowUnresolved(row.id)" :size="13"/><TriangleAlert v-else :size="13"/>{{rowUnresolved(row.id)?'Unresolved':state==='Current'?'Ready':state}}</span></td>
 </tr><tr v-if="!rows.length"><td colspan="6">No included documents. Use Bundle membership to select documents for this index.</td></tr></tbody></table></div>
 <p v-if="preview?.part_extracts.length" class="placement-note">Technical Part PDFs: {{preview.part_extracts.map(p=>`Part ${p.part} (${p.pages} source pages)`).join(' · ')}}</p>
 <section v-if="unresolvedEntry" class="map-unresolved" aria-label="Unresolved bookmark destination">
  <strong><TriangleAlert :size="16"/>Bookmark destination unresolved</strong>
  <label v-if="unresolved.length>1">Bookmark to review<select :value="unresolvedEntry.id" @change="unresolvedSelection=($event.target as HTMLSelectElement).value"><option v-for="b in unresolved" :key="b.id" :value="b.id">{{b.title}}</option></select></label>
  <p>{{unresolvedEntry.title}} → {{unresolvedEntry.document_title}}, source {{unresolvedEntry.document_page}}. The saved destination is unavailable in the current bundle.</p>
  <div v-if="!externalPlan"><button class="membership-button primary" @click="emit('reviewBookmark',unresolvedEntry.id,false)">Select destination</button><button class="membership-button quiet" @click="emit('reviewBookmark',unresolvedEntry.id,true)">Remove bookmark</button><button v-if="unresolvedDocument&&documents?.some(d=>d.id===unresolvedDocument)" class="membership-button quiet" @click="emit('openDocument',unresolvedDocument)">Inspect document</button></div>
 </section>
 <footer v-else><strong>{{state==='Current'?'Three references. One exact destination.':`Page mapping ${state.toLowerCase()}`}}</strong><p v-if="state==='Current'&&selectedRow">Source {{selectedRow.source_start+1}}–{{selectedRow.source_end}} → Printed {{selectedRow.printed_start}}–{{selectedRow.printed_end}} → Combined PDF {{selectedRow.combined_start_page}}–{{selectedRow.combined_end_page}}</p><p v-if="inputsPending">Save current inputs and finish processing before recalculating.</p><p v-else-if="preview">{{preview.volumes.length}} volume(s) · {{preview.combined_pages}} physical pages. Printed labels are separate from physical destinations.</p><p>Earlier builds retain their frozen maps. Imported originals remain unchanged.</p></footer>
 <details v-if="preview" class="page-map-details"><summary>Volume, Bates and manual destination details</summary><div v-for="volume in preview.volumes" :key="volume.number" class="bundle-preview-volume"><h3>Volume {{volume.number}}</h3><p>{{volume.cover_pages}} cover + {{volume.index_pages}} index · {{volume.page_count}} physical pages · combined {{volume.combined_start_page}}–{{volume.combined_end_page}}</p><p v-for="row in volume.rows" :key="row.id">{{row.title}} · revision {{row.revision}}<template v-if="row.separator_page"> · separator at physical {{row.separator_page}}</template><template v-if="row.bates_first"> · Bates {{row.bates_first}}–{{row.bates_last}}</template></p></div><p v-for="b in preview.manual_bookmarks" :key="b.id">{{b.title}} · {{b.document_title}}, source {{b.document_page}} — {{b.resolved?`Volume ${b.volume}, physical ${b.volume_page}; combined ${b.combined_page}`:'Needs resolution; omitted from export'}}</p></details>
</section>
</template>
