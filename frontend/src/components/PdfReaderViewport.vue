<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, shallowRef, watch } from 'vue'
import type { PDFDocumentProxy } from 'pdfjs-dist'
import PdfPageSurface from './PdfPageSurface.vue'
import { pageAtOffset, readerScale, type ReaderMode, type ReaderPosition } from '../lib/pdfReader'
import type { PdfSearchMatch } from '../lib/pdfSearch'

const props = withDefaults(defineProps<{pdf:PDFDocumentProxy|null,page:number,zoom:string,mode:ReaderMode,
  interactive?:boolean, matches?:PdfSearchMatch[],activeMatchIndex?:number, surfaceClass?:Record<string,boolean>}>(),
  {interactive:true,matches:()=>[],activeMatchIndex:-1})
const emit = defineEmits<{'update:page':[number],position:[ReaderPosition], ready:[{page:number,scale:number,width:number,height:number}],
  error:[string], internalLink:[number], externalLink:[string], surfaceClick:[MouseEvent],
  surfacePointerdown:[PointerEvent],surfacePointermove:[PointerEvent],surfacePointerup:[PointerEvent],surfacePointercancel:[PointerEvent]}>()
const scroller = ref<HTMLDivElement|null>(null)
type Surface = InstanceType<typeof PdfPageSurface>
const surfaces = new Map<number,Surface>()
type PageSize = {width:number,height:number}
const sizes = ref<PageSize[]>([])
const measuredPages = ref(new Set<number>())
const metadataDocument = shallowRef<PDFDocumentProxy|null>(null)
const width = ref(800), height = ref(600), scrollTop = ref(0)
const gap = 24
const metrics = computed(() => sizes.value.map(size => {
  const scale = readerScale(props.zoom,width.value,height.value,size.width,size.height)
  return {...size,scale,renderWidth:size.width*scale,renderHeight:size.height*scale}
}))
const offsets = computed(() => {
  const values=[0]; for (const item of metrics.value) values.push(values[values.length-1]+item.renderHeight+gap)
  return values
})
const visiblePages = computed(() => {
  if (!metrics.value.length) return []
  if (props.mode==='single') return [Math.min(metrics.value.length,Math.max(1,props.page))]
  const start=pageAtOffset(offsets.value,Math.max(0,scrollTop.value-height.value*.5))
  const end=pageAtOffset(offsets.value,scrollTop.value+height.value*1.5)
  // Cover the whole viewport even for tiny PDF pages. The 24px page gap bounds
  // the number of mounted surfaces by viewport height, not document length.
  return Array.from({length:end-start+1},(_,i)=>start+i+1)
})
// Steady-state visible bitmap target. Offscreen staging and old/new bitmap swaps
// can temporarily exceed this while pages render or the visible count changes.
const rasterBudget = computed(()=>Math.min(16_000_000,Math.floor(32_000_000/Math.max(props.mode==='single'?1:4,visiblePages.value.length))))
const topSpace = computed(() => props.mode==='continuous' ? offsets.value[(visiblePages.value[0] || 1)-1] || 0 : 0)
const bottomSpace = computed(() => props.mode==='continuous' ? Math.max(0,(offsets.value[offsets.value.length-1]||0)-(offsets.value[visiblePages.value[visiblePages.value.length-1]||0]||0)) : 0)
const maxWidth = computed(() => props.mode==='single' ? metrics.value[props.page-1]?.renderWidth || 0 : Math.max(0,...metrics.value.map(m=>m.renderWidth)))
let generation=0, restoreSequence=0, observer:ResizeObserver|null=null, scrollFrame=0, restoring=false, scrollSelectedPage=0
let pendingPosition:ReaderPosition|null=null
let lastAnchor:ReaderPosition={page:props.page,y:0,x:0}
const rendered = new Map<number,{page:number,scale:number,width:number,height:number}>()
const pendingMeasurements = new Map<number,Promise<{width:number,height:number}>>()
const paintWaiters = new Set<{page:number,finish:(painted:boolean)=>void}>()
function waitForPage(page:number):Promise<boolean> {
  if(rendered.has(page) && surfaces.get(page)?.getCanvas()?.width)return Promise.resolve(true)
  return new Promise(resolve=>{
    const entry={page,finish:(painted:boolean)=>{clearTimeout(timeout);paintWaiters.delete(entry);resolve(painted)}}
    const timeout=setTimeout(()=>entry.finish(false),15000)
    paintWaiters.add(entry)
  })
}
function surfaceError(message:string){for(const entry of paintWaiters)entry.finish(false);emit('error',message)}
function capture():ReaderPosition {
  if(restoring && pendingPosition)return {...pendingPosition}
  const wrap=scroller.value
  // Read the painted layout, not metrics that may already reflect a new zoom.
  if(wrap){
    const bounds=wrap.getBoundingClientRect(), density=bounds.width/wrap.offsetWidth || 1
    const top=bounds.top+20*density
    const frames=Array.from(wrap.querySelectorAll<HTMLElement>('[data-reader-page]'))
    const frame=frames.find(item=>item.getBoundingClientRect().bottom>top) || frames[frames.length-1]
    if(frame){
      const rect=frame.getBoundingClientRect()
      if(rect.height>0 && rect.width>0)return {page:Number(frame.dataset.readerPage),
        y:Math.max(0,(top-rect.top)/rect.height),x:wrap.scrollLeft/(rect.width/density)}
    }
  }
  const index=props.mode==='continuous' ? pageAtOffset(offsets.value,wrap?.scrollTop||0) : props.page-1
  const metric=metrics.value[index]
  return {page:index+1,y:metric ? Math.max(0,((wrap?.scrollTop||0)-(props.mode==='continuous'?(offsets.value[index]||0):0))/metric.renderHeight) : 0,
    x:metric ? (wrap?.scrollLeft||0)/metric.renderWidth : 0}
}
async function restore(position:ReaderPosition) {
  const call=++restoreSequence
  restoring=true
  pendingPosition=position
  lastAnchor={...position}
  await nextTick()
  if(call!==restoreSequence)return
  // Retained frames can belong to the previous document. Keep the requested
  // anchor pending until this document's initial metadata can place it safely.
  if (!props.pdf || metadataDocument.value!==props.pdf) return
  const wrap=scroller.value, index=Math.min(metrics.value.length-1,Math.max(0,position.page-1)), metric=metrics.value[index]
  if (!wrap || !metric) {restoring=false;return}
  wrap.scrollTop=(props.mode==='continuous'?(offsets.value[index]||0):0)+Math.max(0,position.y)*metric.renderHeight
  wrap.scrollLeft=Math.max(0,position.x)*metric.renderWidth
  scrollTop.value=wrap.scrollTop
  await nextTick()
  if(call===restoreSequence){
    restoring=false;pendingPosition=null
    // Page/zoom/mode changes at the top of a single page need not dispatch a
    // scroll event. Persist their settled anchor before the window can close.
    remember()
  }
}
function remember() { if (metrics.value.length) {lastAnchor=capture();emit('position',lastAnchor)} }
function onScroll() {
  if (scrollFrame) return
  scrollFrame=requestAnimationFrame(() => {
    scrollFrame=0
    const wrap=scroller.value
    if (!wrap) return
    scrollTop.value=wrap.scrollTop
    if (!restoring && props.mode==='continuous' && metrics.value.length) {
      const number=pageAtOffset(offsets.value,wrap.scrollTop+wrap.clientHeight*.5)+1
      if (number!==props.page) {scrollSelectedPage=number;emit('update:page',number)}
    }
    if (!restoring) remember()
  })
}
function pageMeasured(page:number) {
  return !!props.pdf && metadataDocument.value===props.pdf && measuredPages.value.has(page)
}
function pageSize(document:PDFDocumentProxy, number:number) {
  let pending=pendingMeasurements.get(number)
  if (!pending) {
    pending=document.getPage(number).then(page=>{
      const viewport=page.getViewport({scale:1})
      return {width:viewport.width,height:viewport.height}
    })
    pendingMeasurements.set(number,pending)
    const request=pending
    // Both eager visible-page requests and background hydration share a request.
    void pending.finally(()=>{
      if(pendingMeasurements.get(number)===request)pendingMeasurements.delete(number)
    }).catch(()=>{})
  }
  return pending
}
async function measurePages(numbers:number[], call=generation, document=props.pdf) {
  if (!document || metadataDocument.value!==document || call!==generation) return
  const missing=[...new Set(numbers)].filter(number=>number>=1 && number<=document.numPages && !measuredPages.value.has(number))
  if (!missing.length) return
  const measurements=await Promise.all(missing.map(async number=>({number,...await pageSize(document,number)})))
  if (call!==generation || document!==props.pdf) return
  const anchor=capture()
  let next:PageSize[]|undefined
  for (const {number,width,height} of measurements) {
    const prior=sizes.value[number-1]
    if (!prior || measuredPages.value.has(number)) continue
    if (prior.width!==width || prior.height!==height) {
      next ||= [...sizes.value]
      next[number-1]={width,height}
    }
    measuredPages.value.add(number)
  }
  // Uniform documents do not need an O(page-count) metric recalculation for
  // every metadata batch; readiness is independent of layout dimensions.
  if (next) {sizes.value=next;await restore(anchor)}
}
async function loadSizes() {
  const call=++generation, document=props.pdf
  metadataDocument.value=null
  measuredPages.value=new Set()
  pendingMeasurements.clear()
  for(const entry of paintWaiters)entry.finish(false)
  // Retain layout while a revision loads, including the editor's old-page snapshot.
  // PdfPageSurface independently clears its bitmap and text when pdf becomes null.
  rendered.clear()
  if (!document) return
  try {
    const firstNumber=Math.min(Math.max(1,props.page),document.numPages)
    const viewport=await pageSize(document,firstNumber)
    if (call!==generation) return
    sizes.value=Array.from({length:document.numPages},()=>({...viewport}))
    measuredPages.value.add(firstNumber)
    metadataDocument.value=document
    await restore(pendingPosition || {page:props.page,y:0,x:0})
    // Background metadata uses four-page batches; visible pages take priority
    // through measurePages without waiting for those batches. Bitmaps stay local.
    for (let start=0;start<document.numPages;start+=4) {
      await measurePages(Array.from({length:Math.min(4,document.numPages-start)},(_,i)=>start+i+1),call,document)
      if (call!==generation) return
      if (start%32===0) await new Promise(resolve=>setTimeout(resolve,0))
    }
  } catch (err) { if (call===generation) emit('error',err instanceof Error?err.message:String(err)) }
}
function onReady(data:{page:number,scale:number,width:number,height:number}) {
  rendered.set(data.page,data)
  if (data.page===props.page) emit('ready',data)
  for(const entry of paintWaiters)if(entry.page===data.page)entry.finish(true)
  if (props.activeMatchIndex>=0 && props.matches[props.activeMatchIndex]?.page===data.page)
    void nextTick(()=>surfaces.get(data.page)?.scrollToMatch(props.activeMatchIndex))
}
function fragments(page:number) { return props.matches.filter(match=>match.page===page).flatMap(match=>match.fragments) }
function surfaceEvent(name:'surfaceClick'|'surfacePointerdown'|'surfacePointermove'|'surfacePointerup'|'surfacePointercancel',event:MouseEvent|PointerEvent,page:number) {
  if (page!==props.page) { if (props.interactive) emit('update:page',page); return }
  if (name==='surfaceClick') emit(name,event as MouseEvent)
  else if(name==='surfacePointerdown')emit('surfacePointerdown',event as PointerEvent)
  else if(name==='surfacePointermove')emit('surfacePointermove',event as PointerEvent)
  else if(name==='surfacePointerup')emit('surfacePointerup',event as PointerEvent)
  else emit('surfacePointercancel',event as PointerEvent)
}
function getCanvas() { return surfaces.get(props.page)?.getCanvas() || null }
function getElement() { return surfaces.get(props.page)?.getElement() || null }
function getScroller() { return scroller.value }
async function scrollToMatch(index:number) { await nextTick(); surfaces.get(props.page)?.scrollToMatch(index) }
watch(()=>props.pdf,()=>void loadSizes(),{immediate:true})
watch(()=>[metadataDocument.value,props.page,visiblePages.value.join(',')],()=>{
  const call=generation
  // Jump targets bypass sequential metadata hydration. Until measured, their
  // frames reserve estimated space but never stretch a painted page to a guess.
  void measurePages([props.page,...visiblePages.value]).catch(error=>{
    if(call===generation)emit('error',error instanceof Error?error.message:String(error))
  })
})
watch(()=>props.page,async page=>{
  if (scrollSelectedPage===page) scrollSelectedPage=0
  else await restore({page,y:0,x:0})
  const ready=rendered.get(page), surface=surfaces.get(page), metric=metrics.value[page-1]
  if(ready && surface?.getCanvas()?.width && metric && ready.scale===metric.scale) emit('ready',ready)
})
watch(()=>[props.zoom,props.mode],async(values,previous)=>{
  const anchor=capture()
  if(values[1]!==previous[1])anchor.page=props.page
  rendered.clear(); await restore(anchor)
},{flush:'pre'})
watch(()=>props.activeMatchIndex,index=>{if(index>=0)void scrollToMatch(index)})
watch(scroller,element=>{
  observer?.disconnect(); observer=null
  if (!element) return
  const measure=()=>{
    const anchor=capture()
    if(width.value===element.clientWidth&&height.value===element.clientHeight)return
    rendered.clear()
    width.value=element.clientWidth; height.value=element.clientHeight
    void restore(anchor)
  }
  observer=new ResizeObserver(measure);observer.observe(element);measure()
})
onBeforeUnmount(()=>{remember();generation++;observer?.disconnect();if(scrollFrame)cancelAnimationFrame(scrollFrame);for(const entry of paintWaiters)entry.finish(false)})
defineExpose({getCanvas,getElement,getScroller,capture,restore,scrollToMatch,waitForPage})
</script>
<template>
  <div ref="scroller" class="canvas-scroll shared-reader-viewport" :class="{'continuous-reader':mode==='continuous'}" @scroll.passive="onScroll">
    <div class="reader-page-stack" :style="{minWidth:`${maxWidth+40}px`}">
      <div v-if="topSpace" :style="{height:`${topSpace}px`}" aria-hidden="true"></div>
      <div v-for="number in visiblePages" :key="number" class="reader-page-frame" :data-reader-page="number" :aria-label="`PDF page ${number}`" :style="{height:`${metrics[number-1]?.renderHeight||0}px`,width:`${metrics[number-1]?.renderWidth||0}px`}">
        <PdfPageSurface :ref="(value:any)=>{if(value)surfaces.set(number,value);else {surfaces.delete(number);rendered.delete(number)}}" :pdf="pageMeasured(number)?pdf:null" :page-number="number" :scale="metrics[number-1]?.scale||1" :interactive="interactive" :active="true" :max-raster-pixels="rasterBudget"
          :style="{width:`${metrics[number-1]?.renderWidth||0}px`,height:`${metrics[number-1]?.renderHeight||0}px`}" :class="number===page?surfaceClass:undefined" :search-fragments="fragments(number)" :active-match-index="activeMatchIndex"
          @ready="onReady" @error="surfaceError" @internal-link="emit('internalLink',$event)" @external-link="emit('externalLink',$event)"
          @click="surfaceEvent('surfaceClick',$event,number)" @pointerdown="surfaceEvent('surfacePointerdown',$event,number)" @pointermove="surfaceEvent('surfacePointermove',$event,number)" @pointerup="surfaceEvent('surfacePointerup',$event,number)" @pointercancel="surfaceEvent('surfacePointercancel',$event,number)">
          <slot v-if="number===page" :page="number" :scale="metrics[number-1]?.scale||1"></slot>
        </PdfPageSurface>
      </div>
      <div v-if="bottomSpace" :style="{height:`${bottomSpace}px`}" aria-hidden="true"></div>
    </div>
  </div>
</template>
<style scoped>
.shared-reader-viewport{display:block!important;position:relative;overflow:auto;min-height:0;flex:1;padding:20px 0!important;overflow-anchor:none;background:var(--surface-soft)}
.reader-page-stack{display:flex;flex-direction:column;align-items:center;min-height:100%;width:100%;}
.reader-page-frame{position:relative;flex:none;margin-bottom:24px;}
.reader-page-frame:last-child{margin-bottom:0}
</style>
