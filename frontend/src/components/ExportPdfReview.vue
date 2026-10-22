<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import PdfReaderViewport from './PdfReaderViewport.vue'
import PdfReaderControls from './PdfReaderControls.vue'
import { exportPdfDocumentSource } from '../lib/api'
import { usePdfDocument, rememberReadingPosition, loadReadingPosition, flushReadingPositions, readingPositionError, nextZoom, type ReaderMode, type ReaderPosition } from '../lib/pdfReader'
import { usePdfSearch } from '../lib/pdfSearch'
import { readPdfSourceOutline, type PdfSourceOutlineEntry } from '../lib/pdfNavigation'

const props = defineProps<{matterId:string,exportId:string,filename:string,stale?:boolean}>()
const emit = defineEmits<{close:[],openExternalLink:[url:string]}>()
const dialog = ref<HTMLDialogElement|null>(null)
const findInput = ref<HTMLInputElement|null>(null)
const viewport = ref<InstanceType<typeof PdfReaderViewport>|null>(null)
const reader = usePdfDocument()
const {pdf,loading,error} = reader
const pageNumber = ref(1), scale = ref(1), zoom = ref('fit-page'), mode = ref<ReaderMode>('single')
const history = ref<ReaderPosition[]>([])
const outline = ref<Array<{title:string,page:number|null,url:string|null,level:number}>>([])
const outlineError = ref(''), renderError = ref('')
const query = ref('')
const {matches,selectedIndex,busy:searching,error:searchError,truncated,select,step} = usePdfSearch(pdf,query)
let generation=0, loadedKey=''
function remember(position?:ReaderPosition) {
  const at=position || viewport.value?.capture()
  if(at && loadedKey)rememberReadingPosition(loadedKey,{...at,zoom:zoom.value,mode:mode.value})
}
function navigate(page:number) {
  if(!pdf.value || page<1 || page>pdf.value.numPages)return
  const at=viewport.value?.capture()
  if(at)history.value=[...history.value.slice(-49),at]
  pageNumber.value=page
}
async function back() {
  const at=history.value.pop()
  if(at){pageNumber.value=at.page;await nextTick();await viewport.value?.restore(at)}
}
function chooseMatch(index:number) {
  const match=select(index)
  if(match){navigate(match.page);void nextTick(()=>viewport.value?.scrollToMatch(index))}
}
function nextMatch(delta:number){step(delta);chooseMatch(selectedIndex.value)}
function closeReview() { remember();void flushReadingPositions();emit('close') }
function keyboard(event:KeyboardEvent) {
  if(event.isComposing)return
  if(event.key==='Escape'){event.preventDefault();closeReview();return}
  const formField=event.target instanceof HTMLElement && !!event.target.closest('input,textarea,select,[contenteditable="true"]')
  if((event.ctrlKey||event.metaKey)&&!event.altKey){
    if(event.key.toLowerCase()==='f'){event.preventDefault();findInput.value?.focus();findInput.value?.select();return}
    if(formField)return
    if(['+','=','-','0'].includes(event.key)){
      event.preventDefault()
      zoom.value=event.key==='0'?'fit-width':nextZoom(zoom.value,scale.value,event.key==='-'?-1:1)
    }
    return
  }
  if(formField||event.altKey||event.shiftKey||!pdf.value)return
  const destination=event.key==='PageUp'?pageNumber.value-1:event.key==='PageDown'?pageNumber.value+1:
    event.key==='Home'?1:event.key==='End'?pdf.value.numPages:null
  if(destination!==null){event.preventDefault();navigate(Math.min(pdf.value.numPages,Math.max(1,destination)))}
}
async function load() {
  remember()
  const call=++generation
  const key=`export:${props.matterId}:${props.exportId}:${props.filename}`
  void reader.close()
  loadedKey='';outline.value=[];outlineError.value='';renderError.value='';history.value=[];query.value=''
  loading.value=true
  const saved=await loadReadingPosition(key)
  if(call!==generation)return
  pageNumber.value=saved?.page || 1;zoom.value=saved?.zoom || 'fit-page';mode.value=saved?.mode || 'single'
  const matterId=props.matterId,exportId=props.exportId,filename=props.filename
  const opened=await reader.open(()=>exportPdfDocumentSource(matterId,exportId,filename))
  if(!opened || call!==generation)return
  loadedKey=key;pageNumber.value=Math.min(pageNumber.value,opened.numPages)
  await nextTick()
  if(saved)await viewport.value?.restore({...saved,page:pageNumber.value})
  try {
    const result=await readPdfSourceOutline(opened,{isCancelled:()=>call!==generation})
    if(call!==generation)return
    const rows:typeof outline.value=[]
    function collect(entries:PdfSourceOutlineEntry[],level:number){for(const entry of entries){rows.push({...entry,level});collect(entry.children,level+1)}}
    collect(result.entries,0);outline.value=rows
    if(result.truncated)outlineError.value='Showing the first available PDF bookmarks.'
  }catch{if(call===generation)outlineError.value='The PDF bookmarks could not be read.'}
}
watch(()=>[props.matterId,props.exportId,props.filename],()=>void load(),{immediate:true})
onMounted(()=>dialog.value?.showModal())
onBeforeUnmount(()=>{remember();generation++;void flushReadingPositions()})
</script>

<template>
  <dialog ref="dialog" class="export-review" aria-label="Review generated PDF" @keydown.stop="keyboard" @cancel.prevent="closeReview" @click="($event.target===dialog)&&closeReview()">
    <header><div><h2>Review {{filename}}</h2><p>Build {{exportId.slice(0,8)}} · Check the printed index, page numbers and PDF bookmarks. Follow index links to review their destinations.</p></div><button class="btn secondary small" autofocus @click="closeReview">Close review</button></header>
    <p v-if="stale" class="export-review-status" role="status">Matter inputs changed since this export. This is the earlier generated file; rebuild before using it.</p>
    <p v-if="loading" role="status">Opening generated PDF…</p>
    <p v-if="error || renderError" class="error-msg" role="alert">The generated PDF could not be displayed. {{error || renderError}} <button class="btn secondary small" @click="load">Retry review</button></p>
    <p v-if="readingPositionError" class="export-review-status" role="status">{{readingPositionError}}</p>
    <div v-if="pdf" class="export-review-content">
      <aside aria-label="Generated PDF navigation"><h3>PDF bookmarks</h3><p v-if="outlineError" role="status">{{outlineError}}</p><p v-if="!outline.length">No bookmarks in this PDF.</p><button v-for="(item,index) in outline" :key="index" :disabled="!item.page && !item.url" :style="{paddingLeft:`${8+item.level*13}px`}" @click="item.page?navigate(item.page):item.url&&emit('openExternalLink',item.url)">{{item.title}} <small v-if="item.page">p{{item.page}}</small></button></aside>
      <div class="export-review-document">
        <div class="export-search" role="search" aria-label="Find in generated PDF"><input ref="findInput" v-model="query" type="search" aria-label="Find text in generated PDF" placeholder="Find text in this PDF" @keydown.enter.prevent="nextMatch($event.shiftKey?-1:1)"/><span role="status">{{searching?'Searching…':query?`${matches.length}${truncated?'+':''} match${matches.length===1?'':'es'}${matches.length?` · ${selectedIndex+1}/${matches.length}`:''}`:''}}</span><button class="icon-btn small" aria-label="Previous search result" :disabled="!matches.length" @click="nextMatch(-1)">↑</button><button class="icon-btn small" aria-label="Next search result" :disabled="!matches.length" @click="nextMatch(1)">↓</button></div>
        <p v-if="searchError" class="error-msg" role="status">{{searchError}}</p>
        <div v-if="query && matches.length" class="export-search-results" aria-label="Generated PDF search occurrences"><button v-for="match in matches" :key="match.index" :class="{active:match.index===selectedIndex}" @click="chooseMatch(match.index)">p{{match.page}} · {{match.snippet}}</button></div>
        <PdfReaderViewport ref="viewport" :pdf="pdf" v-model:page="pageNumber" :zoom="zoom" :mode="mode" :matches="matches" :active-match-index="query?selectedIndex:-1" @ready="scale=$event.scale" @position="remember" @error="renderError=$event" @internal-link="navigate" @external-link="emit('openExternalLink',$event)"/>
        <PdfReaderControls :page="pageNumber" :count="pdf.numPages" :scale="scale" v-model:zoom="zoom" v-model:mode="mode" :busy="loading" :can-back="!!history.length" page-label="PDF page number" @update:page="navigate" @back="back"/>
      </div>
    </div>
  </dialog>
</template>

<style scoped>
.export-review{position:fixed;inset:0;margin:auto;padding:0;border:0;max-width:98vw;max-height:94vh}
.export-review:not([open]){display:none}.export-review::backdrop{background:#0b1727b8}
.export-review-status{margin:0;padding:8px 16px;font-size:12px;line-height:1.5;color:var(--muted);background:var(--surface-soft)}
.export-search{display:flex;align-items:center;gap:6px;padding:8px 10px;border-bottom:1px solid var(--border);flex-wrap:wrap}
.export-search input{flex:1;min-width:140px;min-height:36px;padding:6px 9px;border:1px solid var(--border);border-radius:6px;background:var(--panel);color:var(--text)}
.export-search span{font-size:12px;color:var(--muted)}
.export-search-results{max-height:100px;overflow:auto;border-bottom:1px solid var(--border);flex-shrink:0}
.export-search-results button{display:block;border:0;background:transparent;color:var(--text);width:100%;padding:7px 10px;text-align:left;font-size:12px}
.export-search-results button.active{background:var(--surface-soft)}
</style>
