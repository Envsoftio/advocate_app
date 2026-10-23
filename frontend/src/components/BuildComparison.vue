<script setup lang="ts">
import { ref, watch } from 'vue'
import { api } from '../lib/api'
import { friendlyError, prettyDate } from '../lib/format'

type PageRow = { id:string; part:number; label:string; title:string; volume:number;
  start_page:number; end_page:number; combined_start_page:number; combined_end_page:number }
type Change = { document_id:string; title:string; reasons:string[]; before:PageRow|null; after:PageRow|null }
type Comparison = { available:boolean; reason?:string; prior_build_id:string; prior_created_at:string;
  prior_pages:number; current_pages:number; changes:Change[]; setting_changes:string[];
  bookmark_changes:string[]; unresolved_bookmarks:string[]; other_inputs_changed:boolean;
  current_matches_prior:boolean }
const props = defineProps<{matterId:string; exportId:string; maxPages:number;
  keepGroups:boolean; refreshKey:number}>()
const result = ref<Comparison|null>(null)
const error = ref('')
const loading = ref(false)
let requestNumber = 0
const range = (row:PageRow|null) => row
  ? `Volume ${row.volume} pages ${row.start_page}–${row.end_page}; combined pages ${row.combined_start_page}–${row.combined_end_page}`
  : 'Not included'
async function refresh() {
  const number = ++requestNumber
  loading.value = true
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
</script>

<template>
  <section class="panel build-comparison" aria-label="What changed since the selected build">
    <div class="panel-header"><div><h2>What changed since this build?</h2>
      <span>Compare its recorded page map with the current draft plan</span></div>
      <button class="btn secondary small" :disabled="loading" @click="refresh">Refresh</button></div>
    <div class="build-comparison-body">
      <p v-if="loading" role="status">Comparing build and current plan…</p>
      <p v-else-if="error" class="error-msg" role="alert">{{error}}</p>
      <p v-else-if="result && !result.available" class="hint">{{result.reason}}</p>
      <template v-else-if="result">
        <p class="hint">Build {{result.prior_build_id.slice(0, 8)}} · {{prettyDate(result.prior_created_at)}} · {{result.prior_pages}} pages then, {{result.current_pages}} in the current plan.</p>
        <p v-if="result.current_matches_prior" role="status">The recorded inputs and saved settings still match this build.</p>
        <p v-else-if="!result.changes.length && !result.setting_changes.length && !result.bookmark_changes.length && !result.other_inputs_changed">The draft page plan is unchanged. Review the build before saving it.</p>
        <p v-if="result.setting_changes.length"><strong>Settings changed:</strong> {{result.setting_changes.join(', ')}}.</p>
        <p v-if="result.bookmark_changes.length"><strong>Bookmarks edited:</strong> {{result.bookmark_changes.join(', ')}}.</p>
        <p v-if="result.other_inputs_changed">Other recorded inputs changed. Inspect technical checks and rebuild before use.</p>
        <div v-if="result.changes.length" class="bundle-preview-scroll"><table><thead><tr><th>Document</th><th>Change</th><th>Earlier build</th><th>Current plan</th></tr></thead>
          <tbody><tr v-for="change in result.changes" :key="change.document_id"><td><strong>{{change.title}}</strong><small v-if="change.before?.label !== change.after?.label">Label: {{change.before?.label || 'none'}} → {{change.after?.label || 'none'}}</small></td><td>{{change.reasons.join(', ')}}</td><td>{{range(change.before)}}</td><td>{{range(change.after)}}</td></tr></tbody></table></div>
        <p v-if="result.unresolved_bookmarks.length" class="notice amber"><strong>Bookmarks need review:</strong> {{result.unresolved_bookmarks.join(', ')}}.</p>
        <p class="hint">Page shifts can affect references outside this package. Review those references manually; rebuilding does not update source citations.</p>
      </template>
    </div>
  </section>
</template>
