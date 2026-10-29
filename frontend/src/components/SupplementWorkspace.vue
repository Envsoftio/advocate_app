<script setup lang="ts">
import {computed,nextTick,onBeforeUnmount,ref,watch} from 'vue'
import {Info,ChevronDown} from 'lucide-vue-next'
import {api} from '../lib/api'
import {friendlyError,prettyDate} from '../lib/format'
import type {ExportManifest} from '../lib/types'
import type {LayoutSettings} from '../lib/bundleSettings'
import NumberingSettings from './NumberingSettings.vue'
import BundlePreview from './BundlePreview.vue'
type Source={id:string;title:string;revision:number;page_count:number;pages:Array<{id:string;page_index:number}>}
type Saved={id:string;version:number;title:string;baseline_build_id:string;selected_page_ids:string[];settings:LayoutSettings;updated_at:string}
type Catalog={plans:Saved[];documents:Source[];input_snapshot:string;defaults:LayoutSettings}
const props=defineProps<{matterId:string;baseline:ExportManifest;initialPlanId?:string;blocked:boolean;refreshKey:number}>()
const emit=defineEmits<{close:[];dirtyChanged:[boolean];saved:[string];context:[string]}>()
const sources=ref<Source[]>([]),saved=ref<Saved|null>(null),title=ref('Supplementary bundle 1'),ids=ref<string[]>([]),settings=ref<LayoutSettings|null>(null),snapshot=ref(''),loaded=ref(false),loading=ref(false),saving=ref(false),error=ref(''),mapError=ref(''),plan=ref<any>(null)
const mode=ref<'plan'|'membership'|'numbering'|'map'>('plan'),draftIds=ref<string[]>([]),draftSettings=ref<LayoutSettings|null>(null)
const baselineId=computed(()=>saved.value?.baseline_build_id||props.baseline.id)
const signature=()=>JSON.stringify({title:title.value,ids:ids.value,settings:settings.value})
const clean=ref(''),dirty=computed(()=>loaded.value&&(signature()!==clean.value||mode.value==='membership'&&JSON.stringify(draftIds.value)!==JSON.stringify(ids.value)||mode.value==='numbering'&&JSON.stringify(draftSettings.value)!==JSON.stringify(settings.value)))
watch(dirty,v=>emit('dirtyChanged',v))
watch(()=>[title.value,ids.value.length],()=>emit('context',`${title.value} · ${ids.value.length} selected source pages`),{immediate:true})
const missing=computed(()=>ids.value.filter(id=>!sources.value.some(d=>d.pages.some(p=>p.id===id))))
const selectedGroups=computed(()=>sources.value.map(d=>({...d,selected:d.pages.filter(p=>ids.value.includes(p.id))})).filter(d=>d.selected.length))
const titleInput=ref<HTMLInputElement|null>(null),pickerTrigger=ref<HTMLButtonElement|null>(null),numberTrigger=ref<HTMLButtonElement|null>(null),prepareTrigger=ref<HTMLButtonElement|null>(null)
let loadSequence=0,mapSequence=0,saveSequence=0,timer:ReturnType<typeof setTimeout>|undefined
const body=()=>({baseline_build_id:baselineId.value,title:title.value,selected_page_ids:ids.value,...settings.value,expected_snapshot:snapshot.value,expected_version:saved.value?.version})
async function load(preserve=false){
 const call=++loadSequence;loading.value=true;error.value='';plan.value=null;++mapSequence
 try{
  const data=await api<Catalog>(`/matters/${props.matterId}/supplements`)
  if(call!==loadSequence)return
  sources.value=data.documents;snapshot.value=data.input_snapshot
  if(!preserve){
   const existing=data.plans.find(p=>p.id===props.initialPlanId)
   if(props.initialPlanId&&!existing)throw new Error('Saved supplement is unavailable. Return and choose another saved plan.')
   saved.value=existing||null;title.value=existing?.title||'Supplementary bundle 1';ids.value=[...(existing?.selected_page_ids||[])];settings.value=structuredClone(existing?.settings||data.defaults)
   clean.value=signature();loaded.value=true
  }
  await refreshMap()
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
  saved.value=p;clean.value=signature();mode.value='plan';emit('saved',p.id);await refreshMap();return true
 }catch(e){error.value=friendlyError(e);return false}finally{saving.value=false}
}
function discard(){if(saved.value){title.value=saved.value.title;ids.value=[...saved.value.selected_page_ids];settings.value=JSON.parse(JSON.stringify(saved.value.settings))}else{ids.value=[];title.value='Supplementary bundle 1'}mode.value='plan';clean.value=signature();emit('dirtyChanged',false)}
function openPicker(){if(saving.value||mode.value==='membership')return;draftIds.value=[...ids.value];mode.value='membership';void focusHeading()}
function openNumbering(){if(saving.value)return;draftSettings.value=JSON.parse(JSON.stringify(settings.value));mode.value='numbering';void focusHeading()}
async function focusHeading(){await nextTick();document.querySelector<HTMLElement>('.supplement-workspace h1')?.focus()}
async function returnPlan(apply=false){const caller=mode.value;if(apply){if(caller==='membership')ids.value=[...draftIds.value];if(caller==='numbering')settings.value=JSON.parse(JSON.stringify(draftSettings.value))}mode.value='plan';await nextTick();(caller==='membership'?pickerTrigger:caller==='numbering'?numberTrigger:prepareTrigger).value?.focus()}
function toggle(id:string){draftIds.value=draftIds.value.includes(id)?draftIds.value.filter(p=>p!==id):[...draftIds.value,id]}
async function prepare(){if(!ids.value.length){error.value='Select at least one source page before preparing the map.';return}if(await saveDraft()){if(plan.value){mode.value='map';await focusHeading()}}}
const leaveDialog=ref<HTMLDialogElement|null>(null)
function requestClose(){if(dirty.value)leaveDialog.value?.showModal();else emit('close')}
defineExpose({saveDraft,discard,openPicker,openNumbering,prepare})
void load()
onBeforeUnmount(()=>{loadSequence++;mapSequence++;saveSequence++;clearTimeout(timer);emit('dirtyChanged',false)})
</script>
<template>
<section class="supplement-workspace" :data-mode="mode" @keydown.esc.stop.prevent="mode==='plan'?requestClose():returnPlan()">
 <header class="supplement-heading"><div><h1 tabindex="-1">{{mode==='plan'?'Create supplementary bundle':mode==='membership'?'Supplement membership':mode==='numbering'?'Numbering & navigation':'Supplement page map'}}</h1><p>{{mode==='plan'?'Choose new material. The selected earlier build remains unchanged.':`${title} · ${ids.length} selected source pages · baseline ${baselineId.slice(0,8)}`}}</p></div><button class="membership-button secondary" :disabled="saving" @click="mode==='plan'?requestClose():returnPlan()">{{mode==='plan'?'Cancel':'Cancel / return'}}</button></header>
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
  <aside class="supplement-card supplement-summary"><h2>Supplement preview</h2><h3>{{title||'Untitled supplement'}}</h3><dl><div><dt>Selected source pages</dt><dd>{{ids.length}}</dd></div><div><dt>Generated pages</dt><dd>{{plan?.generated_pages??'—'}}</dd></div><div><dt>Planned physical pages</dt><dd>{{plan?.combined_pages??'—'}}</dd></div><div><dt>Volumes</dt><dd>{{plan?.volumes.length??'—'}}</dd></div></dl><div class="supplement-notice"><strong><Info :size="16"/>Linked to build {{baselineId.slice(0,8)}}</strong><p>This plan is separate. It does not replace or renumber the earlier bundle.</p><p>Generation and insertion policies follow in B04B.</p></div></aside>
 </div>
 <p v-if="mapError" role="alert" class="supplement-error">{{mapError}} <button class="text-tool" @click="load(true)">Reload material; retain selection</button></p>
 <footer class="supplement-footer"><span>{{saved?`Plan ${saved.id.slice(0,8)} · v${saved.version}`:'New supplemental plan'}} · {{dirty?'Unsaved changes':saved?'Saved selection':'Not saved yet'}}<small>Review the new page map before building. Generation is unavailable in B04A.</small></span><button class="membership-button secondary" :disabled="saving||blocked" @click="saveDraft">{{saving?'Saving…':'Save plan'}}</button><button ref="prepareTrigger" class="membership-button primary" :disabled="saving||blocked||!ids.length||!!missing.length" @click="prepare">Prepare supplement</button></footer>
 <details class="supplement-provenance"><summary>Baseline identity and provenance</summary><p>Saved locally. Portable matter backups do not yet include supplemental plans or prior build outputs.</p><p>Build {{baselineId}} · {{prettyDate(baseline.created_at)}} · immutable saved output</p><p v-for="f in baseline.files" :key="f.name">{{f.name}} · {{f.bytes}} bytes · {{f.sha256||'Digest not recorded'}}</p></details>
 </template>
 <template v-else-if="mode==='membership'">
  <div class="membership-summary">{{draftIds.length}} selected source pages · Selection order determines supplemental order. Primary membership is unchanged.</div>
  <section class="supplement-card supplement-picker"><h2>Choose source pages</h2><p>IDs refer to an exact stored working revision. New revisions require explicit reselection.</p><div v-for="d in sources" :key="d.id" class="supplement-document"><h3>{{d.title}} · revision {{d.revision}}</h3><div class="supplement-page-options"><label v-for="p in d.pages" :key="p.id" :title="p.id"><input type="checkbox" :checked="draftIds.includes(p.id)" :disabled="saving||blocked" :aria-label="`${d.title} source page ${p.page_index+1}`" @change="toggle(p.id)"/>Source {{p.page_index+1}}</label></div></div><div v-for="id in draftIds.filter(id=>!sources.some(d=>d.pages.some(p=>p.id===id)))" :key="id"><span>Unavailable identity {{id}}</span><button class="text-tool" @click="toggle(id)">Remove unavailable page</button></div></section>
  <footer class="supplement-footer"><span>{{draftIds.length}} selected · {{ids.length}} in caller draft</span><button class="membership-button secondary" @click="returnPlan()">Cancel selection</button><button class="membership-button primary" :disabled="blocked" @click="returnPlan(true)">Apply selection</button></footer>
 </template>
 <template v-else-if="mode==='numbering'&&draftSettings">
  <div class="supplement-numbering-grid"><NumberingSettings v-model="draftSettings.output_marks" :disabled="blocked||saving"/><section class="supplement-card"><h2>Supplement settings</h2><p>{{ids.length}} exact source-page identities remain selected.</p><label>Front matter<select v-model="draftSettings.output_marks.front_matter"><option value="cover_index">Cover + generated index</option><option value="index_only">Generated index only</option></select></label><label>Maximum pages per volume<input v-model.number="draftSettings.max_pages_per_volume" type="number" min="2" max="2000"/></label><p>These settings belong to this supplemental plan. Earlier labels are unchanged. Preserve-reference insertion policies are unavailable until B04B.</p></section></div>
  <footer class="supplement-footer"><span>Apply returns to this supplement and invalidates its preview.</span><button class="membership-button secondary" @click="returnPlan()">Cancel numbering</button><button class="membership-button primary" :disabled="blocked" @click="returnPlan(true)">Apply numbering</button></footer>
 </template>
 <template v-else-if="mode==='map'">
  <div class="supplement-map-meta"><strong>{{ids.length}} selected source pages · {{plan?.generated_pages??'—'}} generated · {{plan?.combined_pages??'—'}} physical</strong><p>Plan {{saved?.id}} · baseline {{baselineId}}</p></div>
  <BundlePreview :matter-id="matterId" :max-pages="settings.max_pages_per_volume" :keep-groups="settings.keep_groups_intact" :refresh-key="refreshKey" :supplied-plan="plan" :external-plan="true" :inputs-pending="!plan||blocked"/>
  <p v-if="mapError" role="alert">{{mapError}}</p><footer class="supplement-footer"><button class="membership-button secondary" @click="returnPlan()">Back to supplement</button><button class="membership-button secondary" :disabled="blocked" @click="refreshMap">Recalculate selected map</button><button class="membership-button primary" disabled title="B04B adds supported supplemental generation and output checks">Build supplement · unavailable</button></footer><p>Selection and map validation are planning checks only. Artifact checks, review and export require a generated supplemental output in B04B.</p>
 </template>
 </template>
 <dialog @keydown.esc.stop.prevent="leaveDialog?.close()" ref="leaveDialog" class="build-tool-dialog" @cancel.prevent="leaveDialog?.close()"><h2>Keep this supplemental draft?</h2><p>Cancel returns without saving new changes. Any previously saved plan stays available.</p><div class="build-actions"><button class="membership-button secondary" @click="leaveDialog?.close()">Keep editing</button><button class="membership-button secondary" @click="discard();leaveDialog?.close();emit('close')">Discard changes</button><button class="membership-button primary" :disabled="saving||blocked" @click="saveDraft().then(ok=>{if(ok){leaveDialog?.close();emit('close')}})">Save and return</button></div><p v-if="error" role="alert">{{error}}</p></dialog>
</section>
</template>
