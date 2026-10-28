<script setup lang="ts">
import {computed, onMounted, onBeforeUnmount, ref, watch} from 'vue'
import {Info} from 'lucide-vue-next'
import EngineNotice from './EngineNotice.vue'
import {api, ApiError} from '../lib/api'
import type {BundleMembershipState} from '../lib/types'
const props=defineProps<{matterId:string; blocked:boolean}>()
const emit=defineEmits<{applied:[]; close:[]; dirty:[boolean]; open:[string]; retry:[]; review:[]}>()
const state=ref<BundleMembershipState|null>(null), selected=ref<string[]>([])
const filter=ref<'all'|'included'|'reference'>('all')
const loading=ref(true), saving=ref(false), error=ref(''), stale=ref(false)
const heading=ref<HTMLElement|null>(null)
let sequence=0
const dirty=computed(()=>!!state.value && state.value.documents.some(d=>!!d.bundle_included!==selected.value.includes(d.id)))
watch(dirty,value=>emit('dirty',value))
const documents=computed(()=>state.value?.documents||[])
const included=computed(()=>documents.value.filter(d=>selected.value.includes(d.id)))
const pages=computed(()=>included.value.reduce((sum,d)=>sum+d.page_count,0))
const referencePages=computed(()=>documents.value.reduce((sum,d)=>sum+d.page_count,0)-pages.value)
const visible=computed(()=>documents.value.filter(d=>filter.value==='all'||selected.value.includes(d.id)===(filter.value==='included')))
async function load(){
 const call=++sequence; loading.value=true;error.value=''
 try{
  const result=await api<BundleMembershipState>(`/matters/${props.matterId}/membership`)
  if(call!==sequence)return
  state.value=result; selected.value=result.documents.filter(d=>d.bundle_included).map(d=>d.id);stale.value=false
 }catch(e){if(call===sequence)error.value=e instanceof Error?e.message:String(e)}
 finally{if(call===sequence)loading.value=false}
}
function toggle(id:string){selected.value=selected.value.includes(id)?selected.value.filter(i=>i!==id):[...selected.value,id]}
async function apply(returnToCaller=true){
 if(!state.value||saving.value||props.blocked||stale.value)return false
 saving.value=true;error.value=''
 try{
  const result=await api<BundleMembershipState>(`/matters/${props.matterId}/membership`,'PUT',{included_ids:selected.value,expected_snapshot:state.value.input_snapshot})
  state.value=result;selected.value=result.documents.filter(d=>d.bundle_included).map(d=>d.id)
  emit('dirty',false);emit('applied');saving.value=false;if(returnToCaller)emit('close');return true
 }catch(e){error.value=e instanceof Error?e.message:String(e);stale.value=e instanceof ApiError&&e.status===409;return false}
 finally{saving.value=false}
}
function discard(){if(state.value)selected.value=state.value.documents.filter(d=>d.bundle_included).map(d=>d.id)}
onMounted(()=>{void load();heading.value?.focus({preventScroll:true})})
onBeforeUnmount(()=>{sequence++})
defineExpose({apply,discard,hasPendingDraft:()=>dirty.value,isSaving:()=>saving.value})
</script>
<template>
 <section class="bundle-membership" aria-labelledby="membership-title" :aria-busy="loading||saving" @keydown.esc.stop.prevent="!saving && emit('close')">
  <EngineNotice @retry="emit('retry')" @review="emit('review')"/>
  <header class="membership-heading">
   <h1 id="membership-title" ref="heading" tabindex="-1">Bundle membership</h1>
   <button class="membership-button primary membership-apply" :disabled="loading||saving||blocked||stale||!state" @click="apply()">{{saving?'Applying…':'Apply membership'}}</button>
   <button v-if="dirty" class="membership-return membership-cancel" :disabled="saving" @click="emit('close')">Cancel changes</button>
   <p aria-live="polite">{{documents.length}} matter documents · {{included.length}} included / {{pages}} source pages · {{documents.length-included.length}} reference-only / {{referencePages}} source pages</p>
  </header>
  <div class="membership-filters" aria-label="Membership filters">
   <button class="membership-button quiet" :class="{primary:filter==='all'}" :aria-pressed="filter==='all'" @click="filter='all'">All documents · {{documents.length}}</button>
   <button class="membership-button quiet" :class="{primary:filter==='included'}" :aria-pressed="filter==='included'" @click="filter='included'">Included · {{included.length}}</button>
   <button class="membership-button quiet" :class="{primary:filter==='reference'}" :aria-pressed="filter==='reference'" @click="filter='reference'">Reference only · {{documents.length-included.length}}</button>
   <span class="membership-secondary-actions">
    <button class="membership-button secondary" disabled title="Reviewed title and date suggestions are not implemented in this phase">Title &amp; date suggestions</button>
    <button class="membership-button secondary" disabled title="This matter currently has one primary bundle; variant review is a later phase">Review variants</button>
   </span>
  </div>
  <div v-if="error" class="membership-message error" role="alert">{{error}} Your selection has been retained. <button v-if="stale || !state" class="membership-button secondary" :disabled="saving||blocked" @click="load">{{stale?'Discard draft and reload':'Retry loading'}}</button></div>
  <div v-if="loading" class="membership-message" role="status">Loading bundle membership…</div>
  <section v-else-if="state" class="membership-card" aria-label="Bundle membership selection">
   <h2>{{state.title}}</h2>
   <p class="membership-showing">Showing {{visible.length}} of {{documents.length}} documents. Inclusion is specific to this bundle variant.</p>
   <div class="membership-table-scroll" tabindex="0" aria-label="Scrollable membership documents">
    <table class="membership-table">
     <colgroup><col class="include-col"><col class="document-col"><col class="group-col"><col class="pages-col"><col class="membership-col"><col class="review-col"></colgroup>
     <thead><tr><th scope="col">Include</th><th scope="col">Document</th><th scope="col">Group</th><th scope="col">Source pages</th><th scope="col">Membership</th><th scope="col">Review</th></tr></thead>
     <tbody><tr v-for="doc in visible" :key="doc.id">
      <td><input type="checkbox" :aria-label="`Include ${doc.title}`" :checked="selected.includes(doc.id)" :disabled="saving||blocked||stale" @change="toggle(doc.id)"></td>
      <td><button class="membership-document" :title="`${doc.title} · Working revision ${doc.revision} · Open document`" @click="emit('open',doc.id)">{{doc.title}}</button></td>
      <td>{{selected.includes(doc.id)?`Part ${doc.part}${doc.group_key?' · '+doc.group_key:''}`:'Reference material'}}</td>
      <td>{{doc.page_count}}</td><td>{{selected.includes(doc.id)?'Included':'Reference only'}}</td>
      <td>{{selected.includes(doc.id)?'Not checked':'Not exported'}}</td>
     </tr></tbody>
    </table>
    <p v-if="!visible.length" class="membership-empty">{{documents.length?'No documents match this filter.':'No documents yet. Import documents from Documents; new imports start as reference-only.'}}</p>
   </div>
   <p class="membership-explanation"><Info :size="16"/> Excluding keeps the source and revisions. Related bookmarks and links must be checked again.</p>
  </section>
  <div v-if="!included.length" class="membership-draft-actions">
   <span role="status">{{dirty?'Unapplied membership changes':state?`Membership revision ${state.revision}`:''}}<template v-if="state&&!included.length"> · No included pages; building is blocked.</template></span>
   <button class="membership-return" :disabled="saving" @click="emit('close')">Return without changes</button>
  </div>
 </section>
</template>
