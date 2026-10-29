<script setup lang="ts">
import {computed,nextTick,onBeforeUnmount,ref,watch} from 'vue'
import {Info,ChevronDown,AlertTriangle,CircleCheck,FileText} from 'lucide-vue-next'
import {api} from '../lib/api'
import {friendlyError,prettyDate} from '../lib/format'
import type {ExportManifest,Job} from '../lib/types'
import type {LayoutSettings} from '../lib/bundleSettings'
import NumberingSettings from './NumberingSettings.vue'
import BundlePreview from './BundlePreview.vue'
import OutputChecksDialog from './OutputChecksDialog.vue'
type Source={id:string;title:string;revision:number;page_count:number;pages:Array<{id:string;page_index:number}>}
type PolicyMode='separate_supplement'|'preserve_printed_labels'
type Saved={id:string;version:number;title:string;baseline_build_id:string;selected_page_ids:string[];settings:LayoutSettings;updated_at:string;input_snapshot:string;insertion_policy:PolicyMode|null;reviewed_policy?:{mode:PolicyMode;contract_digest:string}|null}
type Catalog={plans:Saved[];documents:Source[];input_snapshot:string;defaults:LayoutSettings}
const props=defineProps<{matterId:string;baseline:ExportManifest;initialPlanId?:string;initialMode?:'plan'|'policy';blocked:boolean;refreshKey:number;job:Job|null;artifacts:ExportManifest[]}>()
const emit=defineEmits<{close:[];dirtyChanged:[boolean];saved:[string];context:[string];build:[string];cancel:[];retry:[Job];review:[string,string];export:[ExportManifest];finding:[any,string]}>()
const sources=ref<Source[]>([]),saved=ref<Saved|null>(null),title=ref('Supplementary bundle 1'),ids=ref<string[]>([]),settings=ref<LayoutSettings|null>(null),snapshot=ref(''),loaded=ref(false),loading=ref(false),saving=ref(false),error=ref(''),mapError=ref(''),plan=ref<any>(null)
const mode=ref<'plan'|'membership'|'numbering'|'map'|'policy'|'result'>('plan'),draftIds=ref<string[]>([]),draftSettings=ref<LayoutSettings|null>(null)
const insertionPolicy=ref<PolicyMode|null>(null),reviewedPolicySignature=ref(''),policyPreview=ref<any>(null),policyLoading=ref(false),policyError=ref('')
const checksOpen=ref(false)
const baselineId=computed(()=>saved.value?.baseline_build_id||props.baseline.id)
const policySignature=()=>JSON.stringify({baseline:baselineId.value,title:title.value,ids:ids.value,settings:settings.value,policy:insertionPolicy.value,snapshot:snapshot.value})
const policyAccepted=computed(()=>!!reviewedPolicySignature.value&&reviewedPolicySignature.value===policySignature())
const signature=()=>JSON.stringify({title:title.value,ids:ids.value,settings:settings.value,policy:insertionPolicy.value,reviewed:policyAccepted.value})
const clean=ref(''),dirty=computed(()=>loaded.value&&(signature()!==clean.value||mode.value==='membership'&&JSON.stringify(draftIds.value)!==JSON.stringify(ids.value)||mode.value==='numbering'&&JSON.stringify(draftSettings.value)!==JSON.stringify(settings.value)))
watch(dirty,v=>emit('dirtyChanged',v))
watch(()=>[title.value,ids.value.length],()=>emit('context',`${title.value} · ${ids.value.length} selected source pages`),{immediate:true})
const missing=computed(()=>ids.value.filter(id=>!sources.value.some(d=>d.pages.some(p=>p.id===id))))
const selectedGroups=computed(()=>sources.value.map(d=>({...d,selected:d.pages.filter(p=>ids.value.includes(p.id))})).filter(d=>d.selected.length))
const titleInput=ref<HTMLInputElement|null>(null),pickerTrigger=ref<HTMLButtonElement|null>(null),numberTrigger=ref<HTMLButtonElement|null>(null),mapTrigger=ref<HTMLButtonElement|null>(null),prepareTrigger=ref<HTMLButtonElement|null>(null)
let loadSequence=0,mapSequence=0,saveSequence=0,timer:ReturnType<typeof setTimeout>|undefined
const body=()=>({baseline_build_id:baselineId.value,title:title.value,selected_page_ids:ids.value,...settings.value,expected_snapshot:snapshot.value,expected_version:saved.value?.version,insertion_policy:insertionPolicy.value,policy_reviewed:policyAccepted.value})
async function load(preserve=false){
 const call=++loadSequence;loading.value=true;error.value='';plan.value=null;++mapSequence
 try{
  const data=await api<Catalog>(`/matters/${props.matterId}/supplements`)
  if(call!==loadSequence)return
  sources.value=data.documents;snapshot.value=data.input_snapshot
  if(!preserve){
   const existing=data.plans.find(p=>p.id===props.initialPlanId)
   if(props.initialPlanId&&!existing)throw new Error('Saved supplement is unavailable. Return and choose another saved plan.')
   saved.value=existing||null;title.value=existing?.title||'Supplementary bundle 1';ids.value=[...(existing?.selected_page_ids||[])];settings.value=structuredClone(existing?.settings||data.defaults);insertionPolicy.value=existing?.insertion_policy||null
   reviewedPolicySignature.value=existing?.reviewed_policy?policySignature():'';clean.value=signature();loaded.value=true
  }
  await refreshMap()
  if(props.initialMode==='policy'&&ids.value.length){mode.value='policy';insertionPolicy.value=insertionPolicy.value||'preserve_printed_labels';await refreshPolicy()}
 }catch(e){if(call===loadSequence)error.value=friendlyError(e)}finally{if(call===loadSequence)loading.value=false}
}
async function refreshMap(){
 const call=++mapSequence;plan.value=null;mapError.value=''
 if(!loaded.value||props.blocked||!settings.value)return
 try{const p=await api<any>(`/matters/${props.matterId}/supplement-preview`,'POST',body());if(call===mapSequence)plan.value=p}
 catch(e){if(call===mapSequence)mapError.value=friendlyError(e)}
}
watch(()=>[signature(),props.refreshKey,props.blocked],()=>{plan.value=null;++mapSequence;clearTimeout(timer);timer=setTimeout(()=>void refreshMap(),180)})
async function saveDraft(){
 if(saving.value||props.blocked||!loaded.value)return false
 if(mode.value==='membership')ids.value=[...draftIds.value]
 if(mode.value==='numbering')settings.value=JSON.parse(JSON.stringify(draftSettings.value))
 saving.value=true;error.value='';const call=++saveSequence
 try{
  const p=await api<Saved>(`/matters/${props.matterId}/supplements${saved.value?'/'+saved.value.id:''}`,saved.value?'PUT':'POST',body())
  if(call!==saveSequence)return false
  saved.value=p;if(p.reviewed_policy)reviewedPolicySignature.value=policySignature();clean.value=signature();if(mode.value!=='policy')mode.value='plan';emit('saved',p.id);await refreshMap();return true
 }catch(e){error.value=friendlyError(e);return false}finally{saving.value=false}
}
function discard(){if(saved.value){title.value=saved.value.title;ids.value=[...saved.value.selected_page_ids];settings.value=JSON.parse(JSON.stringify(saved.value.settings));insertionPolicy.value=saved.value.insertion_policy;reviewedPolicySignature.value=saved.value.reviewed_policy?policySignature():''}else{ids.value=[];title.value='Supplementary bundle 1';insertionPolicy.value=null;reviewedPolicySignature.value=''}mode.value='plan';clean.value=signature();emit('dirtyChanged',false)}
function openPicker(){if(saving.value||mode.value==='membership')return;draftIds.value=[...ids.value];mode.value='membership';void focusHeading()}
function openNumbering(){if(saving.value)return;draftSettings.value=JSON.parse(JSON.stringify(settings.value));mode.value='numbering';void focusHeading()}
async function focusHeading(){await nextTick();document.querySelector<HTMLElement>('.supplement-workspace h1')?.focus()}
async function returnPlan(apply=false){const caller=mode.value;if(apply){if(caller==='membership')ids.value=[...draftIds.value];if(caller==='numbering')settings.value=JSON.parse(JSON.stringify(draftSettings.value))}mode.value='plan';await nextTick();(caller==='membership'?pickerTrigger:caller==='numbering'?numberTrigger:caller==='map'?mapTrigger:prepareTrigger).value?.focus()}
function toggle(id:string){draftIds.value=draftIds.value.includes(id)?draftIds.value.filter(p=>p!==id):[...draftIds.value,id]}
async function prepare(){if(!ids.value.length){error.value='Select at least one source page before preparing the map.';return}if(!saved.value&&!(await saveDraft()))return;mode.value='policy';insertionPolicy.value=insertionPolicy.value||'separate_supplement';await refreshPolicy();await focusHeading()}
async function openMap(){if(!ids.value.length){error.value='Select at least one source page before preparing the map.';return}mode.value='map';await refreshMap();await focusHeading()}
async function refreshPolicy(){policyPreview.value=null;policyError.value='';if(!insertionPolicy.value||!settings.value)return;policyLoading.value=true;try{policyPreview.value=await api<any>(`/matters/${props.matterId}/supplement-policy-preview`,'POST',body())}catch(e){policyError.value=friendlyError(e)}finally{policyLoading.value=false}}
async function choosePolicy(value:PolicyMode){insertionPolicy.value=value;reviewedPolicySignature.value='';await refreshPolicy()}
async function acceptPolicy(){if(!policyPreview.value||policyPreview.value.blockers?.length)return;reviewedPolicySignature.value=policySignature();if(!(await saveDraft()))reviewedPolicySignature.value='';else{mode.value='plan';await nextTick();prepareTrigger.value?.focus()}}
async function buildSupplement(){if(!saved.value||!policyAccepted.value){mode.value='policy';await refreshPolicy();return}mode.value='result';emit('build',saved.value.id)}
const supplementJob=computed(()=>props.job?.kind==='supplement-build'&&props.job.target_id===saved.value?.id?props.job:null)
const artifact=computed(()=>props.artifacts.find(a=>a.artifact_kind==='supplement'&&a.supplement_plan_id===saved.value?.id&&a.supplement_plan_version===saved.value?.version)||null)
watch(()=>supplementJob.value?.status,status=>{if(status==='completed')mode.value='result'})
const leaveDialog=ref<HTMLDialogElement|null>(null)
function requestClose(){if(dirty.value)leaveDialog.value?.showModal();else emit('close')}
defineExpose({saveDraft,discard,openPicker,openNumbering,prepare,openMap})
void load()
onBeforeUnmount(()=>{loadSequence++;mapSequence++;saveSequence++;clearTimeout(timer);emit('dirtyChanged',false)})
</script>
<template>
<section class="supplement-workspace" :data-mode="mode" @keydown.esc.stop.prevent="mode==='plan'?requestClose():returnPlan()">
 <header class="supplement-heading" :class="{'policy-heading':mode==='policy'}"><div><h1 tabindex="-1">{{mode==='plan'?'Create supplementary bundle':mode==='membership'?'Supplement membership':mode==='numbering'?'Numbering & navigation':mode==='map'?'Supplement page map':mode==='result'?'Supplement ready for review':insertionPolicy==='preserve_printed_labels'?'Preserve existing references':'Create a separate supplement'}}</h1><p>{{mode==='plan'?'Choose new material. The selected earlier build remains unchanged.':mode==='policy'?(insertionPolicy==='preserve_printed_labels'?`Preview inserted labels against build ${baselineId.slice(0,8)} before applying this update policy.`:`Review the distinct artifact map against build ${baselineId.slice(0,8)} before accepting this policy.`):`${title} · ${ids.length} selected source pages · baseline ${baselineId.slice(0,8)}`}}</p></div><button class="membership-button secondary" :disabled="saving" @click="mode==='plan'?requestClose():returnPlan()">{{mode==='plan'?'Cancel':'Cancel'}}</button></header>
 <p v-if="loading" role="status">Loading saved supplemental material…</p>
 <div v-if="error" role="alert" class="supplement-error">{{error}} <button class="text-tool" :disabled="saving" @click="load(loaded)">Reload material; retain selection</button></div>
 <p v-if="blocked" role="status">Local engine busy or unavailable. Your supplemental draft is retained.</p>
 <template v-if="loaded&&settings">
 <template v-if="mode==='plan'">
 <div class="supplement-grid">
  <section class="supplement-card supplement-material"><h2>New material</h2><strong class="supplement-baseline">Baseline · build {{baselineId.slice(0,8)}} · {{baseline.pages}} physical pages</strong><p class="supplement-help">Choose document pages or add other documents from this matter.</p>
   <div class="supplement-selection" tabindex="0" aria-label="Selected supplemental material"><p v-if="!ids.length">No source pages selected. Choose material explicitly.</p><div v-for="d in selectedGroups" :key="d.id"><label><input type="checkbox" checked :disabled="saving||blocked" @change="ids=ids.filter(id=>!d.selected.some(p=>p.id===id))"/>{{d.title}} · source {{d.selected.map(p=>p.page_index+1).join(', ')}}</label><small>{{d.selected.length}} pages · working revision {{d.revision}} · explicit selection</small></div><p v-if="missing.length" role="alert">{{missing.length}} selected page identities are missing or stale. Choose material to remove them explicitly.</p></div>
   <button ref="pickerTrigger" class="membership-button secondary" :disabled="saving||blocked" @click="openPicker">Choose other material</button>
   <label class="supplement-title">Supplement title<input ref="titleInput" v-model="title" maxlength="200" :disabled="saving||blocked"/></label>
   <label class="supplement-numbering">Numbering<button ref="numberTrigger" class="supplement-field" :disabled="saving||blocked" @click="openNumbering"><span>{{settings.output_marks.numbering_prefix}}{{settings.output_marks.numbering_start}} onwards · {{settings.output_marks.numbering_style}}</span><ChevronDown :size="14"/></button></label>
  </section>
  <aside class="supplement-card supplement-summary"><h2>Supplement preview</h2><h3>{{title||'Untitled supplement'}}</h3><dl><div><dt>Selected source pages</dt><dd>{{ids.length}}</dd></div><div><dt>Generated pages</dt><dd>{{plan?.generated_pages??'—'}}</dd></div><div><dt>Planned physical pages</dt><dd>{{plan?.combined_pages??'—'}}</dd></div><div><dt>Volumes</dt><dd>{{plan?.volumes.length??'—'}}</dd></div></dl><div class="supplement-notice"><strong><Info :size="16"/>Linked to build {{baselineId.slice(0,8)}}</strong><p>This plan and every generated artifact remain separate from the earlier bundle.</p><p>{{policyAccepted?(insertionPolicy==='preserve_printed_labels'?'Reviewed insert labels accepted.':'Separate supplemental numbering accepted.'):'Choose and review a supported output policy.'}}</p></div></aside>
 </div>
 <p v-if="mapError" role="alert" class="supplement-error">{{mapError}} <button class="text-tool" @click="load(true)">Reload material; retain selection</button></p>
 <footer class="supplement-footer"><span>{{saved?`Plan ${saved.id.slice(0,8)} · v${saved.version}`:'New supplemental plan'}} · {{dirty?'Unsaved changes':saved?'Saved selection':'Not saved yet'}}<small>{{policyAccepted?'Policy reviewed for this exact selection and settings.':'Review the page map and an explicit policy before building.'}}</small></span><button class="membership-button secondary" :disabled="saving||blocked" @click="saveDraft">{{saving?'Saving…':'Save plan'}}</button><button ref="mapTrigger" class="membership-button secondary" :disabled="saving||blocked||!ids.length||!!missing.length" @click="openMap">Prepare supplement</button><button ref="prepareTrigger" class="membership-button secondary" :disabled="saving||blocked||!ids.length||!!missing.length" @click="prepare">{{policyAccepted?'Review policy':'Choose policy'}}</button><button v-if="policyAccepted" class="membership-button primary" :disabled="blocked||saving||!!supplementJob&&['queued','running'].includes(supplementJob.status)" @click="buildSupplement">Build supplement</button></footer>
 <details class="supplement-provenance"><summary>Baseline identity and provenance</summary><p>Saved locally. Portable matter backups do not yet include supplemental plans or prior build outputs.</p><p>Build {{baselineId}} · {{prettyDate(baseline.created_at)}} · immutable saved output</p><p v-for="f in baseline.files" :key="f.name">{{f.name}} · {{f.bytes}} bytes · {{f.sha256||'Digest not recorded'}}</p></details>
 </template>
 <template v-else-if="mode==='membership'">
  <div class="membership-summary">{{draftIds.length}} selected source pages · Selection order determines supplemental order. Primary membership is unchanged.</div>
  <section class="supplement-card supplement-picker"><h2>Choose source pages</h2><p>IDs refer to an exact stored working revision. New revisions require explicit reselection.</p><div v-for="d in sources" :key="d.id" class="supplement-document"><h3>{{d.title}} · revision {{d.revision}}</h3><div class="supplement-page-options"><label v-for="p in d.pages" :key="p.id" :title="p.id"><input type="checkbox" :checked="draftIds.includes(p.id)" :disabled="saving||blocked" :aria-label="`${d.title} source page ${p.page_index+1}`" @change="toggle(p.id)"/>Source {{p.page_index+1}}</label></div></div><div v-for="id in draftIds.filter(id=>!sources.some(d=>d.pages.some(p=>p.id===id)))" :key="id"><span>Unavailable identity {{id}}</span><button class="text-tool" @click="toggle(id)">Remove unavailable page</button></div></section>
  <footer class="supplement-footer"><span>{{draftIds.length}} selected · {{ids.length}} in caller draft</span><button class="membership-button secondary" @click="returnPlan()">Cancel selection</button><button class="membership-button primary" :disabled="blocked" @click="returnPlan(true)">Apply selection</button></footer>
 </template>
 <template v-else-if="mode==='numbering'&&draftSettings">
  <div class="supplement-numbering-grid"><NumberingSettings v-model="draftSettings.output_marks" :disabled="blocked||saving"/><section class="supplement-card"><h2>Supplement settings</h2><p>{{ids.length}} exact source-page identities remain selected.</p><label>Front matter<select v-model="draftSettings.output_marks.front_matter"><option value="cover_index">Cover + generated index</option><option value="index_only">Generated index only</option></select></label><label>Maximum pages per volume<input v-model.number="draftSettings.max_pages_per_volume" type="number" min="2" max="2000"/></label><p>These settings belong only to this supplemental plan. Changing them invalidates the reviewed insertion policy and current map.</p></section></div>
  <footer class="supplement-footer"><span>Apply returns to this supplement and invalidates its preview.</span><button class="membership-button secondary" @click="returnPlan()">Cancel numbering</button><button class="membership-button primary" :disabled="blocked" @click="returnPlan(true)">Apply numbering</button></footer>
 </template>
 <template v-else-if="mode==='policy'">
  <div class="supplement-policy-strip" :class="policyPreview?.blockers?.length?'danger':'info'"><Info v-if="!policyPreview?.blockers?.length" :size="16"/><AlertTriangle v-else :size="16"/><span>{{insertionPolicy==='preserve_printed_labels'?'Printed references stay fixed · Physical PDF positions will change':'A distinct artifact uses supplemental numbering · The baseline remains unchanged'}}</span></div>
  <p v-if="policyLoading" role="status">Reconciling exact page identities, labels and physical positions…</p><p v-if="policyError" class="supplement-error" role="alert">{{policyError}}</p>
  <section v-if="policyPreview" class="supplement-insertion-card"><h2>{{insertionPolicy==='preserve_printed_labels'?'Insertion preview':'Separate artifact preview'}} · {{policyPreview.artifact_map?.volumes?.length||0}} volume{{policyPreview.artifact_map?.volumes?.length===1?'':'s'}}</h2><strong>{{ids.length}} selected source pages · {{policyPreview.artifact_map?.combined_pages}} planned physical pages</strong>
   <div v-if="insertionPolicy==='preserve_printed_labels'" class="supplement-insertion-scroll"><table><thead><tr><th>Material</th><th>Source position</th><th>Printed / viewer label</th><th>Combined PDF position</th></tr></thead><tbody><tr v-for="(row,index) in policyPreview.preview_rows" :key="index"><th>{{row.material}}</th><td>{{row.source_position}}</td><td>{{row.printed_label}}</td><td>{{row.combined_position}}</td></tr></tbody></table></div>
   <div v-else class="supplement-separate-summary"><p>{{policyPreview.summary}}</p><dl><div><dt>Index rows</dt><dd>{{policyPreview.effects.index_rows}}</dd></div><div><dt>Viewer labels</dt><dd>{{policyPreview.effects.viewer_labels}}</dd></div><div><dt>Physical output</dt><dd>{{policyPreview.effects.physical_positions}}</dd></div></dl></div>
   <p class="supplement-insertion-note">Index rows, bookmarks, links and viewer labels are rebuilt from the selected deterministic page map. Earlier artifact bytes are never changed.</p>
  </section>
  <div v-if="policyPreview?.blockers?.length" class="supplement-policy-blockers" role="alert"><strong>Resolve before accepting</strong><p v-for="item in policyPreview.blockers" :key="item.code">{{item.message}}</p></div>
  <div v-else-if="policyPreview" class="supplement-policy-actions"><section><AlertTriangle :size="16"/><div><strong>Review the numbering policy</strong><p>Use this notation only after reviewing the exact map. Conflicts are blocked. It is not a court-approved rule.</p></div></section><button class="membership-button primary" :disabled="blocked||saving" @click="acceptPolicy">{{insertionPolicy==='preserve_printed_labels'?'Use insert numbering':'Use separate numbering'}}</button></div>
  <details v-if="policyPreview" class="supplement-policy-evidence"><summary>Mapping reconciliation and exact identities</summary><p>Baseline {{baselineId}} · plan {{saved?.id||'unsaved'}} · {{ids.length}} exact revision-bound IDs.</p><p>{{policyPreview.effects.bookmarks}}</p><p>{{policyPreview.effects.links}}</p><p v-for="boundary in policyPreview.volume_boundaries" :key="boundary.volume">Volume {{boundary.volume}} boundary: baseline {{boundary.baseline_end}} → reconciled {{boundary.reconciled_end}}</p><code v-for="id in ids" :key="id">{{id}}</code></details>
  <div class="supplement-policy-switch" role="group" aria-label="Supplement output policy"><span>Other supported policy:</span><button class="text-tool" v-if="insertionPolicy!=='separate_supplement'" @click="choosePolicy('separate_supplement')">Build as a separate supplement</button><button class="text-tool" v-if="insertionPolicy!=='preserve_printed_labels'" @click="choosePolicy('preserve_printed_labels')">Preview preserved printed labels</button></div>
 </template>
 <template v-else-if="mode==='map'">
  <div class="supplement-map-meta"><strong>{{ids.length}} selected source pages · {{plan?.generated_pages??'—'}} generated · {{plan?.combined_pages??'—'}} physical</strong><p>Plan {{saved?.id}} · baseline {{baselineId}}</p></div>
  <BundlePreview :matter-id="matterId" :max-pages="settings.max_pages_per_volume" :keep-groups="settings.keep_groups_intact" :refresh-key="refreshKey" :supplied-plan="plan" :external-plan="true" :inputs-pending="!plan||blocked"/>
  <p v-if="mapError" role="alert">{{mapError}}</p><footer class="supplement-footer"><button class="membership-button secondary" @click="returnPlan()">Back to supplement</button><button class="membership-button secondary" :disabled="blocked" @click="refreshMap">Recalculate selected map</button><button class="membership-button primary" :disabled="blocked||!policyAccepted" @click="buildSupplement">Build supplement</button></footer><p>The generated artifact, manifest and checks remain bound to this exact plan version, baseline, page identities, settings and reviewed policy.</p>
 </template>
 <template v-else-if="mode==='result'">
  <p v-if="!supplementJob&&!artifact" role="status">Submitting the reviewed supplemental plan…</p>
  <section v-if="supplementJob&&!artifact" class="supplement-card supplement-job"><CircleCheck v-if="supplementJob.status==='completed'" :size="18"/><AlertTriangle v-else :size="18"/><h2>{{supplementJob.status==='failed'?'Supplement build failed':supplementJob.message}}</h2><progress v-if="['queued','running'].includes(supplementJob.status)" :value="supplementJob.progress" max="100"/><p v-if="supplementJob.error" role="alert">{{supplementJob.error}}</p><button v-if="['queued','running'].includes(supplementJob.status)" class="membership-button secondary" @click="emit('cancel')">Cancel build</button><button v-else-if="supplementJob.status==='interrupted'" class="membership-button secondary" @click="emit('retry',supplementJob)">Retry as a new build</button><button v-else class="membership-button secondary" @click="returnPlan()">Return to plan</button></section>
  <section v-if="artifact" class="supplement-card supplement-result"><CircleCheck :size="18"/><h2>Distinct supplemental artifact generated</h2><p>Build {{artifact.id}} · plan {{artifact.supplement_plan_id}} v{{artifact.supplement_plan_version}} · baseline {{artifact.baseline_build_id}}</p><p>{{artifact.pages}} physical pages · {{artifact.volumes}} volume{{artifact.volumes===1?'':'s'}} · technically generated, not legally approved or released.</p><table><thead><tr><th>File</th><th>Bytes</th><th>Action</th></tr></thead><tbody><tr v-for="file in artifact.files" :key="file.name"><td>{{file.name}}</td><td>{{file.bytes.toLocaleString()}}</td><td><button v-if="file.name.endsWith('.pdf')" class="text-tool" @click="emit('review',artifact.id,file.name)"><FileText :size="15"/>Review</button></td></tr></tbody></table><footer class="supplement-footer"><button class="membership-button secondary" @click="returnPlan()">Back to plan</button><button class="membership-button secondary" @click="checksOpen=true">Output checks</button><button class="membership-button secondary" @click="emit('review',artifact.id,'combined.pdf')">Review supplement</button><button class="membership-button primary" @click="emit('export',artifact)">Export files</button></footer></section>
 </template>
 </template>
 <OutputChecksDialog v-if="checksOpen&&artifact" :matter-id="matterId" :artifact="artifact" @close="checksOpen=false" @review="checksOpen=false;emit('review',artifact.id,'combined.pdf')" @finding="checksOpen=false;emit('finding',$event,artifact.id)"/>
 <dialog @keydown.esc.stop.prevent="leaveDialog?.close()" ref="leaveDialog" class="build-tool-dialog" @cancel.prevent="leaveDialog?.close()"><h2>Keep this supplemental draft?</h2><p>Cancel returns without saving new changes. Any previously saved plan stays available.</p><div class="build-actions"><button class="membership-button secondary" @click="leaveDialog?.close()">Keep editing</button><button class="membership-button secondary" @click="discard();leaveDialog?.close();emit('close')">Discard changes</button><button class="membership-button primary" :disabled="saving||blocked" @click="saveDraft().then(ok=>{if(ok){leaveDialog?.close();emit('close')}})">Save and return</button></div><p v-if="error" role="alert">{{error}}</p></dialog>
</section>
</template>
