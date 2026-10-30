<script setup lang="ts">
import { ArrowLeft, BookOpen, Focus, Search, Info, Shield, Pencil, Highlighter, LayoutGrid, ScanText } from 'lucide-vue-next'
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import EngineNotice from './EngineNotice.vue'
import { engineBlocked } from '../lib/engineConnection'
import PdfReaderViewport from './PdfReaderViewport.vue'
import PdfReaderControls from './PdfReaderControls.vue'
import PdfThumbnailRail from './PdfThumbnailRail.vue'
import type { ExportManifest } from '../lib/types'
import { api, originalDocumentSource, originalImage, exportPdfDocumentSource } from '../lib/api'
import { usePdfDocument, rememberReadingPosition, loadReadingPosition, flushReadingPositions, readingPositionError, nextZoom, type ReaderMode, type ReaderPosition } from '../lib/pdfReader'
import { usePdfSearch } from '../lib/pdfSearch'
import { readPdfSourceOutline, type PdfSourceOutlineEntry } from '../lib/pdfNavigation'

const props = defineProps<{matterId:string,exportId:string,filename:string,stale?:boolean,manifest?:ExportManifest,originalId?:string,artifactOptions?:Array<{value:string,label:string}>}>()
const emit = defineEmits<{close:[],export:[],checks:[],rebuild:[],source:[id:string,page:number,revision:number],retryEngine:[],artifact:[value:string],openExternalLink:[url:string]}>()
type OriginalInfo = {filename:string,sha256:string,media_type:string,page_count:number|null,password_protected:boolean}
const generated = computed(()=>!props.originalId)
const sourceRow = computed(()=>{
 const volume = props.filename.match(/^volume-(\d+)\.pdf$/)?.[1]
 return props.manifest?.page_map_snapshot?.find(row=>volume ? row.volume===Number(volume)&&pageNumber.value>=row.start_page&&pageNumber.value<=row.end_page : props.filename==='combined.pdf'&&pageNumber.value>=row.combined_start_page&&pageNumber.value<=row.combined_end_page)
})
const printedLabel = computed(()=>{
 const row=sourceRow.value
 if(!row)return ''
 const offset=pageNumber.value-(props.filename==='combined.pdf'?row.combined_start_page:row.start_page)
 return row.printed_labels?.[offset] ?? String(row.start_page+offset)
})
function viewSource(){
 const row=sourceRow.value
 const revision=props.manifest?.input_revisions?.find(r=>r.document_id===row?.id)?.revision
 if(row&&revision)emit('source',row.id,pageNumber.value-(props.filename==='combined.pdf'?row.combined_start_page:row.start_page)+1,revision)
}
const originalInfo = ref<OriginalInfo|null>(null)
const originalImageUrl = ref('')
const artifactLabel = computed(()=>props.originalId ? 'Imported original' : 'Generated PDF')
const artifactValue = computed(()=>props.originalId ? 'original' : `export:${props.exportId}:${props.filename}`)
let returnFocus: HTMLElement|null = null
const dialog = ref<HTMLDialogElement|null>(null)
const findInput = ref<HTMLInputElement|null>(null)
const searchOpen = ref(false)
const infoOpen = ref(false)
async function showSearch() { searchOpen.value=!searchOpen.value; if(searchOpen.value){await nextTick();findInput.value?.focus()} }
const viewport = ref<InstanceType<typeof PdfReaderViewport>|null>(null)
const reader = usePdfDocument()
const {pdf,loading,error} = reader
const pageNumber = ref(1), scale = ref(1), zoom = ref('fit-width'), mode = ref<ReaderMode>('single')
const history = ref<ReaderPosition[]>([])
const outline = ref<Array<{title:string,page:number|null,url:string|null,level:number}>>([])
const outlineError = ref(''), renderError = ref('')
const query = ref('')
const navigationOpen = ref(true), focusedReading = ref(false)
const focusButton = ref<HTMLButtonElement|null>(null)
async function toggleFocus() { focusedReading.value = !focusedReading.value; await nextTick(); focusButton.value?.focus() }
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
  if(event.key==='Tab'){
    const controls=Array.from(dialog.value?.querySelectorAll<HTMLElement>('button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),summary,[tabindex="0"]') || [])
      .filter(el=>el.getClientRects().length && el.closest('dialog')===dialog.value)
    const first=controls[0],last=controls[controls.length-1]
    if(event.shiftKey && document.activeElement===first){event.preventDefault();last?.focus()}
    else if(!event.shiftKey && document.activeElement===last){event.preventDefault();first?.focus()}
    return
  }
  if(event.key==='Escape'){event.preventDefault();if(focusedReading.value){void toggleFocus();return}closeReview();return}
  const formField=event.target instanceof HTMLElement && !!event.target.closest('input,textarea,select,[contenteditable="true"]')
  if((event.ctrlKey||event.metaKey)&&!event.altKey){
    if(event.key.toLowerCase()==='f'){event.preventDefault();focusedReading.value=false;searchOpen.value=true;void nextTick(()=>{findInput.value?.focus();findInput.value?.select()});return}
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
async function load(retainContext = false) {
  const retained = retainContext ? viewport.value?.capture() : null
  const retainedQuery = query.value
  const retainedZoom = zoom.value, retainedMode = mode.value
  remember()
  const call=++generation
  const originalId=props.originalId
  const key=originalId ? `original:${originalId}` : `export:${props.matterId}:${props.exportId}:${props.filename}`
  originalInfo.value=null
  if(originalImageUrl.value)URL.revokeObjectURL(originalImageUrl.value)
  originalImageUrl.value=''
  void reader.close()
  loadedKey='';outline.value=[];outlineError.value='';renderError.value='';error.value='';history.value=[];query.value=''
  loading.value=true
  const saved=retained ? {...retained,zoom:retainedZoom,mode:retainedMode} : await loadReadingPosition(key)
  if(retainContext)query.value=retainedQuery
  if(call!==generation)return
  pageNumber.value=saved?.page || 1;zoom.value=saved?.zoom || (originalId?'fit-page':'75');mode.value=saved?.mode || 'single'
  const matterId=props.matterId,exportId=props.exportId,filename=props.filename
  if(originalId){
    try {
      const info=await api<OriginalInfo>(`/documents/${originalId}/original-info`)
      if(call!==generation)return
      originalInfo.value=info
      if(info.password_protected){error.value='This preserved PDF is password protected. Original preview is unavailable; the decrypted working copy is a separate artifact.';loading.value=false;return}
      if(info.media_type!=='application/pdf'){
        const blob=await originalImage(originalId)
        if(call!==generation)return
        originalImageUrl.value=URL.createObjectURL(blob);loading.value=false;return
      }
    }catch(err){if(call===generation){error.value=err instanceof Error?err.message:String(err);loading.value=false}return}
  }
  const opened=await reader.open(()=>originalId ? originalDocumentSource(originalId) : exportPdfDocumentSource(matterId,exportId,filename))
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
watch(()=>[props.matterId,props.exportId,props.filename,props.originalId],()=>void load(),{immediate:true})
onMounted(()=>{
  returnFocus=document.activeElement as HTMLElement|null
  // Keep the PDF usable when the available window is too narrow for a docked outline.
  navigationOpen.value=window.innerWidth/Number.parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--ui-scale')||'1')>1100
  dialog.value?.showModal()
})
onBeforeUnmount(()=>{remember();generation++;if(originalImageUrl.value)URL.revokeObjectURL(originalImageUrl.value);dialog.value?.close();void nextTick(()=>{(returnFocus?.isConnected ? returnFocus : document.querySelector<HTMLElement>('.viewer-root'))?.focus({preventScroll:true})});void flushReadingPositions()})
</script>

<template>
  <dialog ref="dialog" class="export-review" :class="{'artifact-focused':focusedReading,'generated-review':generated}" :aria-label="originalId ? 'Review imported original' : 'Review generated PDF'" @keydown.stop="keyboard" @cancel.prevent="closeReview" @click="($event.target===dialog)&&closeReview()">
    <header v-if="generated" class="generated-header"><div><h2>{{filename}}</h2><p><span>Generated Output</span> Build {{exportId.slice(0,8)}} · {{stale?'Stale':'Saved'}} · Read only</p><select v-if="artifactOptions" class="generated-version" aria-label="Document version" :value="artifactValue" @change="emit('artifact',($event.target as HTMLSelectElement).value)"><option v-for="option in artifactOptions" :key="option.value" :value="option.value">{{option.label}}</option></select></div><button class="membership-button quiet" @click="emit('checks')">Output checks</button><button class="membership-button primary" @click="emit('export')">{{stale?'Save a copy':'Export files'}}</button><button class="icon-btn" aria-label="Close review" @click="closeReview"><ArrowLeft :size="18"/></button></header>
    <header v-else class="artifact-header"><button class="btn secondary small" aria-label="Close review" @click="closeReview"><ArrowLeft :size="16"/> Documents</button><div class="artifact-title"><h2 :title="filename">{{filename}}</h2><select v-if="artifactOptions" aria-label="Document version" :value="artifactValue" @change="emit('artifact',($event.target as HTMLSelectElement).value)"><option v-for="option in artifactOptions" :key="option.value" :value="option.value">{{option.label}}</option></select><span v-else>{{artifactLabel}} · Read only</span></div><span class="artifact-readonly">{{originalId ? 'Preserved source file' : stale ? 'Previous generated output · Stale' : 'Generated PDF · Read only'}}</span><button v-if="artifactOptions" class="btn primary small" @click="emit('artifact','working')">Working Copy</button></header>
    <EngineNotice review-label="Close review to reconcile" @retry="emit('retryEngine')" @review="closeReview"/>
    <div v-if="!generated" class="artifact-modebar"><span class="artifact-read-tab"><BookOpen :size="15"/> Read</span><button v-for="tool in ['Edit','Annotate','Pages','OCR','Redact']" :key="tool" class="text-tool" :aria-label="`${tool} unavailable in read-only artifact`" disabled :title="`${tool} is available in the working copy`"><component :is="tool==='Edit'?Pencil:tool==='Annotate'?Highlighter:tool==='Pages'?LayoutGrid:tool==='OCR'?ScanText:Shield" :size="15"/>{{tool}}</button><span class="artifact-readonly">{{artifactLabel}} · Read only</span><button ref="focusButton" class="icon-btn small" :aria-label="focusedReading ? 'Exit focused reading' : 'Focused reading'" :aria-pressed="focusedReading" @click="toggleFocus"><Focus :size="16"/></button></div>
    <span class="sr-only">{{originalInfo?.page_count}} original source pages · {{artifactLabel}} · Read only</span>
    <div v-if="originalImageUrl" class="original-image"><img :src="originalImageUrl" :alt="`Imported original ${filename}`"/><p>Original image · The working PDF is a converted copy.</p></div>

    <p v-if="loading" role="status">Opening {{artifactLabel}}…</p>
    <p v-if="error || renderError" class="error-msg" role="alert">{{artifactLabel}} could not be displayed. {{error || renderError}} <button class="btn secondary small" :disabled="engineBlocked" @click="load(true)">Retry review</button></p>
    <p v-if="readingPositionError && !engineBlocked" class="export-review-status" role="status">{{readingPositionError}}</p>
    <template v-if="pdf">        <PdfReaderControls :page="pageNumber" :count="pdf.numPages" :scale="scale" v-model:zoom="zoom" v-model:mode="mode" :busy="loading" :can-back="!!history.length" :page-label="originalId ? 'Original source page number' : 'PDF page number'" @update:page="navigate" @back="back"><template #actions><button v-if="generated" ref="focusButton" class="icon-btn small" :aria-label="focusedReading ? 'Exit focused reading' : 'Focused reading'" :aria-pressed="focusedReading" @click="toggleFocus"><Focus :size="16"/></button><button class="icon-btn small" aria-label="Search artifact" :aria-expanded="searchOpen" @click="showSearch"><Search :size="16"/></button><button class="icon-btn small" :aria-expanded="navigationOpen && !focusedReading" aria-controls="artifact-navigation" :aria-label="originalId ? 'Pages' : 'PDF bookmarks'" @click="navigationOpen=!navigationOpen;focusedReading=false"><LayoutGrid :size="16"/></button><button class="icon-btn small reader-info-trigger" aria-label="Artifact information" :aria-expanded="infoOpen" @click="infoOpen=!infoOpen"><Info :size="16"/></button></template></PdfReaderControls></template>
    <div v-if="generated && pdf" class="generated-page-context"><span>{{sourceRow ? `Volume ${sourceRow.volume} · Printed ${printedLabel}` : 'Generated front matter'}} · {{filename==='combined.pdf'?'Combined':'Physical'}} {{pageNumber}}</span><button v-if="sourceRow" class="text-tool" @click="viewSource">View source: {{sourceRow.title}}</button></div>
    <div v-if="pdf" class="export-review-content" :class="{'navigation-open':navigationOpen && !focusedReading}">
      <aside v-if="infoOpen && !focusedReading" class="artifact-info-panel" aria-label="Artifact information"><h3>{{originalId?'Original file':'Generated file'}}</h3><p>Read only</p><strong>{{filename}}</strong><template v-if="originalInfo"><h4>Pages</h4><p>{{originalInfo.page_count}} original source pages</p><h4>Preservation</h4><p>Source file unchanged. Edits belong to the separate working copy.</p><h4>SHA-256</h4><code>{{originalInfo.sha256}}</code></template><template v-else><h4>Build</h4><p>{{exportId}}</p><p>Source, printed and combined page positions are separate.</p></template><button v-if="artifactOptions" class="btn primary small" @click="emit('artifact','working')">Open Working Copy</button></aside>
      <div id="artifact-navigation" v-show="navigationOpen && !focusedReading" class="artifact-navigation">
      <PdfThumbnailRail v-if="originalId" :pdf="pdf" :count="pdf.numPages" :page="pageNumber" title="Imported original · Read only" :dragging-page="null" :drop-target-page="null" read-only :disabled="loading" @select="navigate"/>
      <aside v-else aria-label="Generated PDF navigation"><h3>Bookmarks</h3><p class="generated-nav-badge">Generated Output</p><p v-if="outlineError" role="status">{{outlineError}}</p><p v-if="!outline.length">No bookmarks in this PDF.</p><button v-for="(item,index) in outline" :key="index" :disabled="!item.page && !item.url" :style="{paddingLeft:`${8+item.level*13}px`}" @click="item.page?navigate(item.page):item.url&&emit('openExternalLink',item.url)">{{item.title}} <small v-if="item.page">p{{item.page}}</small></button></aside>
      <p v-if="generated" class="generated-nav-footer">Build {{exportId.slice(0,8)}} · {{manifest?.created_at?.slice(0,10)}}<br/>{{pdf.numPages}} physical pages · Read only</p>
      </div>
      <div class="export-review-document">
        <div v-show="searchOpen && !focusedReading" class="export-search" role="search" :aria-label="originalId ? 'Find in imported original' : 'Find in generated PDF'"><input ref="findInput" v-model="query" type="search" :aria-label="originalId ? 'Find text in imported original' : 'Find text in generated PDF'" placeholder="Find text in this PDF" @keydown.enter.prevent="nextMatch($event.shiftKey?-1:1)"/><span role="status">{{searching?'Searching…':query?`${matches.length}${truncated?'+':''} match${matches.length===1?'':'es'}${matches.length?` · ${selectedIndex+1}/${matches.length}`:''}`:''}}</span><button class="icon-btn small" aria-label="Previous search result" :disabled="!matches.length" @click="nextMatch(-1)">↑</button><button class="icon-btn small" aria-label="Next search result" :disabled="!matches.length" @click="nextMatch(1)">↓</button></div>
        <p v-if="searchError" class="error-msg" role="status">{{searchError}}</p>
        <div v-if="searchOpen && !focusedReading && query && matches.length" class="export-search-results" :aria-label="originalId ? 'Original PDF search occurrences' : 'Generated PDF search occurrences'"><button v-for="match in matches" :key="match.index" :class="{active:match.index===selectedIndex}" @click="chooseMatch(match.index)">p{{match.page}} · {{match.snippet}}</button></div>
        <PdfReaderViewport :horizontal-gutter="32" ref="viewport" :pdf="pdf" v-model:page="pageNumber" :zoom="zoom" :mode="mode" :matches="matches" :active-match-index="query?selectedIndex:-1" @ready="scale=$event.scale;renderError=''" @position="remember" @error="renderError=`Page ${pageNumber} is unavailable. ${engineBlocked ? 'Its data may not be loaded; reconnect and retry.' : $event}`" @internal-link="navigate" @external-link="emit('openExternalLink',$event)"/>

      </div>
    </div>
    <div v-if="stale" class="generated-stale-bar" role="status"><span>Previous bundle · Matter inputs changed since this export. Earlier output remains unchanged.</span><button class="membership-button primary" @click="emit('rebuild')">Update bundle</button></div>
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
.original-image{overflow:auto;text-align:center;padding:16px}.original-image img{max-width:100%;max-height:65vh;object-fit:contain}
</style>
