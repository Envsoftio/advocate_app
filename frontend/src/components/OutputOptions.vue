<script setup lang="ts">
import {computed,nextTick,onBeforeUnmount,onMounted,ref,watch} from 'vue'
import {Info,X} from 'lucide-vue-next'
import {api} from '../lib/api'
import {friendlyError} from '../lib/format'
import type {SavedLayout} from '../lib/bundleSettings'
const props=defineProps<{matterId:string;disabled:boolean;inputsDirty:boolean}>()
const emit=defineEmits<{close:[];modified:[];dirtyChanged:[value:boolean]}>()
const dialog=ref<HTMLDialogElement|null>(null),settings=ref<SavedLayout|null>(null),baseline=ref(''),error=ref(''),loading=ref(false),saving=ref(false),confirmClose=ref(false)
let alive=true,sequence=0
let returnFocus:HTMLElement|null=null
const dirty=computed(()=>!!settings.value&&JSON.stringify(settings.value)!==baseline.value)
const marks=computed(()=>settings.value?.output_marks)
watch(dirty,v=>emit('dirtyChanged',v))
async function load(){const call=++sequence;loading.value=true;error.value='';try{const result=await api<SavedLayout>(`/matters/${props.matterId}/bundle-settings`);if(alive&&call===sequence){settings.value=result;baseline.value=JSON.stringify(result)}}catch(e){if(alive&&call===sequence)error.value=friendlyError(e)}finally{if(alive&&call===sequence)loading.value=false}}
async function saveDraft(){if(!settings.value||saving.value||props.disabled||props.inputsDirty)return false;if(!dirty.value)return true;saving.value=true;error.value='';const d=JSON.parse(JSON.stringify(settings.value)) as SavedLayout;try{const result=await api<SavedLayout>(`/matters/${props.matterId}/bundle-settings`,'PUT',{max_pages_per_volume:d.max_pages_per_volume,keep_groups_intact:d.keep_groups_intact,output_marks:d.output_marks,expected_snapshot:d.input_snapshot});if(!alive)return false;settings.value=result;baseline.value=JSON.stringify(result);emit('dirtyChanged',false);emit('modified');return true}catch(e){if(alive)error.value=friendlyError(e);return false}finally{if(alive)saving.value=false}}
function discard(){if(baseline.value)settings.value=JSON.parse(baseline.value);confirmClose.value=false;emit('dirtyChanged',false)}
function close(){if(saving.value)return;if(dirty.value){returnFocus=document.activeElement as HTMLElement;confirmClose.value=true;void nextTick(()=>dialog.value?.querySelector<HTMLButtonElement>('.output-discard')?.focus())}else emit('close')}
function keepEditing(){confirmClose.value=false;void nextTick(()=>returnFocus?.focus())}
function keydown(event:KeyboardEvent){if((event.metaKey||event.ctrlKey)&&event.key.toLowerCase()==='s'){event.preventDefault();event.stopPropagation();void saveDraft()}}
async function apply(){if(await saveDraft())emit('close')}
onMounted(()=>{const positions:Array<[HTMLElement,number]>=[];let parent=dialog.value?.parentElement;while(parent){positions.push([parent,parent.scrollTop]);parent=parent.parentElement}dialog.value?.showModal();positions.forEach(([el,top])=>el.scrollTop=top);void load()})
onBeforeUnmount(()=>{alive=false;sequence++;dialog.value?.close();emit('dirtyChanged',false)})
defineExpose({saveDraft,discard})
</script>
<template>
 <Teleport to="body">
 <dialog ref="dialog" class="output-options" aria-labelledby="output-options-title" @cancel.prevent="close" @keydown="keydown">
  <header><h2 id="output-options-title">Output options</h2><button class="icon-btn" aria-label="Close output options" :disabled="saving" @click="close"><X :size="16"/></button></header>
  <p class="output-intro">Choose processing for the next generated bundle. Imported originals stay preserved.</p>
  <div class="output-scroll">
   <p v-if="loading" role="status">Loading saved options…</p>
   <div v-if="error" class="output-error" role="alert">{{error}} <button v-if="!settings" class="text-tool" @click="load">Retry loading</button><button v-else class="text-tool" @click="discard();load()">Reload saved options</button></div>
   <p v-if="inputsDirty" role="status">Save pending volume settings before applying these options.</p>
   <div v-if="marks" class="output-columns">
    <fieldset class="output-controls" :disabled="saving || disabled || loading">
     <label class="output-field">Page quality<select aria-label="Page quality" v-model="marks.processing_quality" aria-describedby="quality-note"><option value="original">Original quality</option><option value="lossless">Lossless optimization · no image reduction</option><option value="balanced">Balanced images · 240 dpi / JPEG 85</option><option value="small">Smaller images · 150 dpi / JPEG 70</option></select></label>
     <p id="quality-note" class="output-caption quality-note">{{marks.processing_quality==='balanced'||marks.processing_quality==='small'?'Lossy image reduction; inspect scans and signatures. No target size is guaranteed.':'No image downsampling. Size is measured after generation.'}}</p>
     <h3>Cleanup before export</h3>
     <label class="output-check"><input type="checkbox" v-model="marks.cleanup_metadata"/>Remove author and editing metadata <span title="Clears PDF document information and XMP only; visible page text is unchanged.">ⓘ</span></label>
     <label class="output-check fixed-policy"><input type="checkbox" checked disabled aria-describedby="fixed-policy"/>Remove private comments <small>Always omitted</small></label>
     <label class="output-check fixed-policy"><input type="checkbox" checked disabled aria-describedby="fixed-policy"/>Remove source attachments and actions <small>Always omitted</small></label>
     <label class="output-field print-field">Print layout<select aria-label="Print layout" v-model="marks.print_layout" aria-describedby="print-note"><option value="screen">Screen output · no added blank pages</option><option disabled>Double-sided document starts · unavailable</option></select></label>
     <h3 class="office-title">Office review drafts</h3>
     <div class="output-office"><label class="output-check"><input type="checkbox" v-model="marks.office_docx"/>DOCX</label><label class="output-check"><input type="checkbox" v-model="marks.office_xlsx"/>XLSX</label><label class="output-check"><input type="checkbox" v-model="marks.office_pptx"/>PPTX</label></div>
     <p class="output-caption">Text drafts / index workbook; layout and images differ.</p>
    </fieldset>
    <section class="output-preview">
     <h3>Output preview</h3><span class="output-badge"><Info :size="14"/>Screen output</span>
     <dl><div><dt>Processing scope</dt><dd>Next build PDFs</dd></div><div><dt>Intentional print blanks</dt><dd>0</dd></div><div><dt>Generated navigation</dt><dd>Checked after processing</dd></div><div><dt>Existing searchable text</dt><dd>Checked for changes</dd></div><div><dt>Output size</dt><dd>Measured after build</dd></div></dl>
     <div class="output-review-note"><strong><Info :size="16"/>Review the result</strong><p>Inspect scan clarity and cleanup. Existing PDF marks and redactions follow the saved output policy; signatures are not verified.</p></div>
    </section>
   </div>
   <details v-if="marks" class="output-scope"><summary>Scope and unavailable options</summary><p id="fixed-policy">Private review notes and source PDF annotations, attachments and actions are already omitted by bundle assembly. Generated index links and explicitly reviewed links follow their saved settings. These fixed policies cannot be turned off here.</p><p id="print-note">Double-sided blank-page insertion is unavailable: the page map has no supported duplex rule. Print a completed PDF with the existing Print tool. Password-protected copies remain under More output tools.</p><p>Metadata cleanup clears PDF Info and XMP in new PDFs only, not visible text, filenames, matter fields, reports or Office draft properties. Office drafts are generated from this build’s combined PDF / CSV and included in its ZIP. A failure leaves prior builds available.</p></details>
  </div>
  <footer v-if="confirmClose" role="alertdialog" aria-label="Unsaved output options"><span>Discard unapplied options?</span><button class="membership-button secondary" @click="keepEditing">Keep editing</button><button class="membership-button secondary output-discard" @click="discard();emit('close')">Discard changes</button><button class="membership-button primary" :disabled="saving||disabled||inputsDirty" @click="apply">Save changes</button></footer>
  <footer v-else><span role="status">{{saving?'Applying options…':dirty?'Unapplied changes':''}}</span><button class="membership-button secondary" :disabled="saving" @click="close">Cancel</button><button class="membership-button primary" :disabled="!settings||loading||saving||disabled||inputsDirty" @click="apply">{{error&&settings?'Retry options':'Apply options'}}</button></footer>
 </dialog>
 </Teleport>
</template>
