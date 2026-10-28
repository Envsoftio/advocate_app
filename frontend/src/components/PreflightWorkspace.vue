<script setup lang="ts">
import {computed,ref} from 'vue'
import {Info,TriangleAlert,CircleX} from 'lucide-vue-next'
import type {Check} from '../lib/types'
const props=defineProps<{checks:Check[];loading:boolean;error:string;pending:boolean;blocked:boolean;filter:string}>()
const emit=defineEmits<{run:[];finding:[check:Check];build:[];'update:filter':[value:string]}>()
const reviewOpen=ref(false)
const errors=computed(()=>props.checks.filter(c=>c.severity==='error'))
const warnings=computed(()=>props.checks.filter(c=>c.severity==='warning'))
const sections=computed(()=>[
 {key:'error',title:'ERRORS · MUST RESOLVE BEFORE THIS BUILD',rows:errors.value},
 {key:'warning',title:'WARNINGS · REVIEW RECOMMENDED',rows:warnings.value},
 {key:'info',title:'TECHNICAL NOTES',rows:props.checks.filter(c=>c.severity==='info')},
].filter(s=>props.filter==='all'||props.filter===s.key))
</script>
<template>
 <section class="preflight-workspace" aria-label="Current revision Preflight">
  <header class="build-heading"><div><h1 tabindex="-1">Preflight · current revision</h1><p>Current working revision · Review findings before rebuilding the bundle.</p></div><button class="membership-button primary" :disabled="blocked" @click="emit('run')">Run checks</button></header>
  <div class="preflight-summary"><div><span>Errors</span><strong class="qa-error">{{loading?'—':errors.length}}</strong></div><div><span>Warnings</span><strong>{{loading?'—':warnings.length}}</strong></div><div><span>Technical checks</span><strong>{{loading?'Running':error?'Unavailable':pending?'Saved inputs only':checks.length?'Input scan completed':'Not run'}}</strong></div><div><span>Human review</span><button class="text-tool" @click="reviewOpen=!reviewOpen">Open checklist</button></div></div>
  <p v-if="loading" role="status">Checking the current saved input snapshot…</p>
  <p v-if="error" role="alert" class="build-strip danger">{{error}}</p><p v-else-if="pending" role="status" class="build-strip warning">Inputs have pending changes. Save and run checks again.</p>
  <div class="preflight-findings"><div class="preflight-filter"><label>Show<select aria-label="Show" :value="filter" @change="emit('update:filter',($event.target as HTMLSelectElement).value)"><option value="all">All findings</option><option value="error">Errors</option><option value="warning">Warnings</option><option value="info">Technical notes</option></select></label><small>Checks describe saved inputs, not generated output.</small></div>
   <section v-for="section in sections" :key="section.key" :data-severity="section.key"><h2>{{section.title}}</h2><div v-for="(check,index) in section.rows" :key="check.finding_id||index" class="preflight-finding"><component :is="section.key==='error'?CircleX:section.key==='warning'?TriangleAlert:Info" :size="16"/><div><strong>{{check.code.toLowerCase().split('_').join(' ')}}</strong><p>{{check.message}}</p></div><button v-if="check.document_id" :id="`finding-${check.finding_id}`" class="membership-button quiet" :disabled="loading||blocked" @click="emit('finding',check)">{{check.page_index===undefined?'Review document':`Review source ${check.page_index+1}`}}</button></div><p v-if="!section.rows.length" class="preflight-no-findings">{{loading?'Checking current inputs…':'No '+section.key+' findings reported.'}}</p></section>
   <section class="preflight-human"><h2>HUMAN REVIEW</h2><p>Review completeness, document titles, sequence and pagination against your requirements.</p><button class="membership-button quiet" :aria-expanded="reviewOpen" @click="reviewOpen=!reviewOpen">{{reviewOpen?'Close checklist':'Open review checklist'}}</button><div v-show="reviewOpen"><slot/></div></section>
  </div>
  <footer class="build-footer"><span>Passing technical checks does not mean the filing is legally complete or accepted by a court.</span><button class="membership-button primary" :disabled="blocked||loading||!!error||!!errors.length||pending" @click="emit('build')">Build settings</button></footer>
 </section>
</template>
