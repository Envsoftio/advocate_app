<script setup lang="ts">
import { computed } from 'vue'
import { Settings2, Files, ListTree, ListChecks, Box, Plus, ClipboardList, ArrowRight } from 'lucide-vue-next'
import type { Matter, MatterDocument, ExportManifest, ManualBookmark } from '../lib/types'
import { needsDocumentReview } from '../lib/matterFlow'
import { prettyDate } from '../lib/format'
const props=defineProps<{matter:Matter;documents:MatterDocument[];outputs:ExportManifest[];bookmarks:ManualBookmark[];profile:string;checkCount:number}>()
const emit=defineEmits<{navigate:[view:'documents'|'index'|'validation'|'build'|'storage'];import:[];details:[]}>()
const review=computed(()=>props.documents.filter(needsDocumentReview).length)
const unresolved=computed(()=>props.bookmarks.filter(b=>!b.resolved).length)
const latest=computed(()=>props.outputs[0])
const activity=computed(()=>[
 ...props.documents.map(d=>({id:d.id,time:d.updated_at,title:d.revision>1?'Working copy updated':'Document imported',detail:d.title})),
 ...props.outputs.map(e=>({id:e.id,time:e.created_at,title:'Bundle generated',detail:`${e.pages} pages · ${e.volumes} volumes`}))
].sort((a,b)=>b.time.localeCompare(a.time)).slice(0,3))
</script>
<template>
<section class="matter-overview flow-page" aria-label="Matter overview">
 <header class="flow-heading"><div><h1>{{matter.name}}</h1><p>{{profile}} · {{matter.case_number||'Draft matter'}} · {{matter.case_type}}</p></div><button class="btn secondary" @click="emit('details')"><Settings2 :size="15"/>Matter details</button></header>
 <div class="overview-metrics" aria-label="Matter summary"><span><strong>{{documents.length}}</strong>Documents</span><span><strong>{{documents.reduce((n,d)=>n+d.page_count,0)}}</strong>Source pages</span><span><strong>{{review}}</strong>Need review</span><span><strong>{{documents.filter(d=>d.ocr_status==='needs_ocr').length}}</strong>OCR pending</span><span><strong>{{unresolved}}</strong>Unresolved bookmarks</span></div>
 <div class="overview-review"><ClipboardList :size="22"/><div><h2>{{!documents.length?'Add your source documents':review?`${review} ${review===1?'document needs':'documents need'} review`:'Continue preparing this matter'}}</h2><p>Check document dates, classification and OCR before preparing the index.</p></div><button class="btn primary" @click="documents.length?emit('navigate','documents'):emit('import')"><ArrowRight :size="15"/>{{documents.length?'Continue document review':'Import documents'}}</button></div>
 <div class="overview-columns"><section class="overview-preparation"><div class="flow-section-heading"><h2>Preparation</h2><span>Working copies</span></div><div class="preparation-step"><Files :size="18"/><div><h3>Documents</h3><p>{{documents.length}} imported · {{review}} need review</p></div><button class="btn quiet" @click="emit('navigate','documents')">Review</button></div><div class="preparation-step"><ListTree :size="18"/><div><h3>Index &amp; bookmarks</h3><p>{{unresolved?`${unresolved} destinations need attention`:'Review bundle order and destinations'}}</p></div><button class="btn quiet" @click="emit('navigate','index')">Open index</button></div><div class="preparation-step"><ListChecks :size="18"/><div><h3>Technical checks</h3><p>{{checkCount}} current findings · Professional review required</p></div><button class="btn quiet" @click="emit('navigate','validation')">Run checks</button></div><div class="preparation-step"><Box :size="18"/><div><h3>Bundle</h3><p>{{latest?latest.stale?'Inputs changed · Rebuild required':`${latest.pages} pages · Review generated output`:'Not built'}}</p></div><button class="btn quiet" @click="emit('navigate','build')">Build settings</button></div></section><section class="overview-activity"><h2>Latest activity</h2><div v-for="item in activity" :key="item.id"><time>{{prettyDate(item.time)}}</time><span><strong>{{item.title}}</strong><small>{{item.detail}}</small></span></div><p v-if="!activity.length" class="flow-muted">No documents imported yet.</p></section></div>
 <footer class="flow-footer"><button class="btn secondary" @click="emit('import')"><Plus :size="15"/>Import documents</button><span class="flow-muted">Preparation and technical checks support your professional review.</span></footer>
</section>
</template>
