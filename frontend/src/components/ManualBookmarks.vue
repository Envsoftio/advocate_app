<script setup lang="ts">
import {computed,nextTick,onBeforeUnmount,ref,watch} from 'vue'
import {GripVertical,Bookmark as BookmarkIcon,Plus} from 'lucide-vue-next'
import {VueDraggable,type SortableEvent} from 'vue-draggable-plus'
import {api} from '../lib/api'
import {friendlyError} from '../lib/format'
import type {ManualBookmark as Bookmark,MatterDocument} from '../lib/types'
import type {LayoutSettings} from '../lib/bundleSettings'
import NumberPlacementPreview from './NumberPlacementPreview.vue'
const props=defineProps<{matterId:string;documents:MatterDocument[];refreshKey?:number;disabled:boolean;settings:LayoutSettings;preset?:{documentId:string;sequence:number}}>()
const emit=defineEmits<{modified:[];dirtyChanged:[value:boolean]}>()
const destination=ref<{printed_label:string;combined_page:number;volume:number}|null>(null)
const entries=ref<Bookmark[]>([]),saved=ref('[]'),selected=ref(''),error=ref(''),busy=ref(false),titleInput=ref<HTMLInputElement|null>(null)
const entry=computed(()=>entries.value.find(e=>e.id===selected.value))
const target=computed(()=>props.documents.find(d=>d.id===entry.value?.document_id))
const validTarget=computed(()=>!!entry.value&&!!target.value?.bundle_included&&entry.value.target_revision===target.value.revision&&entry.value.page_index>=0&&entry.value.page_index<target.value.page_count)
const dirty=computed(()=>JSON.stringify(serialise(entries.value))!==saved.value)
const parents=computed(()=>entries.value.slice(0,entries.value.findIndex(b=>b.id===selected.value)).filter(b=>b.id!==selected.value&&!descendantsOf(selected.value,b,new Map(entries.value.map(b=>[b.id,b])))))
const loading=ref(true),loaded=ref(false)
let pendingPreset:string|undefined
let snapshot=''
let bookmarkOrderBeforeDrag:Bookmark[]=[],sequence=0,alive=true
function serialise(values:Bookmark[]){return values.map(({id,title,document_id,page_index,target_revision,parent_id})=>({id,title,document_id,page_index,target_revision,parent_id}))}
watch(dirty,value=>emit('dirtyChanged',value),{immediate:true})
async function load(){
 if(dirty.value||busy.value)return
 const call=++sequence;loading.value=true;error.value=''
 try{
  const state=await api<{input_snapshot:string}>(`/matters/${props.matterId}/bundle-settings`)
  const value=await api<Bookmark[]>(`/matters/${props.matterId}/bookmarks`)
  if(!alive||call!==sequence||dirty.value)return
  snapshot=state.input_snapshot;entries.value=value;saved.value=JSON.stringify(serialise(value));loaded.value=true
  if(!entry.value)selected.value=value[0]?.id||''
 }catch(e){if(alive&&call===sequence)error.value=friendlyError(e)}
 finally{if(alive&&call===sequence){loading.value=false;if(loaded.value&&pendingPreset){const id=pendingPreset;pendingPreset=undefined;void add(id)}}}
}
watch(()=>[props.matterId,props.refreshKey],()=>{void load()},{immediate:true})
async function add(documentId?:string){if(!loaded.value||loading.value||busy.value)return;const doc=props.documents.find(d=>d.id===documentId)||props.documents.find(d=>d.bundle_included);if(!doc){error.value='Include a working document before adding a bookmark.';return}const b:Bookmark={id:crypto.randomUUID().replace(/-/g,''),title:documentId?doc.title:'New bookmark',document_id:doc.id,page_index:0,target_revision:doc.revision,parent_id:null,position:entries.value.length,resolved:true};entries.value.push(b);selected.value=b.id;await nextTick();titleInput.value?.focus();titleInput.value?.select()}
watch(()=>props.preset?.sequence,()=>{if(props.preset?.documentId){if(!loaded.value||loading.value)pendingPreset=props.preset.documentId;else void add(props.preset.documentId)}})
async function saveDraft(){if(!dirty.value)return true;if(busy.value||loading.value||!loaded.value||props.disabled)return false;if(entries.value.some(e=>!e.title.trim())){error.value='Every bookmark needs a label.';return false}busy.value=true;error.value='';try{const value=await api<Bookmark[]>(`/matters/${props.matterId}/bookmarks`,'PUT',{entries:serialise(entries.value),expected_snapshot:snapshot||undefined});if(!alive)return false;entries.value=value;saved.value=JSON.stringify(serialise(value));snapshot=(await api<{input_snapshot:string}>(`/matters/${props.matterId}/bundle-settings`)).input_snapshot;emit('modified');return true}catch(e){error.value=friendlyError(e);return false}finally{busy.value=false}}
function discard(){entries.value=JSON.parse(saved.value);if(!entry.value)selected.value=entries.value[0]?.id||'';error.value='';void load()}
function chooseDocument(id:string){if(!entry.value)return;entry.value.document_id=id;entry.value.page_index=0;entry.value.target_revision=props.documents.find(d=>d.id===id)?.revision||entry.value.target_revision;entry.value.resolved=true}
function choosePage(value:string){if(!entry.value)return;entry.value.page_index=Number(value)-1;confirmTarget()}
function confirmTarget(){if(!entry.value||!target.value?.bundle_included||!Number.isInteger(entry.value.page_index)||entry.value.page_index<0||entry.value.page_index>=target.value.page_count){error.value='Select an existing page in an included working document. No target was moved automatically.';return}entry.value.target_revision=target.value.revision;entry.value.resolved=true;error.value=''}
function remove(){if(!entry.value)return;const old=entry.value;entries.value=entries.value.filter(e=>e.id!==old.id).map(e=>e.parent_id===old.id?{...e,parent_id:old.parent_id}:e);selected.value=entries.value[0]?.id||''}
function depth(b:Bookmark){let level=0,parent=b.parent_id;const seen=new Set<string>();while(parent&&!seen.has(parent)){seen.add(parent);level++;parent=entries.value.find(e=>e.id===parent)?.parent_id||null}return level}
function generate(){if(!loaded.value||loading.value||busy.value)return;for(const doc of props.documents.filter(d=>d.bundle_included)){if(entries.value.some(b=>b.document_id===doc.id&&b.page_index===0))continue;entries.value.push({id:crypto.randomUUID().replace(/-/g,''),title:doc.title,document_id:doc.id,page_index:0,target_revision:doc.revision,parent_id:null,position:entries.value.length,resolved:true})}if(!entry.value)selected.value=entries.value[0]?.id||''}
function descendantsOf(id: string, candidate: Bookmark, byId: Map<string, Bookmark>): boolean {
  let parent = candidate.parent_id
  while (parent) {
    if (parent === id) return true
    parent = byId.get(parent)?.parent_id || null
  }
  return false
}
function siblingIndex(index: number, step: number): number {
  const entry = entries.value[index]
  if (!entry) return -1
  const siblings = entries.value.filter(item => item.parent_id === entry.parent_id)
  const position = siblings.findIndex(item => item.id === entry.id)
  return entries.value.findIndex(item => item.id === siblings[position + step]?.id)
}
function reorder(sourceId: string, targetId: string, after: boolean) {
  if (sourceId === targetId || busy.value || loading.value || props.disabled) return
  const source = entries.value.find(item => item.id === sourceId)
  const target = entries.value.find(item => item.id === targetId)
  if (!source || !target) return
  if (source.parent_id !== target.parent_id) {
    error.value = 'Drag bookmarks within the same level. Use Parent bookmark to change nesting.'
    return
  }
  const byId = new Map(entries.value.map(item => [item.id, item]))
  const block = entries.value.filter(item => item.id === sourceId || descendantsOf(sourceId, item, byId))
  const remaining = entries.value.filter(item => !block.includes(item))
  const targetBlock = remaining.filter(item => item.id === targetId || descendantsOf(targetId, item, byId))
  const position = remaining.findIndex(item => item.id === targetId) + (after ? targetBlock.length : 0)
  remaining.splice(position, 0, ...block)
  entries.value=remaining
}
function move(index: number, step: number) {
  const targetIndex = siblingIndex(index, step)
  if (targetIndex < 0) return
  reorder(entries.value[index].id, entries.value[targetIndex].id, step > 0)
}
function startBookmarkSort() {
  bookmarkOrderBeforeDrag = entries.value.map(entry => ({...entry}))
}
function endBookmarkSort(event: SortableEvent) {
  const before = bookmarkOrderBeforeDrag
  bookmarkOrderBeforeDrag = []
  if (!before.length) return
  const oldIndex = event.oldIndex
  const newIndex = event.newIndex
  const sourceId = event.item.dataset.bookmarkId ?? (oldIndex === undefined ? undefined : before[oldIndex]?.id)
  const targetId = newIndex === undefined ? undefined : before[newIndex]?.id
  entries.value = before
  if (oldIndex === undefined || newIndex === undefined || !sourceId || !targetId || sourceId === targetId) {
    entries.value = [...before]
    return
  }
  reorder(sourceId, targetId, newIndex > oldIndex)
}

defineExpose({saveDraft,discard})
onBeforeUnmount(()=>{alive=false;sequence++;emit('dirtyChanged',false)})
</script>
<template>
<section class="bookmark-workspace" aria-label="Manual bookmarks">
 <aside class="structure-card bookmark-tree"><h2>Bookmark tree</h2><button class="membership-button quiet" :disabled="busy||loading||!loaded||disabled" @click="add()"><Plus :size="16"/>Add bookmark</button>
 <p v-if="loading" role="status">Loading saved bookmarks…</p><VueDraggable v-model="entries" class="bookmark-tree-list" item-key="id" handle=".bookmark-drag-handle" :disabled="busy||loading||!loaded||disabled" :animation="160" :force-fallback="true" @start="startBookmarkSort" @end="endBookmarkSort">
 <div v-for="(b,index) in entries" :key="b.id" :data-bookmark-id="b.id" :class="['bookmark-tree-row',{'selected':selected===b.id}]" :style="{paddingLeft:8+depth(b)*18+'px'}"><span class="bookmark-drag-handle" role="button" :tabindex="busy||loading||!loaded||disabled?-1:0" :aria-label="`Reorder bookmark ${index+1}: ${b.title}. Press Up or Down arrow keys.`" @keydown.up.prevent="move(index,-1)" @keydown.down.prevent="move(index,1)"><GripVertical :size="14"/></span><button @click="selected=b.id"><BookmarkIcon :size="15"/><span>{{b.title}}</span><span v-if="!b.resolved" title="Needs resolution">!</span></button></div>
 </VueDraggable><p v-if="!entries.length">No manual bookmarks. Add one or generate from the current index.</p><footer><button class="membership-button quiet" :disabled="busy||loading||!loaded||disabled" @click="generate">Generate from index</button><span>{{entries.length}} bookmarks</span></footer></aside>
 <section class="structure-card bookmark-editor"><div class="bookmark-fields"><h2>{{entry?.title||'Select a bookmark'}}</h2><span :class="['map-state',validTarget?'current':'pending']">{{validTarget?'Destination selected':'Needs resolution'}}</span><fieldset v-if="entry" :disabled="busy||loading||!loaded||disabled"><label>Bookmark label<input ref="titleInput" v-model="entry.title" aria-label="Bookmark label" maxlength="200"/></label><label>Target document<select aria-label="Target document" :value="entry.document_id" @change="chooseDocument(($event.target as HTMLSelectElement).value)"><option v-if="!target" :value="entry.document_id">Missing or archived document</option><option v-for="d in documents" :key="d.id" :value="d.id" :disabled="!d.bundle_included">{{d.title}}{{d.bundle_included?'':' (reference only)'}}</option></select></label><label>Source document page<input :value="entry.page_index+1" type="number" min="1" :max="target?.page_count||1" @input="choosePage(($event.target as HTMLInputElement).value)"/></label><div class="bookmark-destination"><strong>GENERATED DESTINATION</strong><p v-if="destination">Printed {{destination.printed_label}}<br/>Combined PDF position {{destination.combined_page}}</p><p v-else>Destination pending current page map</p></div><button v-if="!validTarget" class="membership-button secondary" @click="confirmTarget">Confirm selected destination</button><details class="bookmark-hierarchy"><summary>Hierarchy &amp; deletion</summary><label>Parent bookmark<select aria-label="Parent bookmark" v-model="entry.parent_id"><option :value="null">Top level</option><option v-for="b in parents" :key="b.id" :value="b.id">{{b.title}}</option></select></label><button class="text-tool bookmark-remove" @click="remove">Delete selected bookmark</button></details><p>Index links, bookmarks and PDF labels share this map. Missing targets must be selected again.</p></fieldset><p v-if="error" role="alert" class="structure-error">{{error}} <button v-if="!loaded" class="text-tool" @click="load">Retry loading bookmarks</button></p></div>
 <NumberPlacementPreview v-if="entry" @placement="destination=$event" destination :matter-id="matterId" :settings="settings" :documents="documents" :refresh-key="refreshKey||0" :disabled="disabled" :document-id="entry.document_id" :source-page="entry.page_index+1" :expected-revision="entry.target_revision"/>
 </section><div class="bookmark-save"><span v-if="dirty">Unsaved bookmark changes</span><button v-if="dirty" class="membership-button secondary" :disabled="busy" @click="discard">Discard bookmark changes</button><button class="membership-button primary" :disabled="!dirty||busy||loading||!loaded||disabled" @click="saveDraft">{{busy?'Saving…':error?'Retry bookmark save':'Save bookmark'}}</button></div>
</section>
</template>
