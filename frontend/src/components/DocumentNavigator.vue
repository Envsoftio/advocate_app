<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { ChevronDown, ChevronRight, FileText, GripVertical, ListFilter, Search, ArrowDownWideNarrow, PanelLeftClose, Plus, Ellipsis } from 'lucide-vue-next'
import { VueDraggable } from 'vue-draggable-plus'
import type { MatterDocument } from '../lib/types'

const props = defineProps<{ documents:MatterDocument[], selectedId:string|null, busy:boolean }>()
const emit = defineEmits<{ select:[string], open:[string], reorder:[MatterDocument[]],  import:[], table:[], cover:[], searchPages:[], collapse:[] }>()
const query = ref('')
const part = ref(0)
const expanded = ref(new Set([1]))
watch(()=>props.selectedId,id=>{
  const selected=props.documents.find(doc=>doc.id===id)
  if(selected) expanded.value=new Set([...expanded.value,selected.part])
},{immediate:true})
const groups = computed(()=>[1,2,3,4].filter(value=>!part.value || value===part.value).map(value=>({
  part:value, documents:props.documents.filter(doc=>doc.part===value && `${doc.title} ${doc.label} ${doc.kind}`.toLowerCase().includes(query.value.toLowerCase()))
})))
const totalPages=computed(()=>props.documents.reduce((sum,doc)=>sum+doc.page_count,0))
function toggle(value:number){ const next=new Set(expanded.value);next.has(value)?next.delete(value):next.add(value);expanded.value=next }
function move(id:string,direction:number) {
  const document=props.documents.find(doc=>doc.id===id)
  if(!document)return
  const ordered=props.documents.filter(doc=>doc.part===document.part)
  const index=ordered.findIndex(doc=>doc.id===id), target=index+direction
  if(target<0 || target>=ordered.length)return
  ;[ordered[index],ordered[target]]=[ordered[target],ordered[index]]
  reorder(document.part,ordered)
}
function reorder(value:number,ordered:MatterDocument[]){
  if(query.value || props.busy)return
  let index=0
  // Keep all other Parts in their existing positions in the deterministic order.
  emit('reorder',props.documents.map(doc=>doc.part===value?ordered[index++]:doc))
}
</script>
<template>
  <section class="document-navigator" aria-label="Matter documents" @keydown.esc.stop.prevent="emit('collapse')">
    <header><h2>Documents</h2><span>{{documents.length}}</span><button class="icon-btn small" aria-label="Collapse documents" title="Collapse documents" @click="emit('collapse')"><PanelLeftClose :size="16"/></button></header>
    <label class="navigator-search"><Search :size="16"/><input v-model="query" aria-label="Find a document" placeholder="Find a document"/></label>
    <div class="navigator-filters"><label><ListFilter :size="16"/><select v-model.number="part" aria-label="Filter documents by Part"><option :value="0">All Parts</option><option v-for="value in 4" :key="value" :value="value">Part {{value}}</option></select></label><button class="btn secondary small" title="Review a sort preview in the document table" @click="emit('table')"><ArrowDownWideNarrow :size="16"/> Sort</button></div>
    <div class="navigator-groups">
      <section v-for="group in groups" :key="group.part" class="navigator-part">
        <button class="navigator-part-heading" :aria-expanded="expanded.has(group.part) || !!query" @click="toggle(group.part)"><component :is="expanded.has(group.part)||query?ChevronDown:ChevronRight" :size="14"/><span>Part {{group.part}}<small>{{group.documents.length}} documents</small></span></button>
        <VueDraggable v-if="expanded.has(group.part) || query" :model-value="group.documents" handle=".navigator-grip" :disabled="busy || !!query" :animation="150" @update:model-value="reorder(group.part,$event)">
          <div v-for="doc in group.documents" :key="doc.id" class="navigator-document" :class="{selected:doc.id===selectedId}">
            <button class="icon-btn navigator-grip" :disabled="busy || !!query" :aria-label="`Reorder ${doc.title}. Use Up or Down.`" title="Drag or use Up / Down to reorder" @keydown.up.prevent="move(doc.id,-1)" @keydown.down.prevent="move(doc.id,1)"><GripVertical :size="13"/></button>
            <button class="navigator-document-select" :aria-current="doc.id===selectedId?'true':undefined" :title="doc.title" @click="emit('select',doc.id)" @dblclick="emit('open',doc.id)"><FileText :size="16"/><span><strong>{{doc.title}}</strong><small>{{doc.page_count}} source pages<span v-if="doc.label"> · {{doc.label}}</span><span v-if="doc.ocr_status==='needs_ocr'"> · OCR required</span></small></span></button>
          </div>
        </VueDraggable>
      </section>
      <p v-if="!documents.length" class="navigator-empty">Import a PDF or image to start preparing this matter.</p>
      <p v-else-if="!groups.some(group=>group.documents.length)" class="navigator-empty">No matching documents.</p>
    </div>
    <div class="navigator-actions"><button class="text-tool" :disabled="busy" @click="emit('import')"><Plus :size="15"/> Import documents</button><details><summary class="icon-btn small" role="button" aria-label="More document list actions"><Ellipsis :size="17"/></summary><div class="navigator-menu" @click="($event.currentTarget as HTMLElement).closest('details')?.removeAttribute('open')"><button @click="emit('table')">Open table</button><button :disabled="busy" @click="emit('cover')">Generate draft cover</button><button @click="emit('searchPages')">Search page text / archived files</button></div></details></div>
    <footer>{{documents.length}} documents · {{totalPages}} source pages</footer>
  </section>
</template>
