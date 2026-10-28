import {ref,watch,onBeforeUnmount} from 'vue'
import {api} from './api'
import {friendlyError} from './format'
export interface OutputFinding {finding_id:string;code:string;message:string;severity:'error'|'warning'|'info';document_id:string;target_revision:number;page_index:number;artifact:'working';output_file:string;output_page_index:number}
export interface OutputCheck {id:string;name:string;outcome:'passed'|'failed'|'warning'|'not-run'|'not-applicable';blocking:boolean;evidence:{summary:string;[key:string]:unknown};findings:OutputFinding[]}
export interface ValidationRun {id:string;created_at:string;build_id:string;input_digest:string;selected_files:string[];artifact_digest:string;checks:OutputCheck[];measured_files:Record<string,{pages?:number;bytes?:number;sha256?:string;error?:string}>}
export interface OutputValidation {build_id:string;run:ValidationRun|null;history:ValidationRun[];review:{version:number;artifact_digest:string;items:Record<string,boolean>;updated_at:string|null};review_labels:Record<string,string>;stale:boolean;can_download:boolean;can_release:boolean;download_reasons:string[];release_reasons:string[]}
// Navigation context is per immutable build, never shared across matters.
export const outputCheckContexts=new Map<string,{file:string;runId:string;findingId?:string}>()
export function useOutputValidation(identity:()=>[string,string],refresh:()=>unknown=()=>0){
 const state=ref<OutputValidation|null>(null),pending=ref(false),error=ref('')
 let sequence=0
 async function request(action='state',payload?:unknown){
  const [matter,build]=identity();const call=++sequence
  if(!build){state.value=null;pending.value=false;return}
  pending.value=true;error.value=''
  try{const result=await api<OutputValidation>(`/matters/${matter}/exports/${build}/validation/${action}`,action==='state'?'GET':action==='review'?'PUT':'POST',payload)
   if(call===sequence&&identity()[0]===matter&&identity()[1]===build)state.value=result
  }catch(e){if(call===sequence)error.value=friendlyError(e)}finally{if(call===sequence)pending.value=false}
 }
 watch(()=>[...identity(),refresh()],()=>{state.value=null;void request()},{immediate:true})
 onBeforeUnmount(()=>sequence++)
 async function review(key:string,value:boolean){if(!state.value||pending.value)return;await request('review',{...state.value.review,items:{...state.value.review.items,[key]:value}})}
 return {state,pending,error,reload:()=>request(),run:(files?:string[])=>request('run',files?{files}:{}),review}
}
