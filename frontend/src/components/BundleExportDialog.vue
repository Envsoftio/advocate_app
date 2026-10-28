<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref } from 'vue'
import {useOutputValidation} from '../lib/outputValidation'
import {computed} from 'vue'
import { X, Download } from 'lucide-vue-next'
import { downloadExport, inTauri } from '../lib/api'
import { bytesLabel, friendlyError } from '../lib/format'
import type { ExportManifest } from '../lib/types'
const props=defineProps<{matterId:string;artifact:ExportManifest;filename?:string}>()
const emit=defineEmits<{close:[]}>()
const dialog=ref<HTMLDialogElement|null>(null)
const selected=ref(props.filename || props.artifact.files.find(f=>f.name.endsWith('.zip'))?.name || props.artifact.files[0]?.name || '')
const {state:qa,pending:qaPending,error:qaError}=useOutputValidation(()=>[props.matterId,props.artifact.id])
const release=ref(false)
const allowed=computed(()=>!!qa.value&&(release.value?qa.value.can_release:qa.value.can_download))
const pending=ref(false), message=ref(''), error=ref('')
let returnFocus:HTMLElement|null=null
async function save(){
 if(pending.value||!allowed.value)return
 pending.value=true;message.value='';error.value=''
 const name=selected.value
 try{
  const saved=await downloadExport(props.matterId,props.artifact.id,name,true,release.value)
  message.value=saved ? (inTauri()?`${name} saved.`:`Download requested for ${name}. Check your browser downloads.`) : 'Save cancelled. The generated build is unchanged.'
 }catch(err){error.value=friendlyError(err)}finally{pending.value=false}
}
function close(){if(!pending.value)emit('close')}
onMounted(()=>{returnFocus=document.activeElement as HTMLElement;dialog.value?.showModal()})
onBeforeUnmount(()=>{dialog.value?.close();void nextTick(()=>returnFocus?.isConnected&&returnFocus.focus())})
</script>
<template>
 <dialog ref="dialog" class="bundle-export-dialog" aria-labelledby="bundle-export-title" @cancel.prevent="close">
  <header><h2 id="bundle-export-title">Export bundle files</h2><button class="icon-btn" :disabled="pending" aria-label="Close export" @click="close"><X :size="18"/></button></header>
  <div class="build-badges"><span>Build {{artifact.id.slice(0,8)}} · {{artifact.stale?'Stale':'Saved'}}</span><span class="warning">{{qa?.can_release?'Human review recorded':'Human review pending'}}</span></div>
  <label>Destination<div class="export-destination">{{inTauri()?'Choose a location in the system Save dialog':'Browser downloads · location managed by your browser'}}</div></label>
  <label>File name<select v-model="selected" :disabled="pending"><option v-for="file in artifact.files" :value="file.name" :key="file.name">{{file.name}} · {{bytesLabel(file.bytes)}}</option></select></label>
  <p class="export-package-info">{{selected.endsWith('.zip')?'Includes the saved PDFs, CSV index and technical report.':'Exports the selected file from this saved build.'}}</p>
  <p>Exports a copy of build {{artifact.id.slice(0,8)}}. Later matter edits cannot change these files. A review copy is not released for filing. Release requires current inputs, technical checks and all four human-review items.</p>
  <label class="release-choice"><input type="checkbox" v-model="release" :disabled="pending"/> Release reviewed output</label>
  <p v-if="qaPending" role="status">Loading release checks…</p><p v-if="qaError" role="alert">{{qaError}}</p><p v-if="qa&&!allowed" class="build-strip warning">{{(release?qa.release_reasons:qa.download_reasons).join(' · ')}}</p>
  <p v-if="artifact.stale" class="build-strip warning">Previous output — current preparation has changed.</p>
  <p v-if="error" role="alert" class="build-strip danger">{{error}}</p><p role="status">{{pending?'Waiting for export…':message}}</p>
  <footer><button class="membership-button secondary" :disabled="pending" @click="close">{{message?'Close':'Cancel'}}</button><button class="membership-button primary" :disabled="pending || !selected || qaPending || !allowed" @click="save"><Download :size="16"/>{{pending?'Exporting…':error?'Retry export':'Export files'}}</button></footer>
 </dialog>
</template>
