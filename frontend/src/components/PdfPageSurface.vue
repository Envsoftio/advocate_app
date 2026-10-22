<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { TextLayer } from 'pdfjs-dist'
import type { PDFDocumentProxy, PDFPageProxy, RenderTask } from 'pdfjs-dist'
import { friendlyError } from '../lib/format'
import { pdfRasterSize } from '../lib/pdfSurface'
import { resolvePdfDestination, safePdfExternalUrl } from '../lib/pdfNavigation'
import type { PdfSearchFragment } from '../lib/pdfSearch'

type PageReady = {page:number,scale:number,width:number,height:number}
type NativeLink = {id:string,left:number,top:number,width:number,height:number,label:string,page?:number,url?:string}
type MatchRect = {matchIndex:number,left:number,top:number,width:number,height:number}
const props = withDefaults(defineProps<{
  pdf: PDFDocumentProxy | null
  pageNumber: number
  /** PDF.js viewport scale. Numeric 100% zoom is 96 / 72. */
  scale: number
  /** Inactive pages retain their layout footprint but release their bitmap. */
  active?: boolean
  interactive?: boolean
  searchFragments?: PdfSearchFragment[]
  activeMatchIndex?: number
  maxRasterPixels?: number
}>(), {active:true,interactive:true,searchFragments:() => [],activeMatchIndex:-1,maxRasterPixels:16_000_000})
const emit = defineEmits<{
  ready: [value:PageReady]
  busy: [value:boolean]
  error: [message:string]
  internalLink: [page:number]
  externalLink: [url:string]
}>()
const element = ref<HTMLDivElement | null>(null)
const canvas = ref<HTMLCanvasElement | null>(null)
const textContainer = ref<HTMLDivElement | null>(null)
const width = ref(0)
const height = ref(0)
const busy = ref(false)
const painted = ref(false)
const links = ref<NativeLink[]>([])
const rectangles = ref<MatchRect[]>([])
const surfaceStyle = computed(() => width.value && height.value
  ? {width:`${width.value}px`,height:`${height.value}px`} : {})
let generation = 0
let renderTask: RenderTask | null = null
let ownedPage: PDFPageProxy | null = null
let textLayer: TextLayer | null = null
let textDivByItem = new Map<number, HTMLElement>()
let mounted = false
let lastReady: PageReady | null = null
let pendingMatch: number | null = null
let rasterRequest: {pdf:PDFDocumentProxy,page:number,scale:number,width:number,height:number} | null = null

function setBusy(value: boolean) {
  if (busy.value !== value) { busy.value = value; emit('busy', value) }
}
function cancelRendering() {
  generation++
  renderTask?.cancel()
  renderTask = null
  textLayer?.cancel()
  textLayer = null
}
function clearLayers() {
  textContainer.value?.replaceChildren()
  textDivByItem.clear()
  rectangles.value = []
  links.value = []
}
function releaseBitmap() {
  if (canvas.value) { canvas.value.width = 0; canvas.value.height = 0 }
  painted.value = false
  clearLayers()
}
function isCurrent(call: number, pdf: PDFDocumentProxy, page: number) {
  return mounted && call === generation && props.pdf === pdf && props.pageNumber === page
}

async function nativeLinks(page: PDFPageProxy, pdf: PDFDocumentProxy,
    viewport: ReturnType<PDFPageProxy['getViewport']>, call: number) {
  try {
    const annotations = await page.getAnnotations({intent:'display'})
    const result: NativeLink[] = []
    for (const annotation of annotations) {
      if (!isCurrent(call, pdf, page.pageNumber)) return
      if (annotation.subtype !== 'Link' || !Array.isArray(annotation.rect) ||
          annotation.rect.length !== 4 || !annotation.rect.every(Number.isFinite)) continue
      // Ignore launch, JavaScript, named actions, file attachments and remote PDF destinations.
      if (annotation.action || annotation.actions || annotation.attachment || annotation.resetForm) continue
      const url = safePdfExternalUrl(annotation.url)
      let target: number | null = null
      if (!url && annotation.dest) {
        target = await resolvePdfDestination(pdf, annotation.dest, () => !isCurrent(call, pdf, page.pageNumber))
      }
      if (!url && target === null) continue
      const [x1,y1,x2,y2] = viewport.convertToViewportRectangle(annotation.rect)
      const left = Math.max(0, Math.min(x1, x2)), top = Math.max(0, Math.min(y1, y2))
      const right = Math.min(viewport.width, Math.max(x1, x2)), bottom = Math.min(viewport.height, Math.max(y1, y2))
      if (right <= left || bottom <= top) continue
      result.push({id:String(annotation.id ?? result.length),left,top,width:right-left,height:bottom-top,
        label:url ? `Open ${url}` : `Go to PDF page ${target}`, ...(url ? {url} : {page:target!})})
    }
    if (isCurrent(call, pdf, page.pageNumber)) links.value = result
  } catch { /* A damaged link annotation must not prevent reading the page. */ }
}

function updateHighlights() {
  const root = element.value
  if (!root || !painted.value || !props.active) { rectangles.value = []; return }
  const origin = root.getBoundingClientRect()
  if (!origin.width || !origin.height) { rectangles.value = []; return }
  const output: MatchRect[] = []
  for (const fragment of props.searchFragments) {
    const div = textDivByItem.get(fragment.itemIndex)
    const text = div?.firstChild
    if (!text || text.nodeType !== Node.TEXT_NODE || !Number.isInteger(fragment.start) ||
        !Number.isInteger(fragment.end) || fragment.start < 0 || fragment.end <= fragment.start ||
        fragment.end > (text.textContent?.length || 0)) continue
    const range = document.createRange()
    range.setStart(text, fragment.start)
    range.setEnd(text, fragment.end)
    for (const rect of Array.from(range.getClientRects())) {
      if (rect.width <= 0 || rect.height <= 0) continue
      // Convert from screen coordinates, including any enclosing CSS scale.
      output.push({matchIndex:fragment.matchIndex,
        left:(rect.left-origin.left)*width.value/origin.width,
        top:(rect.top-origin.top)*height.value/origin.height,
        width:rect.width*width.value/origin.width,height:rect.height*height.value/origin.height})
    }
    range.detach()
  }
  rectangles.value = output
  if (pendingMatch !== null) void nextTick(() => { if (pendingMatch !== null) scrollToMatch(pendingMatch) })
}

async function selectableText(page: PDFPageProxy, pdf: PDFDocumentProxy,
    viewport: ReturnType<PDFPageProxy['getViewport']>, call: number) {
  try {
    const content = await page.getTextContent()
    if (!isCurrent(call, pdf, page.pageNumber) || !props.active) return
    const staged = document.createElement('div')
    staged.style.setProperty('--scale-factor', String(viewport.scale))
    const layer = new TextLayer({textContentSource:content,container:staged,viewport})
    textLayer = layer
    await layer.render()
    if (!isCurrent(call, pdf, page.pageNumber) || !props.active || !textContainer.value) return
    const container = textContainer.value
    container.style.cssText = staged.style.cssText
    container.setAttribute('data-main-rotation', String(viewport.rotation))
    container.replaceChildren()
    while (staged.firstChild) container.append(staged.firstChild)
    const mapping = new Map<number, HTMLElement>()
    let textIndex = 0
    content.items.forEach((item, itemIndex) => {
      if ('str' in item) {
        const div = layer.textDivs[textIndex++]
        if (div) mapping.set(itemIndex, div)
      }
    })
    textDivByItem = mapping
    await nextTick()
    if (isCurrent(call, pdf, page.pageNumber)) updateHighlights()
  } catch (error) {
    if (isCurrent(call, pdf, page.pageNumber) && !(error instanceof Error &&
        ['AbortException', 'RenderingCancelledException'].includes(error.name)))
      console.warn('Selectable PDF text is unavailable for this page:', error)
  }
}

async function renderSurface() {
  cancelRendering()
  rasterRequest = null
  const call = generation
  const pdf = props.pdf
  const pageNumber = props.pageNumber
  if (!mounted || !pdf || !canvas.value || !Number.isInteger(pageNumber) || pageNumber < 1 || pageNumber > pdf.numPages) {
    releaseBitmap()
    lastReady = null
    setBusy(false)
    return
  }
  if (!props.active) { releaseBitmap(); setBusy(false) }
  else { clearLayers(); setBusy(true) }
  let staged: HTMLCanvasElement | null = null
  try {
    const page = await pdf.getPage(pageNumber)
    if (!isCurrent(call, pdf, pageNumber)) return
    ownedPage = page
    const scale = Number.isFinite(props.scale) && props.scale > 0 ? props.scale : 1
    const viewport = page.getViewport({scale})
    if (!props.active) { width.value = viewport.width; height.value = viewport.height; return }
    const size = pdfRasterSize(viewport.width, viewport.height, window.devicePixelRatio || 1,props.maxRasterPixels)
    rasterRequest = {pdf,page:pageNumber,scale,width:size.width,height:size.height}
    staged = document.createElement('canvas')
    staged.width = size.width
    staged.height = size.height
    const context = staged.getContext('2d')
    if (!context) throw new Error('The PDF canvas could not be created.')
    const task = page.render({canvasContext:context,viewport,
      transform:[size.scaleX,0,0,size.scaleY,0,0]})
    renderTask = task
    await task.promise
    if (!isCurrent(call, pdf, pageNumber) || !props.active || !canvas.value) return
    const destination = canvas.value
    destination.width = size.width
    destination.height = size.height
    const visibleContext = destination.getContext('2d')
    if (!visibleContext) throw new Error('The PDF canvas could not be displayed.')
    visibleContext.drawImage(staged, 0, 0)
    width.value = viewport.width
    height.value = viewport.height
    painted.value = true
    lastReady = {page:pageNumber,scale,width:viewport.width,height:viewport.height}
    // Commit geometry before notifying the parent that editing coordinates are safe.
    await nextTick()
    if (!isCurrent(call, pdf, pageNumber)) return
    emit('ready', lastReady)
    void selectableText(page, pdf, viewport, call)
    void nativeLinks(page, pdf, viewport, call)
  } catch (error) {
    if (isCurrent(call, pdf, pageNumber) && !(error instanceof Error && error.name === 'RenderingCancelledException'))
      emit('error', friendlyError(error))
  } finally {
    if (staged) { staged.width = 0; staged.height = 0 }
    if (call === generation) { renderTask = null; setBusy(false) }
  }
}

function followLink(link: NativeLink) {
  if (!props.interactive || busy.value) return
  if (link.url) emit('externalLink', link.url)
  else if (link.page) emit('internalLink', link.page)
}
function scrollToMatch(index: number): boolean {
  if (!Number.isInteger(index) || index < 0) return false
  pendingMatch = index
  const match = element.value?.querySelector<HTMLElement>(`[data-pdf-match="${Number(index)}"]`)
  if (!match) return false
  match.scrollIntoView({block:'center',inline:'nearest',behavior:'auto'})
  pendingMatch = null
  return true
}
watch(() => [props.pdf,props.pageNumber,props.scale,props.active] as const, () => { void renderSurface() })
watch(() => props.maxRasterPixels, () => {
  const request = rasterRequest
  // A viewport budget change need not discard a sharp bitmap or its selectable
  // text when this page is already smaller than both the old and new limits.
  // Before getPage resolves, renderSurface will read the latest budget itself.
  if (!request || !props.active || request.pdf !== props.pdf || request.page !== props.pageNumber ||
      request.scale !== props.scale || !ownedPage) return
  const viewport = ownedPage.getViewport({scale:request.scale})
  const size = pdfRasterSize(viewport.width,viewport.height,window.devicePixelRatio || 1,props.maxRasterPixels)
  if (size.width !== request.width || size.height !== request.height) void renderSurface()
})
watch(() => props.searchFragments, () => { void nextTick(updateHighlights) }, {deep:true})
onMounted(() => { mounted = true; void renderSurface() })
onBeforeUnmount(() => {
  mounted = false
  const pending=renderTask?.promise, page=ownedPage
  cancelRendering(); releaseBitmap();ownedPage=null
  // PDF.js refuses cleanup while another shared renderer still uses the page.
  void (pending || Promise.resolve()).catch(()=>{}).then(()=>page?.cleanup())
})
defineExpose({getCanvas:() => canvas.value,getTextLayer:() => textContainer.value,getElement:() => element.value,
  getViewport:() => lastReady,scrollToMatch})
</script>

<template>
  <div ref="element" class="pdf-placement-surface pdf-page-surface" :class="{'is-interactive':interactive,'is-rendering':busy}"
    :style="surfaceStyle" :data-page="pageNumber" :aria-busy="busy" role="group" :aria-label="`PDF page ${pageNumber}`">
    <canvas ref="canvas" class="pdf-paper" width="0" height="0" :style="{visibility:painted ? 'visible' : 'hidden'}" :aria-label="`PDF page ${pageNumber}`"></canvas>
    <div ref="textContainer" class="pdf-text-layer textLayer" aria-label="Selectable PDF text"></div>
    <div class="pdf-search-highlights" aria-hidden="true">
      <span v-for="(rectangle,index) in rectangles" :key="index" class="pdf-search-highlight"
        :class="{'is-active':rectangle.matchIndex===activeMatchIndex}" :data-pdf-match="rectangle.matchIndex"
        :style="{left:`${rectangle.left}px`,top:`${rectangle.top}px`,width:`${rectangle.width}px`,height:`${rectangle.height}px`}"></span>
    </div>
    <div v-if="interactive && !busy" class="pdf-native-links" aria-label="PDF links">
      <button v-for="link in links" :key="link.id" class="pdf-native-link" type="button" :title="link.label" :aria-label="link.label"
        :style="{left:`${link.left}px`,top:`${link.top}px`,width:`${link.width}px`,height:`${link.height}px`}"
        @pointerdown.stop @click.stop="followLink(link)"></button>
    </div>
    <slot />
  </div>
</template>

<style scoped>
.pdf-page-surface{position:relative;display:inline-block;flex:none;line-height:0;background:white;isolation:isolate}
.pdf-page-surface.is-rendering{pointer-events:none}
.pdf-page-surface.is-rendering :deep(*){pointer-events:none!important}
.pdf-page-surface>.pdf-paper{display:block;width:100%;height:100%;margin:0}
.pdf-text-layer{position:absolute;inset:0;overflow:hidden;line-height:1;text-align:initial;pointer-events:none;user-select:text;-webkit-text-size-adjust:none;text-size-adjust:none;z-index:1;transform-origin:0 0}
.pdf-text-layer[data-main-rotation="90"]{transform:rotate(90deg) translateY(-100%)}
.pdf-text-layer[data-main-rotation="180"]{transform:rotate(180deg) translate(-100%,-100%)}
.pdf-text-layer[data-main-rotation="270"]{transform:rotate(270deg) translateX(-100%)}
.pdf-text-layer :deep(:is(span,br)){position:absolute;color:transparent;white-space:pre;transform-origin:0 0;cursor:text}
.pdf-text-layer :deep(span.markedContent){top:0;height:0}
.pdf-page-surface.is-interactive:not(.is-rendering)>.pdf-text-layer{pointer-events:auto}
.pdf-text-layer :deep(::selection){background:#499cf077}
.pdf-search-highlights,.pdf-native-links{position:absolute;inset:0;pointer-events:none}
.pdf-search-highlights{z-index:2}
.pdf-search-highlight{position:absolute;background:#ffdc5a66;outline:1px solid #caa52a66;mix-blend-mode:multiply;border-radius:2px}
.pdf-search-highlight.is-active{background:#ff9f1a99;outline:2px solid #a55900}
.pdf-native-links{z-index:2}
.pdf-native-link{position:absolute;pointer-events:auto;background:transparent;border:0;padding:0;cursor:pointer;border-radius:1px}
.pdf-native-link:hover{background:#377ee820;outline:1px solid #377ee8}
.pdf-native-link:focus-visible{outline:2px solid #2465bd;outline-offset:2px;background:#377ee830}
</style>
