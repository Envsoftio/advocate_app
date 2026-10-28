<script setup lang="ts">
import {computed,nextTick,onBeforeUnmount,onMounted,ref,watch} from 'vue'
import {X} from 'lucide-vue-next'
import {outputCheckContexts,useOutputValidation, type OutputCheck,type OutputFinding} from '../lib/outputValidation'
import {prettyDate} from '../lib/format'
import type {ExportManifest} from '../lib/types'
const props=defineProps<{matterId:string;artifact:ExportManifest}>()
const emit=defineEmits<{close:[];review:[];changed:[];finding:[finding:any]}>()
const {state,pending,error,run}=useOutputValidation(()=>[props.matterId,props.artifact.id])
const contextKey=`${props.matterId}/${props.artifact.id}`,context=outputCheckContexts.get(contextKey)
const dialog=ref<HTMLDialogElement|null>(null),file=ref(context?.file||''),runId=ref(context?.runId||''),copied=ref(false)
let findingId=context?.findingId
const selectedRun=computed(()=>state.value?.history.find(r=>r.id===runId.value)||state.value?.history[0]||null)
const evidenceRow=ref<OutputCheck|null>(null)
function resultLabel(row:OutputCheck){
 if(row.outcome==='passed'){
  if(row.id==='files')return `${selectedRun.value?.selected_files.length} / ${selectedRun.value?.selected_files.length}`
  if(row.id==='counts')return `${(row.evidence.files as string[]).length} / ${(row.evidence.files as string[]).length}`
  if(row.id==='navigation')return 'No missing targets'
  if(row.id==='labels')return 'Consistent'
  if(row.id==='sizes')return `Within ${row.evidence.limit_mb} MB`
 }
 if(row.id==='cleanup')return 'Not selected'
 if(row.id==='redaction')return row.outcome==='not-applicable'?'No recorded redactions':'Verification unavailable'
 if(row.outcome==='not-applicable')return row.id==='sizes'?'No configured limit':'No PDFs selected'
 return row.outcome==='failed'?'Discrepancies found':row.outcome==='warning'?'Review omissions':'Contract unavailable'
}
const rows=computed(()=>selectedRun.value?.checks||[])
const sourcePages=computed(()=>props.artifact.page_map_snapshot?.reduce((n,r)=>n+r.end_page-r.start_page+1,0)||0)
let focus:HTMLElement|null=null
async function runChecks(){await run(file.value?[file.value]:undefined);runId.value='';evidenceRow.value=null;emit('changed')}
function openFinding(finding:OutputFinding){runId.value=selectedRun.value?.id||'';findingId=finding.finding_id;emit('finding',finding)}
watch(rows,async()=>{if(findingId){await nextTick();dialog.value?.querySelector<HTMLButtonElement>(`[data-finding-id="${CSS.escape(findingId)}"]`)?.focus()}},{flush:'post'})
async function copy(){await navigator.clipboard.writeText(JSON.stringify({build:props.artifact,validation:selectedRun.value},null,2));copied.value=true}
onMounted(()=>{focus=document.activeElement as HTMLElement;dialog.value?.showModal()})
onBeforeUnmount(()=>{outputCheckContexts.set(contextKey,{file:file.value,runId:runId.value,findingId});dialog.value?.close();void nextTick(()=>focus?.isConnected&&focus.focus())})
</script>
<template>
 <dialog ref="dialog" class="output-checks-dialog" aria-labelledby="output-checks-title" @cancel.prevent="emit('close')">
  <header><h2 id="output-checks-title">Output checks · build {{artifact.id.slice(0,8)}}</h2><button class="icon-btn" aria-label="Close output checks" @click="emit('close')"><X :size="18"/></button></header>
  <p class="output-checks-intro">These results belong to the saved output files. Human review is tracked separately.</p>
  <div class="output-checks-body">
   <section class="output-checks-results">
    <label>Output file<select aria-label="Output file" v-model="file" :disabled="pending"><option value="">All outputs · {{artifact.files.length}} saved files</option><option v-for="item in artifact.files" :key="item.name" :value="item.name">{{item.name}}</option></select></label>
    <div class="output-checks-table"><table><thead><tr><th>Check</th><th>Result</th><th>State</th></tr></thead><tbody><tr v-for="row in rows" :key="row.id"><td><button class="qa-check-name" :aria-label="'Evidence: '+row.name" @click="evidenceRow=evidenceRow?.id===row.id?null:row">{{row.name}}</button></td><td><span :title="row.evidence.summary">{{resultLabel(row)}}</span><button v-for="finding in row.findings" :key="finding.finding_id" class="text-tool" :data-finding-id="finding.finding_id" @click="openFinding(finding)">Review source {{finding.page_index+1}}</button></td><td :data-outcome="row.outcome">{{row.outcome.split('-').join(' ')}}</td></tr></tbody></table></div>
    <p v-if="pending" role="status">Checking saved file identities…</p><p v-else-if="!selectedRun">No recorded checks. Run checks before exporting this build.</p>
    <p v-if="error" role="alert" class="qa-error">{{error}}</p>
    <div class="qa-run-actions"><button class="membership-button quiet" :disabled="pending" @click="runChecks">{{pending?'Checking…':'Run output checks'}}</button><select v-model="runId" aria-label="Validation history"><option value="">Latest run</option><option v-for="r in state?.history" :key="r.id" :value="r.id">{{prettyDate(r.created_at)}} · {{r.id.slice(0,6)}} · {{r.selected_files.length}} files</option></select></div>
    <details v-if="evidenceRow" open class="qa-evidence"><summary>{{evidenceRow.name}} · measured evidence</summary><pre>{{JSON.stringify(evidenceRow.evidence,null,2)}}</pre></details>
    <small v-if="selectedRun">Run {{selectedRun.id.slice(0,8)}} · {{selectedRun.selected_files.join(', ')}}</small>
   </section>
   <aside class="output-build-details"><h3>Build details</h3><dl><div><dt>Preparation</dt><dd>{{artifact.id.slice(0,8)}}</dd></div><div><dt>Included documents</dt><dd>{{artifact.input_revisions?.length ?? 'Unavailable'}}</dd></div><div><dt>Source pages</dt><dd>{{sourcePages}}</dd></div><div><dt>Generated pages</dt><dd>{{artifact.pages-sourcePages}}</dd></div><div><dt>Output pages</dt><dd>{{artifact.pages}}</dd></div><div><dt>Input status</dt><dd>{{state?.stale?'Historical':'Saved snapshot'}}</dd></div></dl><p>The reviewed and exported files use the same saved build identity.</p><button class="membership-button quiet" @click="copy">{{copied?'Copied':'Copy build details'}}</button></aside>
  </div>
  <footer><button class="membership-button secondary" @click="emit('review')">Review bundle</button><button class="membership-button primary" @click="emit('close')">Done</button></footer>
 </dialog>
</template>
