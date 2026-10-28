<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { ArrowLeft, ChevronLeft, ChevronRight, Scan, MoveHorizontal, File, Rows2 } from 'lucide-vue-next'
import { nextZoom, type ReaderMode } from '../lib/pdfReader'
import PdfShortcutHelp from './PdfShortcutHelp.vue'
const props = defineProps<{compact?:boolean,page:number,count:number,zoom:string,scale:number,mode:ReaderMode,busy?:boolean,canBack?:boolean,pageLabel?:string,editingTools?:boolean}>()
const emit = defineEmits<{'update:page':[number], 'update:zoom':[string], 'update:mode':[ReaderMode], back:[]}>()
const percent = computed(() => Math.round(props.scale * 72 / 96 * 100))
const pageDraft=ref(String(props.page))
const enteringPage=ref(false)
watch(()=>props.page,page=>{if(!enteringPage.value)pageDraft.value=String(page)})
function jump(event:Event) {
  const input = event.target as HTMLInputElement
  const number = Math.floor(Number(input.value))
  if (Number.isFinite(number)) {
    const target=Math.max(1,Math.min(props.count,number))
    pageDraft.value=String(target);input.value=pageDraft.value
    enteringPage.value=false
    emit('update:page',target)
  } else {pageDraft.value=String(props.page);input.value=pageDraft.value;enteringPage.value=false}
}
</script>
<template>
  <div class="reader-navigation" role="group" aria-label="PDF navigation">

    <button class="icon-btn small" aria-label="Previous page" title="Previous page" :disabled="page<=1 || busy" @click="emit('update:page',page-1)"><ChevronLeft :size="17"/></button>
    <span class="page-counter"><span class="source-label">{{pageLabel && !pageLabel.toLowerCase().includes('source') ? 'PDF' : 'Source'}}</span><input :value="pageDraft" type="number" min="1" :max="count" :aria-label="pageLabel || 'Go to page'" :disabled="busy" @focus="enteringPage=true" @input="pageDraft=($event.target as HTMLInputElement).value" @change="jump" @blur="enteringPage=false;pageDraft=String(page)" @keydown.enter.prevent="jump"/> / {{count}}</span>
    <button class="icon-btn small" aria-label="Next page" title="Next page" :disabled="page>=count || busy" @click="emit('update:page',page+1)"><ChevronRight :size="17"/></button>

    <button class="icon-btn small" aria-label="Zoom out" title="Zoom out (Ctrl/⌘−)" :disabled="zoom==='25'" @click="emit('update:zoom',nextZoom(zoom,scale,-1))">−</button>
    <select :value="zoom" class="zoom-select" aria-label="Zoom" @change="emit('update:zoom',($event.target as HTMLSelectElement).value)"><option value="fit-width">{{zoom==='fit-width'?percent+'%':'Fit width'}}</option><option value="fit-page">{{zoom==='fit-page'?percent+'%':'Fit page'}}</option><option v-for="pct in [25,50,75,100,125,150,200,300,400]" :key="pct" :value="String(pct)">{{pct}}%{{pct===100?' · Actual size':''}}</option></select>
    <button class="icon-btn small" aria-label="Zoom in" title="Zoom in (Ctrl/⌘+)" :disabled="zoom==='400'" @click="emit('update:zoom',nextZoom(zoom,scale,1))">+</button>

    <button class="icon-btn small" aria-label="Fit width" title="Fit width" @click="emit('update:zoom','fit-width')"><MoveHorizontal :size="16"/></button>
    <button class="icon-btn small" aria-label="Fit page" title="Fit page" @click="emit('update:zoom','fit-page')"><Scan :size="16"/></button>

    <div v-if="!compact" class="reader-mode-switch" role="group" aria-label="Page display"><button class="btn secondary small" :class="{active:mode==='single'}" :aria-pressed="mode==='single'" @click="emit('update:mode','single')"><File :size="15"/> Single page</button><button class="btn secondary small" :class="{active:mode==='continuous'}" :aria-pressed="mode==='continuous'" @click="emit('update:mode','continuous')"><Rows2 :size="15"/> Continuous</button></div>
    <slot name="actions"/>
    <details class="reader-view-menu" @keydown.esc.stop.prevent="($event.currentTarget as HTMLDetailsElement).open=false; ($event.currentTarget as HTMLElement).querySelector('summary')?.focus()"><summary role="button" aria-label="View options">View</summary><div class="reader-view-options" @change="($event.currentTarget as HTMLElement).closest('details')?.removeAttribute('open')"><label>Reading mode     <select class="reading-mode-select" :value="mode" aria-label="Reading mode" @change="emit('update:mode',($event.target as HTMLSelectElement).value as ReaderMode)"><option value="single">Single page</option><option value="continuous">Continuous</option></select></label>    <button class="icon-btn small" aria-label="Back to previous reading position" title="Back to previous reading position" :disabled="!canBack || busy" @click="emit('back')"><ArrowLeft :size="16"/></button><PdfShortcutHelp :editing-tools="editingTools"/></div></details>
  </div>
</template>
<style scoped>
.reader-navigation{display:flex;align-items:center;gap:5px;flex-wrap:nowrap;overflow-x:auto;min-width:0;padding:7px 10px;border-top:1px solid var(--border);background:var(--panel)}
.reader-navigation>*{flex-shrink:0}
.reader-navigation select{height:36px;min-height:36px;border:1px solid var(--border);border-radius:6px;background:var(--panel);color:var(--text);font-size:12px;padding:0 7px;max-width:160px}
.reader-navigation .page-counter{white-space:nowrap}.reader-scale{font-size:11px;color:var(--muted);margin-left:3px}
</style>
