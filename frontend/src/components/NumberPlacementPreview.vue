<script setup lang="ts">
import {computed,onBeforeUnmount,ref,watch} from 'vue'
import {api} from '../lib/api'
import {friendlyError} from '../lib/format'
import type {MatterDocument} from '../lib/types'
import type {LayoutSettings} from '../lib/bundleSettings'
const props=defineProps<{matterId:string;settings:LayoutSettings;documents:MatterDocument[];refreshKey:number;disabled?:boolean;documentId?:string;sourcePage?:number;expectedRevision?:number;destination?:boolean}>()
const emit=defineEmits<{placement:[value:Placement|null]}>()
const picker=ref(false),paper=ref<HTMLElement|null>(null)
function showPlacement(){if(paper.value)paper.value.scrollTop=props.settings.output_marks.numbering_position?.startsWith('top')?0:paper.value.scrollHeight}
const selected=ref(''),page=ref(1),loading=ref(false),error=ref('')
type Placement={image:string;document_title:string;source_pages:number;page_index:number;revision:number;volume:number;printed_label:string;pdf_label:string;physical_page:number;combined_page:number;combined_pages:number;bates:string|null}
const result=ref<Placement|null>(null)
watch(result,value=>emit('placement',value),{immediate:true})
const included=computed(()=>props.documents.filter(d=>d.bundle_included))
const doc=computed(()=>included.value.find(d=>d.id===(props.documentId??selected.value)))
let sequence=0,timer:ReturnType<typeof setTimeout>|undefined,alive=true
watch(included,()=>{if(!included.value.some(d=>d.id===selected.value))selected.value=included.value[0]?.id||''},{immediate:true})
watch(selected,()=>{page.value=1})
async function refresh(){const call=++sequence;result.value=null;error.value='';if(props.disabled||!doc.value){loading.value=false;return}loading.value=true;try{const value=await api<Placement>(`/matters/${props.matterId}/numbering-preview`,'POST',{max_pages_per_volume:props.settings.max_pages_per_volume,keep_groups_intact:props.settings.keep_groups_intact,output_marks:props.settings.output_marks,document_id:doc.value.id,page:props.sourcePage??page.value,expected_revision:props.expectedRevision??doc.value.revision});if(alive&&call===sequence)result.value=value}catch(e){if(alive&&call===sequence)error.value=friendlyError(e)}finally{if(alive&&call===sequence)loading.value=false}}
watch(()=>[props.matterId,JSON.stringify(props.settings),props.refreshKey,props.disabled,doc.value?.id,doc.value?.revision,props.expectedRevision,props.sourcePage,page.value],()=>{sequence++;result.value=null;error.value='';loading.value=!!doc.value&&!props.disabled;clearTimeout(timer);timer=setTimeout(refresh,220)},{immediate:true})
onBeforeUnmount(()=>{alive=false;sequence++;clearTimeout(timer)})
</script>
<template>
<section :class="['number-placement',{'destination-preview':destination}]" aria-label="Number placement preview">
 <header><h2>{{destination?'Destination preview':'Number placement preview'}}</h2><button v-if="!destination" class="text-tool" :aria-expanded="picker" @click="picker=!picker">Change page</button></header>
 <div v-if="!destination&&picker" class="placement-picker"><label>Document<select v-model="selected"><option v-for="d in included" :key="d.id" :value="d.id">{{d.title}}</option></select></label><label>Source page<input v-model.number="page" type="number" min="1" :max="doc?.page_count||1"/></label></div>
 <p class="placement-identity">{{doc?.title||'Select an included document'}}<template v-if="doc"> · Working revision {{doc.revision}} · Source {{sourcePage??page}} / {{doc.page_count}}</template></p>
 <div ref="paper" class="placement-paper"><p v-if="disabled" role="status">Preview pending accepted inputs.</p><p v-else-if="loading" role="status">Rendering current page…</p><p v-else-if="error" role="alert">{{error}} <button class="text-tool" @click="refresh">Retry preview</button></p><img v-else-if="result" @load="showPlacement" :src="result.image" :alt="`Actual working page ${result.page_index+1}, printed ${result.printed_label||'unstamped'}`"/></div>
 <template v-if="result&&!destination"><h2 class="placement-references">{{destination?'Generated destination':'One source, three references'}}</h2><dl><div><dt>Source identity</dt><dd>{{result.document_title}} · page {{result.page_index+1}} / {{result.source_pages}}</dd></div><div><dt>Printed / PDF label</dt><dd>{{result.printed_label||'No stamp'}}<template v-if="result.pdf_label!==result.printed_label"> / {{result.pdf_label||'No label'}}</template></dd></div><div><dt>Combined physical position</dt><dd>{{result.combined_page}} / {{result.combined_pages}} · Volume {{result.volume}}, page {{result.physical_page}}</dd></div></dl><p class="placement-note">{{result.bates?`Bates: ${result.bates}.`:'Bates is off.'}} Its identifier is separate from source identity and page labels.</p></template>
</section>
</template>
