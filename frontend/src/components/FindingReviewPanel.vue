<script setup lang="ts">
import {ref,watch,onBeforeUnmount} from 'vue'
import {api} from '../lib/api'
import {friendlyError} from '../lib/format'
import type {Check} from '../lib/types'
const props=defineProps<{matterId:string;finding:Check;changed:boolean}>()
const emit=defineEmits<{return:[];tools:[]}>()
const note=ref(''),pending=ref(false),error=ref(''),saved=ref(false)
let sequence=0
const url=()=>`/matters/${props.matterId}/validation/findings/${props.finding.validation_run_id}/${props.finding.finding_id}`
watch(()=>[props.matterId,props.finding.finding_id],async()=>{const call=++sequence;note.value='';if(!props.finding.validation_run_id)return;try{const r=await api<{review:{note:string}}>(url());if(call===sequence)note.value=r.review.note}catch(e){if(call===sequence)error.value=friendlyError(e)}},{immediate:true})
async function save(){if(!props.finding.validation_run_id||pending.value)return;pending.value=true;const call=++sequence;try{await api(url(),'PUT',{note:note.value,acknowledged:true});if(call===sequence){saved.value=true;emit('return')}}catch(e){if(call===sequence)error.value=friendlyError(e)}finally{if(call===sequence)pending.value=false}}
onBeforeUnmount(()=>sequence++)
</script>
<template><aside class="finding-review-panel" aria-label="Preflight finding"><h2>Preflight finding</h2><span class="build-state">Review required</span><h3>{{finding.code.toLowerCase().split('_').join(' ')}}</h3><p>{{finding.page_index===undefined?'Document finding':`Source ${finding.page_index+1}`}} · Revision {{finding.target_revision ?? 'unavailable'}}</p><p>{{finding.message}}</p><p v-if="changed" role="status" class="qa-error">Target changed; run checks again. This finding cannot identify a page in the new revision.</p><label>Review note<textarea v-model="note" maxlength="2000" :disabled="pending||changed" @input="saved=false"/></label><button class="membership-button primary" :disabled="pending||changed||!finding.validation_run_id" @click="save">{{pending?'Saving…':'Keep page & return'}}</button><button class="membership-button secondary" :disabled="pending||changed" @click="emit('tools')">Open Page tools</button><button class="membership-button quiet" :disabled="pending" @click="emit('return')">Return to Preflight</button><p v-if="error" role="alert" class="qa-error">{{error}}</p><div class="finding-recheck"><strong>Checks must run again</strong><p>A review note does not replace technical checks. Recheck this revision before building.</p></div></aside></template>
