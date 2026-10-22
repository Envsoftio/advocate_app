<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { PDFDocumentProxy, RenderTask } from 'pdfjs-dist'
import { pdfRasterSize } from '../lib/pdfSurface'

const props = defineProps<{pdf:PDFDocumentProxy|null,count:number,page:number,title:string,disabled?:boolean,
  draggingPage:number|null,dropTargetPage:number|null}>()
const emit = defineEmits<{select:[number],reorder:[source:number,target:number],
  dragstart:[event:DragEvent,page:number],dragend:[],dragover:[event:DragEvent,page:number],drop:[event:DragEvent,page:number]}>()
const rail = ref<HTMLDivElement|null>(null)
const heading = ref<HTMLDivElement|null>(null)
const rowHeight = 176
const headerHeight = ref(48), railHeight = ref(600), scrollTop = ref(0)
const focusedPage = ref(props.page)
const rendered = ref(new Set<number>())
const failed = ref(new Set<number>())
const start = computed(() => Math.max(0,Math.floor(Math.max(0,scrollTop.value-headerHeight.value)/rowHeight)-2))
const end = computed(() => Math.min(props.count,Math.ceil((scrollTop.value+railHeight.value)/rowHeight)+2))
const visiblePages = computed(() => {
  const values = Array.from({length:Math.max(0,end.value-start.value)},(_,index) => start.value+index+1)
  // Removing the native drag source from the DOM can end the user's drag.
  if (props.draggingPage && !values.includes(props.draggingPage)) values.push(props.draggingPage)
  return values.sort((a,b) => a-b)
})
const tabStopPage = computed(() => visiblePages.value.includes(focusedPage.value) ? focusedPage.value : visiblePages.value[0])
const canvases = new Map<number,HTMLCanvasElement>()
let generation = 0, task:RenderTask|null = null, observer:ResizeObserver|null = null
let renderTimer:ReturnType<typeof setTimeout>|null = null, dragFrame = 0, dragDirection = 0
let mounted = false, measuredWidth = 0, pendingFocusPage = 0, restoreFocusAfterBusy = false

function cancelRender() { generation++;task?.cancel();task=null;if(renderTimer)clearTimeout(renderTimer);renderTimer=null }
function registerCanvas(page:number,value:unknown) {
  const old = canvases.get(page)
  if (old && old !== value) { old.width=0;old.height=0;canvases.delete(page);rendered.value.delete(page);failed.value.delete(page) }
  if (value instanceof HTMLCanvasElement) canvases.set(page,value)
}
function scheduleRender() {
  cancelRender()
  if (!mounted || !props.pdf) return
  renderTimer=setTimeout(() => {renderTimer=null;void renderVisible(generation)},50)
}
async function renderVisible(call:number) {
  const pdf=props.pdf
  if (!pdf) return
  await nextTick()
  // One small offscreen bitmap at a time, in addition to the mounted previews.
  for (const number of visiblePages.value) {
    if (call!==generation || props.pdf!==pdf) return
    const canvas=canvases.get(number)
    if (!canvas || rendered.value.has(number)) continue
    let staged:HTMLCanvasElement|null=null
    try {
      const page=await pdf.getPage(number)
      if (call!==generation || props.pdf!==pdf || canvases.get(number)!==canvas) return
      const base=page.getViewport({scale:1})
      const box=canvas.parentElement
      const width=Math.max(20,(box?.clientWidth||112)-4),height=Math.max(20,(box?.clientHeight||132)-4)
      const viewport=page.getViewport({scale:Math.min(width/base.width,height/base.height)})
      const size=pdfRasterSize(viewport.width,viewport.height,window.devicePixelRatio||1,256_000)
      staged=document.createElement('canvas');staged.width=size.width;staged.height=size.height
      const context=staged.getContext('2d')
      if(!context)throw new Error('Thumbnail canvas unavailable')
      const pending=page.render({canvasContext:context,viewport,transform:[size.scaleX,0,0,size.scaleY,0,0]})
      task=pending
      await pending.promise
      if(call!==generation || props.pdf!==pdf || canvases.get(number)!==canvas)return
      canvas.width=size.width;canvas.height=size.height
      canvas.style.width=`${viewport.width}px`;canvas.style.height=`${viewport.height}px`
      canvas.getContext('2d')?.drawImage(staged,0,0)
      canvas.dataset.rendered='true'
      rendered.value.add(number);failed.value.delete(number)
    } catch(error) {
      if(call===generation && props.pdf===pdf && !(error instanceof Error && error.name==='RenderingCancelledException')) failed.value.add(number)
    } finally { if(staged){staged.width=0;staged.height=0};if(call===generation)task=null }
  }
}
function resetPreviews() {
  cancelRender();rendered.value.clear();failed.value.clear()
  for(const canvas of canvases.values()){canvas.width=0;canvas.height=0;delete canvas.dataset.rendered}
  scheduleRender()
}
function onScroll() { if(rail.value)scrollTop.value=rail.value.scrollTop }
async function reveal(page:number,focus=false) {
  const wrap=rail.value
  if(!wrap || page<1 || page>props.count)return
  const top=headerHeight.value+(page-1)*rowHeight,bottom=top+rowHeight
  if(top<wrap.scrollTop+headerHeight.value)wrap.scrollTop=(page-1)*rowHeight
  else if(bottom>wrap.scrollTop+wrap.clientHeight)wrap.scrollTop=bottom-wrap.clientHeight+8
  scrollTop.value=wrap.scrollTop
  if(focus){await nextTick();wrap.querySelector<HTMLButtonElement>(`[data-thumbnail-page="${page}"]`)?.focus({preventScroll:true})}
}
async function restoreKeyboardFocus() {
  await nextTick()
  if(!mounted || props.disabled || (!pendingFocusPage && !restoreFocusAfterBusy))return
  if(pendingFocusPage && pendingFocusPage!==props.page)pendingFocusPage=0
  await reveal(props.page,true)
  if(props.disabled)return
  const target=rail.value?.querySelector(`[data-thumbnail-page="${props.page}"]`)
  if(document.activeElement===target){pendingFocusPage=0;restoreFocusAfterBusy=false}
}
async function keydown(event:KeyboardEvent,number:number) {
  if(props.disabled || event.metaKey || event.ctrlKey)return
  if(event.altKey && ['ArrowUp','ArrowDown'].includes(event.key)) {
    event.preventDefault();event.stopPropagation()
    const target=number+(event.key==='ArrowUp'?-1:1)
    if(target>=1 && target<=props.count){pendingFocusPage=target;emit('reorder',number,target)}
    return
  }
  if(event.altKey || event.shiftKey)return
  const step=Math.max(1,Math.floor((railHeight.value-headerHeight.value)/rowHeight))
  const target=event.key==='ArrowUp'?number-1:event.key==='ArrowDown'?number+1:
    event.key==='Home'?1:event.key==='End'?props.count:event.key==='PageUp'?number-step:event.key==='PageDown'?number+step:0
  if(!target && !['Home','ArrowUp','PageUp'].includes(event.key))return
  event.preventDefault();event.stopPropagation()
  const page=Math.min(props.count,Math.max(1,target))
  focusedPage.value=page;emit('select',page);await reveal(page,true)
}
function stopDragScroll(){if(dragFrame)cancelAnimationFrame(dragFrame);dragFrame=0;dragDirection=0}
function dragScroll() {
  dragFrame=0
  if(!props.draggingPage || !rail.value || !dragDirection)return
  rail.value.scrollTop+=dragDirection*14;onScroll()
  dragFrame=requestAnimationFrame(dragScroll)
}
function trackDrag(event:DragEvent) {
  if(!props.draggingPage || !rail.value)return
  const box=rail.value.getBoundingClientRect()
  dragDirection=event.clientY<box.top+headerHeight.value+35?-1:event.clientY>box.bottom-35?1:0
  if(dragDirection && !dragFrame)dragFrame=requestAnimationFrame(dragScroll)
  else if(!dragDirection)stopDragScroll()
}
function leaveDrag(event:DragEvent) {
  if(!(event.relatedTarget instanceof Node) || !(event.currentTarget as HTMLElement).contains(event.relatedTarget))stopDragScroll()
}
watch([start,end],scheduleRender)
watch(() => props.pdf,() => {resetPreviews();void nextTick(() => reveal(props.page))})
watch(() => props.page,page => {
  focusedPage.value=page
  void nextTick(() => reveal(page))
  if(pendingFocusPage===page)void restoreKeyboardFocus()
})
watch(() => props.disabled,disabled => {
  if(disabled){if(rail.value?.contains(document.activeElement))restoreFocusAfterBusy=true;return}
  void restoreKeyboardFocus()
},{flush:'sync'})
watch(() => props.count,() => {void nextTick(() => reveal(props.page));scheduleRender()})
watch(() => props.draggingPage,value => {if(!value)stopDragScroll()})
onMounted(() => {
  mounted=true
  const measure=() => {
    const element=rail.value
    if(!element)return
    railHeight.value=element.clientHeight;headerHeight.value=heading.value?.offsetHeight||48
    if(measuredWidth!==element.clientWidth){measuredWidth=element.clientWidth;resetPreviews()}
    void reveal(props.page)
  }
  observer=new ResizeObserver(measure)
  if(rail.value)observer.observe(rail.value)
  if(heading.value)observer.observe(heading.value)
  measure();scheduleRender()
})
onBeforeUnmount(() => {mounted=false;cancelRender();stopDragScroll();observer?.disconnect();for(const canvas of canvases.values()){canvas.width=0;canvas.height=0};canvases.clear()})
</script>

<template>
  <div ref="rail" class="thumbnail-strip thumbnail-rail virtual-thumbnail-rail" role="navigation" aria-label="Page thumbnails"
    @scroll.passive="onScroll" @dragover="trackDrag" @dragleave="leaveDrag">
    <div ref="heading" class="thumbnail-rail-heading"><span class="thumbnail-hint">{{title}}</span><span>{{count}} pages</span></div>
    <span class="thumbnail-keyboard-help">Use arrow keys to navigate pages. Alt plus up or down moves a page in the working copy.</span>
    <div class="thumbnail-rail-window" :style="{height:`${count*rowHeight}px`}">
      <button v-for="number in visiblePages" :key="number" :data-thumbnail-page="number" class="thumbnail-button"
        :class="{selected:number===page,dragging:draggingPage===number,'drop-target':dropTargetPage===number}"
        :style="{top:`${(number-1)*rowHeight}px`}" :aria-label="`Go to page ${number}`" :aria-current="number===page?'page':undefined"
        :title="`Page ${number} · Drag to reorder. Alt+↑/↓ moves this page.`" :tabindex="number===tabStopPage?0:-1" :draggable="!disabled" :disabled="disabled"
        @focus="focusedPage=number" @keydown="keydown($event,number)" @click="emit('select',number)"
        @dragstart="emit('dragstart',$event,number)" @dragend="stopDragScroll();emit('dragend')" @dragover="emit('dragover',$event,number)" @drop.prevent="stopDragScroll();emit('drop',$event,number)">
        <span class="thumbnail-image"><canvas :ref="value=>registerCanvas(number,value)" :data-page="number" width="1" height="1" :class="{'is-rendered':rendered.has(number)}"></canvas><span v-if="!rendered.has(number)" class="thumbnail-placeholder">{{failed.has(number)?'Preview unavailable':'Loading preview'}}</span></span>
        <span class="thumbnail-page-number">{{number}}</span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.virtual-thumbnail-rail{display:block;position:relative;flex:0 0 148px;width:148px;padding:0 8px;overflow-y:auto;overflow-x:hidden;scrollbar-gutter:stable;overflow-anchor:none;scroll-behavior:auto}
.thumbnail-rail-heading{position:sticky;top:0;z-index:3;display:flex;flex-direction:column;gap:3px;padding:9px 2px;background:var(--panel,#fff);font-size:10px;line-height:1.4;color:var(--muted,#607994)}
.virtual-thumbnail-rail .thumbnail-hint{position:static;margin:0;padding:0;display:-webkit-box;-webkit-line-clamp:2;-webkit-box-orient:vertical;overflow:hidden;overflow-wrap:anywhere;white-space:normal;font-weight:700}
.thumbnail-rail-window{position:relative;width:100%}
.virtual-thumbnail-rail .thumbnail-button{position:absolute;left:0;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:5px;padding:5px;width:100%;height:168px;min-width:0;box-sizing:border-box;flex:none}
.virtual-thumbnail-rail .thumbnail-image{position:relative;display:flex;align-items:center;justify-content:center;flex:none;width:100%;height:132px;min-width:0;overflow:hidden}
.virtual-thumbnail-rail .thumbnail-button canvas{display:none;flex:none;max-width:100%;max-height:100%;object-fit:contain;background:#fff;box-shadow:0 1px 4px #0002}
.virtual-thumbnail-rail .thumbnail-button canvas.is-rendered{display:block}
.virtual-thumbnail-rail .thumbnail-placeholder{font-size:10px;line-height:1.4;white-space:normal;color:var(--muted,#61758d)}
.virtual-thumbnail-rail .thumbnail-page-number{line-height:16px;font-size:11px}
.thumbnail-keyboard-help{position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip-path:inset(50%);white-space:nowrap}
@media(max-width:1000px){.virtual-thumbnail-rail{flex-basis:112px;width:112px;padding:0 5px}}
</style>
