<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { api } from '../lib/api'
import { friendlyError, prettyDate } from '../lib/format'

type PageRow = { id:string; part:number; label:string; title:string; volume:number;
  start_page:number; end_page:number; combined_start_page:number; combined_end_page:number }
type Change = { document_id:string; title:string; reasons:string[]; before:PageRow|null; after:PageRow|null }
type Revision = {document_id:string;title:string;status:string;reasons:string[];before_revision:number|null;after_revision:number|null;before_sha256:string|null;after_sha256:string|null;before_source_pages:number;after_source_pages:number}
type Comparison = {documents:Revision[]; available:boolean; reason?:string; prior_build_id:string; prior_created_at:string;
  prior_pages:number; current_pages:number; changes:Change[]; setting_changes:string[];
  bookmark_changes:string[]; unresolved_bookmarks:string[]; other_inputs_changed:boolean;
  current_matches_prior:boolean }
const props = defineProps<{matterId:string; exportId:string; maxPages:number;
  keepGroups:boolean; refreshKey:number}>()
const result = ref<Comparison|null>(null)
const selected=ref('')
const detail=computed(()=>result.value?.documents?.find(d=>d.document_id===selected.value)||result.value?.documents?.find(d=>d.status!=='unchanged')||result.value?.documents?.[0])
const error = ref('')
const loading = ref(false)
let requestNumber = 0
const range = (row:PageRow|null) => row
  ? `Volume ${row.volume} pages ${row.start_page}–${row.end_page}; combined pages ${row.combined_start_page}–${row.combined_end_page}`
  : 'Not included'
async function refresh() {
  const number = ++requestNumber
  loading.value = true
  result.value = null
  error.value = ''
  try {
    const data = await api<Comparison>(`/matters/${props.matterId}/exports/${props.exportId}/compare`, 'POST', {
      max_pages_per_volume: props.maxPages, keep_groups_intact: props.keepGroups,
    })
    if (number === requestNumber) result.value = data
  } catch (err) {
    if (number === requestNumber) { result.value = null; error.value = friendlyError(err) }
  } finally { if (number === requestNumber) loading.value = false }
}
watch(() => [props.matterId, props.exportId, props.maxPages, props.keepGroups, props.refreshKey],
  () => { void refresh() }, {immediate:true})
onBeforeUnmount(()=>{requestNumber++})
</script>

<template>
 <section class="revision-comparison" aria-label="What changed since the selected build">
  <p v-if="loading" role="status">Comparing stored revisions and current membership…</p>
  <p v-else-if="error" role="alert">{{error}} <button class="text-tool" @click="refresh">Retry comparison</button></p>
  <p v-else-if="result&&!result.available" role="status">{{result.reason}}</p>
  <template v-else-if="result">

   <div class="revision-comparison-scroll"><table><thead><tr><th>Document</th><th>Earlier version</th><th>Current version</th><th>Source pages</th><th>Impact</th></tr></thead><tbody><tr v-for="d in result.documents" :key="d.document_id" :class="{selected:detail?.document_id===d.document_id}"><td><button @click="selected=d.document_id">{{d.title}}</button></td><td>{{d.before_revision?`Revision ${d.before_revision}`:'Not included'}}</td><td>{{d.after_revision?`Revision ${d.after_revision}`:'Not included'}}</td><td>{{d.before_source_pages}} → {{d.after_source_pages}}</td><td>{{d.status}}<br/>{{d.reasons.filter(reason=>reason!==d.status).join(', ')}}</td></tr></tbody></table></div>
   <section v-if="detail" class="supplement-card revision-detail"><h2>{{detail.title}} · revision comparison</h2><dl><div><dt>Earlier build</dt><dd>{{detail.before_source_pages}} selected source pages in build {{result.prior_build_id.slice(0,8)}} · {{prettyDate(result.prior_created_at)}}. {{detail.before_revision?`Stored revision ${detail.before_revision}`:'Absent from baseline membership'}}.</dd></div><div><dt>Current working copy</dt><dd>{{detail.after_source_pages}} included source pages. {{detail.reasons.join('; ')||'Stored revision and mapped membership unchanged.'}}</dd></div><div><dt>Bundle impact</dt><dd>Earlier output: {{result.prior_pages}} physical pages. Current plan: {{result.current_pages}}. Review printed references, volume positions and navigation.</dd></div></dl></section>
   <div class="revision-status"><span v-for="status in ['added','changed','omitted','unchanged']" :key="status">{{result.documents.filter(d=>d.status===status).length}} {{status}}</span><button class="text-tool" @click="refresh">Refresh comparison</button></div>
   <details><summary>Recorded hashes, page ranges and settings</summary><p v-if="detail">Earlier SHA-256: {{detail.before_sha256||'Not recorded'}}<br/>Current SHA-256: {{detail.after_sha256||'Not included'}}</p><p v-if="result.current_matches_prior">The recorded inputs and saved settings still match this build.</p><p v-if="result.setting_changes.length"><strong>Settings changed:</strong> {{result.setting_changes.join(', ')}}</p><p v-if="result.bookmark_changes.length">Bookmarks edited: {{result.bookmark_changes.join(', ')}}</p><p v-if="result.other_inputs_changed">Other recorded inputs changed. Inspect checks before rebuilding.</p><p v-for="(c,i) in result.changes" :key="i">{{c.title}} · {{c.reasons.join(', ')}}<br/>Earlier: {{range(c.before)}}<br/>Current: {{range(c.after)}}</p><p v-if="result.unresolved_bookmarks.length">Bookmarks need review: {{result.unresolved_bookmarks.join(', ')}}</p><p>Page shifts may affect citations outside this package. Rebuilding does not update source citations.</p></details>
  </template>
 </section>
</template>
