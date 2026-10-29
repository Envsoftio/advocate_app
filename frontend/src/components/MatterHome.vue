<script setup lang="ts">
import { computed, ref, watch, onUnmounted } from 'vue'
import { ChevronRight, Folder, Plus, Search, WandSparkles, List, Clock3 } from 'lucide-vue-next'
import type { Matter, MatterDocument, ExportManifest } from '../lib/types'
import { api } from '../lib/api'
import { prettyDate } from '../lib/format'
import { summarizeMatter } from '../lib/matterFlow'
const props=defineProps<{matters:Matter[];archived: Matter[];creating:boolean}>()
const emit=defineEmits<{open:[id:string];create:[];sample:[];restore:[id:string]}>()
const query=ref(''), sort=ref('recent'), scope=ref('active'), status=ref('all'), compact=ref(false)
const summaries=ref<Record<string,ReturnType<typeof summarizeMatter>>>({})
const failed=ref(false)
let sequence=0
async function refresh(){
 const call=++sequence;failed.value=false
 const result:typeof summaries.value={}
 for(const m of props.matters){
  try { const [d,e]=await Promise.all([api<MatterDocument[]>(`/matters/${m.id}/documents`),api<ExportManifest[]>(`/matters/${m.id}/exports`)]);result[m.id]=summarizeMatter(d,e) }
  catch {if(call===sequence)failed.value=true}
  if(call!==sequence)return
 }
 summaries.value=result
}
watch(()=>props.matters,refresh,{immediate:true})
onUnmounted(()=>sequence++)
const visible=computed(()=>(scope.value==='archived'?props.archived:props.matters).filter(m=>
 [m.name,m.case_number,m.case_type,m.petitioner,m.respondent].join(' ').toLowerCase().includes(query.value.trim().toLowerCase()) &&
 (status.value==='all'||summaries.value[m.id]?.package===status.value)
).sort((a,b)=>sort.value==='name'?a.name.localeCompare(b.name):b.updated_at.localeCompare(a.updated_at)))
const recent=computed(()=>[...props.matters].sort((a,b)=>b.updated_at.localeCompare(a.updated_at)).slice(0,3))
function focusSearch(){document.querySelector<HTMLInputElement>('[aria-label="Search matters"]')?.focus()}
function focusActivity(){document.querySelector<HTMLElement>('#matter-recent-activity')?.focus()}
defineExpose({focusSearch,focusActivity})
</script>
<template>
<main id="main-content" tabindex="-1" class="page-content matter-home flow-page">
 <header class="flow-heading"><div><h1>Matters</h1><p>Your matters, source documents and generated packages.</p></div><button class="btn primary" @click="emit('create')"><Plus :size="16"/>New matter</button></header>
 <div class="matter-filters"><label class="matter-search"><Search :size="16"/><input v-model="query" aria-label="Search matters" placeholder="Search matters, references or parties"/></label><select v-model="scope" aria-label="Filter matters"><option value="active">All matters</option><option value="archived">Archived matters</option></select><select v-model="status" aria-label="Filter by package status"><option value="all">All statuses</option><option>Not built</option><option>Current</option><option>Stale</option></select><span class="matter-count">{{visible.length}} {{visible.length===1?'matter':'matters'}}</span><select v-model="sort" aria-label="Sort matters"><option value="recent">Recently modified</option><option value="name">Matter name</option></select><button class="btn secondary" :aria-pressed="!compact" @click="compact=false"><List :size="15"/>List</button><button class="btn quiet" :aria-pressed="compact" @click="compact=true">Compact</button></div>
 <p v-if="failed" role="alert">Some matter counts could not be loaded. <button class="text-tool" @click="refresh">Retry</button></p>
 <div class="matter-table-scroll"><table class="flow-table matter-table" :class="{compact}"><thead><tr><th>Matter / forum</th><th>Preparation</th><th>Documents</th><th>Source pages</th><th>Modified</th><th>Latest package</th><th></th></tr></thead><tbody><tr v-for="m in visible" :key="m.id"><td><button class="matter-home-card" @click="scope==='archived'?emit('restore',m.id):emit('open',m.id)"><Folder :size="18"/><span><strong>{{m.name}}</strong><small>{{m.case_number||'Internal reference'}} · {{m.case_type||'Matter'}}</small></span></button></td><td><span class="flow-status" :data-status="summaries[m.id]?.review?'duplicate':'ready'">{{scope==='archived'?'Archived':summaries[m.id] ? summaries[m.id].review ? `${summaries[m.id].review} need review` : summaries[m.id].documents?'In preparation':'Add documents' : '—'}}</span></td><td>{{summaries[m.id]?.documents??'—'}}</td><td>{{summaries[m.id]?.pages??'—'}}</td><td>{{prettyDate(m.updated_at)}}</td><td><span class="flow-status" :data-status="summaries[m.id]?.package==='Stale'?'duplicate':'neutral'">{{summaries[m.id]?.package??'—'}}</span></td><td><button class="icon-btn" :aria-label="`${scope==='archived'?'Restore':'Open'} ${m.name}`" @click="scope==='archived'?emit('restore',m.id):emit('open',m.id)"><ChevronRight :size="16"/></button></td></tr></tbody></table></div>
 <div v-if="!visible.length" class="home-empty"><Folder :size="28"/><h2>{{query?'No matching matters':scope==='archived'?'No archived matters':'Start your first matter'}}</h2><p>{{query?'Try a different name, reference or party.':'Keep source documents and generated packages together.'}}</p><button v-if="!query&&scope==='active'" class="btn primary" @click="emit('create')">Create your first matter</button></div>
 <section id="matter-recent-activity" tabindex="-1" class="matter-recent"><div class="flow-section-heading"><h2>Recent activity</h2><span>Local history</span></div><div class="matter-activity"><button v-for="m in recent" :key="m.id" @click="emit('open',m.id)"><Clock3 :size="18"/><span><strong>Matter updated</strong><small>{{m.name}} · {{prettyDate(m.updated_at)}}</small></span></button><p v-if="!recent.length" class="flow-muted">Activity will appear as you prepare matters.</p></div></section>
 <div class="home-sample"><p class="flow-muted">Open a matter to continue preparation.</p><button class="btn secondary" :disabled="creating" @click="emit('sample')"><WandSparkles :size="15"/>Open sample matter</button></div>
</main>
</template>
