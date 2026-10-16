<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { Archive, ArrowDown, ArrowUp, BookMarked, ChevronLeft, ChevronRight, RotateCcw, RotateCw, Trash2, FileSearch, FilePenLine, LayoutGrid, LoaderCircle, Type, Pencil, ScanText, PanelRightOpen, PanelRightClose } from 'lucide-vue-next'
import * as pdfjs from 'pdfjs-dist'
import workerSrc from 'pdfjs-dist/build/pdf.worker.min.mjs?url'
import type { PDFDocumentProxy, RenderTask } from 'pdfjs-dist'
import type { MatterDocument } from '../lib/types'
import { api, pdfBytes } from '../lib/api'
import { friendlyError } from '../lib/format'
import { confirmSignatureTransformation } from '../lib/signatures'

pdfjs.GlobalWorkerOptions.workerSrc = workerSrc
const props = defineProps<{ document: MatterDocument | null, disabled?: boolean, detailsOpen?: boolean, detailsDirty?: boolean, documentPosition?: number, documentCount?: number, bookmarkRefreshKey?: number, pageTarget?: {documentId:string, page:number, sequence:number} }>()
const emit = defineEmits<{ modified: [], error: [message: string], pageTools: [], toggleDetails: [], ocr: [], moveDocument: [direction: number], archiveDocument: [] }>()
const canvas = ref<HTMLCanvasElement | null>(null)
const canvasWrap = ref<HTMLDivElement | null>(null)
const thumbnailWrap = ref<HTMLDivElement | null>(null)
const current = ref(1)
const loading = ref(false)
const editing = ref(false)
const switchingPage = ref(false)
const rotatingPage = ref(false)
const viewerError = ref('')
const scalePct = ref(100)
const showThumbnails = ref(false)
const draggingPage = ref<number | null>(null)
const dropTargetPage = ref<number | null>(null)
type ManualBookmark = {id:string,title:string,document_id:string,page_index:number,target_revision:number,parent_id:string|null,resolved:boolean}
const manualBookmarks = ref<ManualBookmark[]>([])
const bookmarkOpen = ref(false)
const bookmarkTitle = ref('')
const bookmarkSaving = ref(false)
const bookmarkError = ref('')
const bookmarkNotice = ref('')
let bookmarkLoadSequence = 0
const pageBookmarks = computed(() => manualBookmarks.value.filter(item =>
  item.document_id === props.document?.id && item.page_index === current.value - 1))
const loadedPageCount = ref(0)
const textMode = ref(false)
const editTextMode = ref(false)
const movingRun = ref<TextRun | null>(null)
const verticalOffset = ref(0)
let pointerRun: TextRun | null = null
let pointerId: number | null = null
let dragStartY = 0
let suppressNextClick = false
const overlayText = ref('')
const overlaySize = ref(12)
const placement = ref<{x:number,y:number,page:number} | null>(null)
type TextRun = { index:number, text:string, relative_bbox:[number,number,number,number] }
const textRuns = ref<TextRun[]>([])
const runsRevision = ref(0)
const runsError = ref('')
const editNotice = ref('')
const selectedRun = ref<TextRun | null>(null)
const replacement = ref('')
const inlineReplacementInput = ref<HTMLTextAreaElement | null>(null)
const editSnapshot = ref('')
const editSnapshotFading = ref(false)
const replacingText = ref(false)
const inlineEditorStyle = computed(() => {
  const box = selectedRun.value?.relative_bbox
  if (!box) return {}
  return {
    ...(box[0] > .5 ? { right: `${(1 - box[2]) * 100}%` } : { left: `${box[0] * 100}%` }),
    ...(box[1] > .62 ? { bottom: `${(1 - box[3]) * 100}%` } : { top: `${box[1] * 100}%` }),
  }
})
let runsSequence = 0
let pdf: PDFDocumentProxy | null = null
let renderTask: RenderTask | null = null
let renderQueue: Promise<void> = Promise.resolve()
let renderRequest = 0
let thumbnailTasks: RenderTask[] = []
let thumbnailSequence = 0
let epoch = 0
let pageRenderSequence = 0
let activeDocumentId: string | null = null
let displayedRevision: number | null = null
let resizeObserver: ResizeObserver | null = null
const total = computed(() => loadedPageCount.value || props.document?.page_count || 0)
const thumbnailPages = computed(() => {
  const start = Math.max(1, Math.min(current.value - 3, total.value - 6))
  return Array.from({length: Math.min(total.value, 7)}, (_, index) => start + index)
})

async function renderThumbnails() {
  const sequence = ++thumbnailSequence
  thumbnailTasks.forEach(task => task.cancel())
  thumbnailTasks = []
  const currentPdf = pdf
  if (!currentPdf || !thumbnailWrap.value) return
  await nextTick()
  if (currentPdf !== pdf || !thumbnailWrap.value) return
  for (const element of thumbnailWrap.value.querySelectorAll<HTMLCanvasElement>('canvas[data-page]')) {
    if (currentPdf !== pdf || sequence !== thumbnailSequence) return
    const number = Number(element.dataset.page)
    try {
      const page = await currentPdf.getPage(number)
      if (currentPdf !== pdf || sequence !== thumbnailSequence) return
      const viewport = page.getViewport({scale: 0.18})
      element.width = Math.round(viewport.width)
      element.height = Math.round(viewport.height)
      const context = element.getContext('2d')
      if (!context) continue
      const task = page.render({canvasContext:context, viewport})
      thumbnailTasks.push(task)
      await task.promise
    } catch (err) {
      if (!(err instanceof Error && err.name === 'RenderingCancelledException')) viewerError.value = friendlyError(err)
    }
  }
}

function render(): Promise<void> {
  const request = ++renderRequest
  renderTask?.cancel()
  const queued = renderQueue.catch(() => {}).then(async () => {
    if (request === renderRequest) await renderPage()
  })
  renderQueue = queued
  return queued
}

async function renderPage() {
  if (!pdf || !canvas.value || !canvasWrap.value) return
  const currentPdf = pdf
  const pageNum = current.value
  try {
    if (renderTask) {
      renderTask.cancel()
      try { await renderTask.promise } catch { /* intentional cancellation */ }
      renderTask = null
    }
    const page = await currentPdf.getPage(pageNum)
    if (pdf !== currentPdf || pageNum !== current.value) return
    const base = page.getViewport({ scale: 1 })
    const limit = Math.max(320, canvasWrap.value.clientWidth - 45)
    const zoom = Math.min(2.2, Math.max(.35, limit / base.width)) * scalePct.value / 100
    const viewport = page.getViewport({ scale: zoom })
    const deviceScale = Math.min(window.devicePixelRatio || 1, 2)
    const element = canvas.value
    element.width = Math.round(viewport.width * deviceScale)
    element.height = Math.round(viewport.height * deviceScale)
    element.style.width = `${viewport.width}px`
    element.style.height = `${viewport.height}px`
    const ctx = element.getContext('2d')
    if (!ctx) return
    const transform: [number,number,number,number,number,number] | undefined =
      deviceScale === 1 ? undefined : [deviceScale, 0, 0, deviceScale, 0, 0]
    renderTask = page.render({ canvasContext: ctx, viewport, transform })
    await renderTask.promise
  } catch (err) {
    if (!(err instanceof Error && err.name === 'RenderingCancelledException'))
      viewerError.value = friendlyError(err)
  }
}

async function loadPdf(doc: MatterDocument | null) {
  if (doc && pdf && doc.id === activeDocumentId && doc.revision === displayedRevision) return
  const call = ++epoch
  const sameDocument = doc?.id === activeDocumentId
  const retainedPage = sameDocument ? current.value : 1
  activeDocumentId = doc?.id ?? null
  loading.value = true
  switchingPage.value = false
  viewerError.value = ''
  current.value = retainedPage
  loadedPageCount.value = 0
  placement.value = null
  selectedRun.value = null
  if (renderTask) { renderTask.cancel(); renderTask = null }
  thumbnailSequence++
  thumbnailTasks.forEach(task => task.cancel())
  thumbnailTasks = []
  if (pdf) { const previous = pdf; pdf = null; void previous.destroy() }
  displayedRevision = null
  if (!doc) { loading.value = false; return }
  try {
    const bytes = await pdfBytes(doc.id, doc.revision)
    if (call !== epoch) return
    pdf = await pdfjs.getDocument({ data: bytes }).promise
    loadedPageCount.value = pdf.numPages
    current.value = Math.min(current.value, pdf.numPages)
    if (!sameDocument && props.pageTarget?.documentId === doc.id) current.value = Math.min(Math.max(1, props.pageTarget.page), pdf.numPages)
    await nextTick()
    if (call !== epoch) return
    await render()
    if (!viewerError.value) displayedRevision = doc.revision
    await renderThumbnails()
  } catch (err) { viewerError.value = friendlyError(err) }
  finally { if (call === epoch) loading.value = false }
}
watch(() => `${props.document?.id || ''}|${props.document?.revision || ''}`, () => void loadPdf(props.document), { immediate: true })
watch(() => props.pageTarget?.sequence, () => {
  const target = props.pageTarget
  if (target && target.documentId === props.document?.id && pdf)
    current.value = Math.min(Math.max(1, target.page), total.value)
})
watch([current, scalePct], async () => {
  const sequence = ++pageRenderSequence
  if (!pdf || loading.value) return
  switchingPage.value = true
  await render()
  if (sequence === pageRenderSequence) switchingPage.value = false
})
watch(canvasWrap, element => {
  resizeObserver?.disconnect()
  resizeObserver = null
  if (!element) return
  resizeObserver = new ResizeObserver(() => void render())
  resizeObserver.observe(element)
})
watch(current, () => void renderThumbnails())
watch(current, () => { placement.value = null })
watch([current, () => props.document?.id, () => props.document?.revision, editTextMode], async () => {
  const sequence = ++runsSequence
  textRuns.value = []
  selectedRun.value = null
  movingRun.value = null
  verticalOffset.value = 0
  pointerRun = null
  pointerId = null
  runsError.value = ''
  if (!editTextMode.value || !props.document) return
  try {
    const result = await api<{revision:number,runs:TextRun[]}>(`/documents/${props.document.id}/pages/${current.value-1}/text-runs`)
    if (sequence !== runsSequence) return
    runsRevision.value = result.revision
    textRuns.value = result.runs
  } catch (err) { if (sequence === runsSequence) runsError.value = friendlyError(err) }
})
watch([current, () => props.document?.id, editTextMode], () => { editNotice.value = '' })
watch(showThumbnails, () => void renderThumbnails())
watch(() => `${props.document?.id || ''}|${props.document?.revision || ''}`, () => {
  bookmarkOpen.value = false
  bookmarkTitle.value = ''
  bookmarkNotice.value = ''
})
watch(() => `${props.document?.id || ''}|${props.document?.revision || ''}|${props.bookmarkRefreshKey || 0}`, async () => {
  const sequence = ++bookmarkLoadSequence
  const doc = props.document
  if (!doc) { manualBookmarks.value = []; return }
  try {
    const entries = await api<ManualBookmark[]>(`/matters/${doc.matter_id}/bookmarks`)
    if (sequence === bookmarkLoadSequence) manualBookmarks.value = entries
  } catch { if (sequence === bookmarkLoadSequence) manualBookmarks.value = [] }
}, {immediate:true})
watch(current, () => { bookmarkOpen.value = false; bookmarkError.value = ''; bookmarkNotice.value = '' })
onBeforeUnmount(() => { epoch++; resizeObserver?.disconnect(); thumbnailSequence++; renderTask?.cancel(); thumbnailTasks.forEach(task => task.cancel()); if (pdf) void pdf.destroy() })

async function addPageBookmark() {
  const doc = props.document
  const title = bookmarkTitle.value.trim()
  const pageIndex = current.value - 1
  if (!doc || !title || bookmarkSaving.value || props.disabled) return
  bookmarkSaving.value = true
  bookmarkError.value = ''
  try {
    const existing = await api<ManualBookmark[]>(`/matters/${doc.matter_id}/bookmarks`)
    const entries = existing.map(({id,title,document_id,page_index,target_revision,parent_id}) =>
      ({id,title,document_id,page_index,target_revision,parent_id}))
    entries.push({id:crypto.randomUUID().replace(/-/g,''),title,
      document_id:doc.id,page_index:pageIndex,target_revision:doc.revision,parent_id:null})
    manualBookmarks.value = await api<ManualBookmark[]>(`/matters/${doc.matter_id}/bookmarks`, 'PUT', {entries})
    bookmarkOpen.value = false
    bookmarkTitle.value = ''
    bookmarkNotice.value = `Bookmarked page ${pageIndex + 1}. Review its destination under Index & bookmarks.`
    emit('modified')
  } catch (err) { bookmarkError.value = friendlyError(err) }
  finally { bookmarkSaving.value = false }
}

async function edit(action: 'rotate' | 'delete' | 'reorder', params: unknown, focusPage?: number): Promise<boolean> {
  if (!props.document || editing.value) return false
  const signature = confirmSignatureTransformation([props.document],
    action === 'rotate' ? 'Rotating a page' : action === 'delete' ? 'Deleting a page' : 'Reordering pages')
  if (!signature.proceed) return false
  const rotating = action === 'rotate'
  const startedAt = performance.now()
  editing.value = true
  if (rotating) rotatingPage.value = true
  try {
    const updated = await api<MatterDocument>(`/documents/${props.document.id}/pages/${action}`, 'POST', {
      ...(params as Record<string, unknown>), acknowledge_signature_risk: signature.acknowledged,
    })
    if (rotating) {
      const remaining = 220 - (performance.now() - startedAt)
      if (remaining > 0 && !window.matchMedia('(prefers-reduced-motion: reduce)').matches)
        await new Promise(resolve => window.setTimeout(resolve, remaining))
      await loadPdf(updated)
    }
    if (focusPage) current.value = focusPage
    emit('modified')
    return true
  } catch (err) { emit('error', friendlyError(err)); return false }
  finally { editing.value = false; if (rotating) rotatingPage.value = false }
}
function move(step: number) {
  if (!props.document || total.value < 2) return
  const from = current.value - 1
  const to = from + step
  if (to < 0 || to >= total.value) return
  const order = Array.from({ length: total.value }, (_, i) => i)
  ;[order[from], order[to]] = [order[to], order[from]]
  void edit('reorder', { order }, to + 1)
}
function startPageDrag(event: DragEvent, number: number) {
  if (editing.value || loading.value || props.disabled) { event.preventDefault(); return }
  draggingPage.value = number
  event.dataTransfer?.setData('text/plain', String(number))
  if (event.dataTransfer) event.dataTransfer.effectAllowed = 'move'
}
function overPage(event: DragEvent, number: number) {
  if (!draggingPage.value || draggingPage.value === number) return
  event.preventDefault()
  if (event.dataTransfer) event.dataTransfer.dropEffect = 'move'
  dropTargetPage.value = number
}
function dropPage(event: DragEvent, target: number) {
  const source = draggingPage.value || Number(event.dataTransfer?.getData('text/plain'))
  draggingPage.value = null
  dropTargetPage.value = null
  if (!Number.isInteger(source) || source < 1 || source > total.value || source === target) return
  const order = Array.from({length: total.value}, (_, index) => index)
  order.splice(source - 1, 1)
  const targetIndex = order.indexOf(target - 1)
  order.splice(targetIndex + (source < target ? 1 : 0), 0, source - 1)
  void edit('reorder', {order}, target)
}
function deleteCurrent() {
  if (total.value <= 1 || !window.confirm(`Remove page ${current.value} from the working copy? The original remains intact.`)) return
  void edit('delete', { page_index: current.value - 1 })
}
function choosePlacement(event: MouseEvent) {
  if (!textMode.value || props.disabled || editing.value || loading.value || !canvas.value) return
  const rect = canvas.value.getBoundingClientRect()
  const x = (event.clientX - rect.left) / rect.width
  const y = (event.clientY - rect.top) / rect.height
  if (x >= 0 && x < 1 && y >= 0 && y < 1) placement.value = { x, y, page: current.value }
}
async function placeText() {
  if (!props.document || !placement.value || !overlayText.value.trim() || editing.value || props.disabled) return
  const signature = confirmSignatureTransformation([props.document], 'Adding visible text')
  if (!signature.proceed) return
  editing.value = true
  try {
    await api(`/documents/${props.document.id}/pages/text-overlay`, 'POST', {
      page_index: placement.value.page - 1, x: placement.value.x, y: placement.value.y,
      text: overlayText.value, font_size: overlaySize.value,
      acknowledge_signature_risk: signature.acknowledged,
    })
    placement.value = null
    overlayText.value = ''
    emit('modified')
  } catch (err) { emit('error', friendlyError(err)) }
  finally { editing.value = false }
}
async function selectRun(run: TextRun) {
  if (props.disabled || editing.value) return
  selectedRun.value = run
  replacement.value = run.text
  runsError.value = ''
  editNotice.value = ''
  await nextTick()
  inlineReplacementInput.value?.focus()
  inlineReplacementInput.value?.select()
}
function cancelReplacement() {
  selectedRun.value = null
  replacement.value = ''
  runsError.value = ''
}
function beginMove(event: PointerEvent, run: TextRun) {
  if (event.button !== 0 || props.disabled || editing.value || loading.value || !canvas.value) return
  pointerRun = run
  pointerId = event.pointerId
  dragStartY = event.clientY
  ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
}
function updateMove(event: PointerEvent) {
  if (!pointerRun || pointerId !== event.pointerId || !canvas.value) return
  if (!movingRun.value && Math.abs(event.clientY - dragStartY) < 5) return
  if (!movingRun.value) {
    movingRun.value = pointerRun
    selectedRun.value = null
    runsError.value = ''
    editNotice.value = ''
  }
  const box = pointerRun.relative_bbox
  const offset = (event.clientY - dragStartY) / canvas.value.getBoundingClientRect().height
  verticalOffset.value = Math.max(-box[1], Math.min(1 - box[3], offset))
  event.preventDefault()
}
function finishMove(event: PointerEvent) {
  if (pointerId !== event.pointerId) return
  pointerId = null
  pointerRun = null
  if (!movingRun.value) return
  const run = movingRun.value
  const offset = verticalOffset.value
  movingRun.value = null
  verticalOffset.value = 0
  suppressNextClick = true
  window.setTimeout(() => { suppressNextClick = false }, 0)
  if (event.type === 'pointerup' && Math.abs(offset) >= .0001) void applyMove(run, offset)
}
function clickRun(run: TextRun) {
  if (suppressNextClick) { suppressNextClick = false; return }
  void selectRun(run)
}
async function applyMove(run: TextRun, offset: number) {
  if (!props.document || editing.value || props.disabled) return
  const signature = confirmSignatureTransformation([props.document], 'Moving visible text')
  if (!signature.proceed) return
  try { editSnapshot.value = canvas.value?.toDataURL('image/png') || '' } catch { editSnapshot.value = '' }
  editSnapshotFading.value = false
  replacingText.value = true
  editing.value = true
  runsError.value = ''
  try {
    const updated = await api<MatterDocument>(`/documents/${props.document.id}/pages/move-text-run`, 'POST', {
      page_index: current.value - 1, span_index: run.index,
      expected_revision: runsRevision.value, expected_text: run.text,
      vertical_offset: offset,
      acknowledge_signature_risk: signature.acknowledged,
    })
    await loadPdf(updated)
    if (!viewerError.value && editSnapshot.value) {
      editSnapshotFading.value = true
      if (!window.matchMedia('(prefers-reduced-motion: reduce)').matches)
        await new Promise(resolve => window.setTimeout(resolve, 200))
    }
    editNotice.value = 'Text moved in the working copy. Review the page; use Undo to revert it.'
    emit('modified')
  } catch (err) { runsError.value = friendlyError(err) }
  finally { editing.value = false; replacingText.value = false; editSnapshot.value = '' }
}
async function applyReplacement() {
  if (!props.document || !selectedRun.value || !replacement.value.trim() || props.disabled || editing.value) return
  const signature = confirmSignatureTransformation([props.document], 'Replacing visible text')
  if (!signature.proceed) return
  editing.value = true
  replacingText.value = true
  runsError.value = ''
  try {
    const result = await api<MatterDocument & {edit_layout:{font_size:number,line_count:number,font_size_reduced:boolean}}>(`/documents/${props.document.id}/pages/replace-text-run`, 'POST', {
      page_index: current.value - 1, span_index: selectedRun.value.index,
      expected_revision: runsRevision.value, expected_text: selectedRun.value.text,
      replacement: replacement.value,
      acknowledge_signature_risk: signature.acknowledged,
    })
    editNotice.value = `Applied in ${result.edit_layout.line_count} line(s) at ${result.edit_layout.font_size} pt${result.edit_layout.font_size_reduced ? ' (reduced to fit)' : ''}. Review the page.`
    selectedRun.value = null
    try { editSnapshot.value = canvas.value?.toDataURL('image/png') || '' } catch { editSnapshot.value = '' }
    editSnapshotFading.value = false
    await loadPdf(result)
    if (!viewerError.value && editSnapshot.value) {
      editSnapshotFading.value = true
      if (!window.matchMedia('(prefers-reduced-motion: reduce)').matches)
        await new Promise(resolve => window.setTimeout(resolve, 200))
    }
    editSnapshot.value = ''
    emit('modified')
  } catch (err) { runsError.value = friendlyError(err) }
  finally { editing.value = false; replacingText.value = false; editSnapshot.value = '' }
}
</script>

<template>
  <div class="viewer-root">
    <template v-if="document">
      <div class="viewer-toolbar">
        <div class="toolbar-group"><FileSearch :size="16"/><span class="toolbar-label">PDF PREVIEW</span></div>
        <div class="toolbar-group">
          <button class="icon-btn small" :disabled="current <= 1 || loading" title="Previous page" @click="current--"><ChevronLeft :size="17" /></button>
          <span class="page-counter">{{ current }} / {{ total }}</span>
          <button class="icon-btn small" :disabled="current >= total || loading" title="Next page" @click="current++"><ChevronRight :size="17" /></button>
        </div>
        <div class="toolbar-group"><button class="icon-btn small" :aria-label="showThumbnails?'Hide thumbnails':'Show thumbnails'" :aria-pressed="showThumbnails" title="Toggle page thumbnails" @click="showThumbnails=!showThumbnails"><LayoutGrid :size="16"/></button><select v-model.number="scalePct" class="zoom-select" aria-label="Zoom"><option :value="25">25%</option><option :value="50">50%</option><option :value="75">75%</option><option :value="100">100%</option><option :value="125">125%</option><option :value="150">150%</option></select></div>
      </div>
      <div class="page-editbar">
        <button class="text-tool" :disabled="editing || loading || disabled" title="Open page tools for this document" @click="emit('pageTools')"><FilePenLine :size="15"/> Page tools</button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-expanded="bookmarkOpen" @click="bookmarkOpen=!bookmarkOpen;bookmarkTitle=document?.title+' — page '+current;bookmarkError=''"><BookMarked :size="15"/> Bookmark page<span v-if="pageBookmarks.length" class="page-bookmark-count">{{pageBookmarks.length}}</span></button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="textMode" @click="textMode=!textMode; editTextMode=false; placement=null"><Type :size="15"/> Add text</button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="editTextMode" @click="editTextMode=!editTextMode; textMode=false; placement=null"><Pencil :size="15"/> Edit text</button>
        <button class="text-tool" :disabled="editing || loading || disabled" title="Rotate this page 90° counterclockwise in the working PDF" @click="edit('rotate', {page_index:current-1,degrees:270})"><RotateCcw :size="16"/> Rotate left</button>
        <button class="text-tool" :disabled="editing || loading || disabled" title="Rotate this page 90° clockwise in the working PDF" @click="edit('rotate', {page_index:current-1,degrees:90})"><RotateCw :size="16"/> Rotate right</button>
        <button class="text-tool page-icon-tool" :disabled="editing || loading || disabled || current<=1" aria-label="Move page left" title="Move page left" @click="move(-1)"><ChevronLeft :size="16"/></button>
        <button class="text-tool page-icon-tool" :disabled="editing || loading || disabled || current>=total" aria-label="Move page right" title="Move page right" @click="move(1)"><ChevronRight :size="16"/></button>
        <button class="text-tool page-icon-tool danger" :disabled="editing || loading || disabled || total<=1" aria-label="Delete page from working copy" title="Delete page from working copy" @click="deleteCurrent"><Trash2 :size="16"/></button>
        <span class="page-editbar-divider" aria-hidden="true"></span>
        <button class="text-tool" :disabled="editing || loading || disabled" @click="emit('ocr')"><ScanText :size="15"/> Run English OCR</button>
        <button class="text-tool page-icon-tool" :disabled="editing || loading || disabled || (documentPosition ?? 0)<=0" aria-label="Move document up" title="Move document up in bundle order" @click="emit('moveDocument',-1)"><ArrowUp :size="16"/></button>
        <button class="text-tool page-icon-tool" :disabled="editing || loading || disabled || (documentPosition ?? 0)>=(documentCount ?? 1)-1" aria-label="Move document down" title="Move document down in bundle order" @click="emit('moveDocument',1)"><ArrowDown :size="16"/></button>
        <button class="text-tool page-icon-tool danger" :disabled="editing || loading || disabled" aria-label="Archive document" title="Archive document" @click="emit('archiveDocument')"><Archive :size="16"/></button>
        <span class="page-editbar-divider" aria-hidden="true"></span>
        <button class="text-tool" :aria-expanded="!!detailsOpen" @click="emit('toggleDetails')"><PanelRightClose v-if="detailsOpen" :size="15"/><PanelRightOpen v-else :size="15"/> Document details<span v-if="detailsDirty" class="details-unsaved-indicator">Unsaved</span></button>
      </div>
      <div v-if="pageBookmarks.length" class="page-bookmarks-present" role="region" :aria-label="`Bookmarks on page ${current}`">
        <BookMarked :size="15" aria-hidden="true"/>
        <strong>Page {{current}} bookmarks</strong>
        <span v-for="item in pageBookmarks" :key="item.id" class="page-bookmark-chip">{{item.title}}<span v-if="!item.resolved" class="page-bookmark-review"> · needs review</span></span>
      </div>
      <div v-if="bookmarkOpen" class="page-bookmark-editor" aria-label="Bookmark current page">
        <strong>Bookmark {{document.title}}, page {{current}}</strong>
        <label>Bookmark title<input v-model="bookmarkTitle" maxlength="200" :disabled="bookmarkSaving || disabled" @keydown.enter.prevent="addPageBookmark"/></label>
        <button class="btn primary small" :disabled="bookmarkSaving || disabled || !bookmarkTitle.trim()" @click="addPageBookmark">{{bookmarkSaving ? 'Saving…' : 'Add bookmark'}}</button>
        <p v-if="bookmarkError" class="error-msg" role="alert">{{bookmarkError}}</p>
      </div>
      <p v-if="bookmarkNotice" class="page-bookmark-notice" role="status">{{bookmarkNotice}}</p>
      <Transition name="soft"><div v-if="detailsDirty" class="preview-draft-warning" role="status">Save document details to use PDF tools.<button v-if="!detailsOpen" class="text-tool" @click="emit('toggleDetails')">Open details</button></div></Transition>
      <Transition name="soft"><div v-if="textMode" class="text-overlay-panel">
        <strong>Visible text overlay</strong>
        <p>Type printable English text, then click its top-left point on the page. This adds content to a working copy; it does not change existing text. Review overlap and spacing before export.</p>
        <label>Text<textarea v-model="overlayText" maxlength="1000" rows="3" :disabled="editing || disabled"/></label>
        <label>Size (pt)<input v-model.number="overlaySize" type="number" min="6" max="36" :disabled="editing || disabled"/></label>
        <div class="page-tool-actions"><span>{{placement ? `Page ${placement.page} · ${Math.round(placement.x*100)}%, ${Math.round(placement.y*100)}%` : 'Click a position on the page'}}</span><button class="btn secondary small" :disabled="!placement || !overlayText.trim() || editing || disabled" @click="placeText">Apply text</button></div>
      </div></Transition>
      <Transition name="soft"><div v-if="editTextMode" class="text-overlay-panel" aria-label="Edit existing text">
        <strong>Edit selectable text</strong>
        <p>Click highlighted text to edit it, or drag it up or down to move it. A move saves when you release the text. Review the page afterward; scanned text cannot be moved. This is not a confidential redaction tool.</p>
        <p v-if="runsError && !selectedRun" class="error-msg" role="alert">{{runsError}}</p>
        <p class="edit-status-slot" role="status">{{editNotice}}</p>
        <p v-if="!textRuns.length && !editNotice">No selectable horizontal text found on this page. Scanned text needs OCR and review.</p>
      </div></Transition>
      <Transition name="soft"><div v-if="total > 0 && showThumbnails" ref="thumbnailWrap" class="thumbnail-strip" aria-label="Page thumbnails">
        <span class="thumbnail-hint">Drag to reorder working-copy pages</span>
        <button v-for="number in thumbnailPages" :key="number" class="thumbnail-button" :class="{selected:number===current,dragging:draggingPage===number,'drop-target':dropTargetPage===number}" :aria-label="'Go to page '+number" :draggable="!disabled && !editing" @dragstart="startPageDrag($event,number)" @dragend="draggingPage=null;dropTargetPage=null" @dragover="overPage($event,number)" @drop.prevent="dropPage($event,number)" @click="current=number"><canvas :data-page="number"></canvas><span>{{number}}</span></button>
      </div></Transition>
      <div ref="canvasWrap" class="canvas-scroll" :aria-busy="loading || editing">
        <div v-if="(loading || editing) && !(replacingText && editSnapshot)" class="preview-status"><LoaderCircle :size="18" class="spin"/> {{ editing ? 'Updating PDF…' : 'Loading PDF…' }}</div>
        <div v-if="viewerError" class="preview-status error-msg">{{ viewerError }}</div>
        <div class="pdf-placement-surface" :class="{'placing-text':textMode && !disabled, 'is-busy':(loading || editing) && !replacingText, 'is-edit-refreshing':replacingText, 'is-switching':switchingPage, 'is-rotating':rotatingPage}" @click="choosePlacement">
          <canvas ref="canvas" class="pdf-paper" :style="{visibility: viewerError ? 'hidden' : 'visible'}" />
          <img v-if="editSnapshot" class="pdf-edit-snapshot" :class="{'is-fading':editSnapshotFading}" :src="editSnapshot" alt="" aria-hidden="true" />
          <span v-if="placement && placement.page===current" class="pdf-placement-marker" :style="{left:`${placement.x*100}%`,top:`${placement.y*100}%`}" aria-hidden="true"></span>
          <template v-if="editTextMode">
            <button v-for="run in textRuns" :key="run.index" class="pdf-text-run" :class="{selected:selectedRun?.index===run.index, 'move-target':movingRun?.index===run.index}"
              :style="{left:`${run.relative_bbox[0]*100}%`,top:`${(run.relative_bbox[1]+(movingRun?.index===run.index ? verticalOffset : 0))*100}%`,width:`${(run.relative_bbox[2]-run.relative_bbox[0])*100}%`,height:`${(run.relative_bbox[3]-run.relative_bbox[1])*100}%`}"
              :aria-label="`Edit text: ${run.text}`" :disabled="disabled || editing" @click.stop="clickRun(run)" @pointerdown="beginMove($event, run)" @pointermove="updateMove" @pointerup="finishMove" @pointercancel="finishMove"><span v-if="movingRun?.index===run.index" class="pdf-move-label">{{run.text}}</span></button>
          </template>
          <div v-if="editTextMode && selectedRun" class="pdf-inline-editor" :style="inlineEditorStyle" @click.stop>
            <label :for="`replacement-${selectedRun.index}`">Replacement</label>
            <textarea :id="`replacement-${selectedRun.index}`" ref="inlineReplacementInput" v-model="replacement" maxlength="2000" rows="2" :disabled="editing || disabled" aria-describedby="inline-edit-hint" @keydown.esc.stop.prevent="cancelReplacement" @keydown.meta.enter.prevent="applyReplacement" @keydown.ctrl.enter.prevent="applyReplacement"></textarea>
            <span id="inline-edit-hint" class="pdf-inline-hint">Apply change · Esc to cancel</span>
            <p v-if="runsError" class="error-msg" role="alert">{{runsError}}</p>
            <div class="pdf-inline-actions">
              <button class="btn secondary small" :disabled="editing" @click="cancelReplacement">Cancel</button>
              <button class="btn secondary small" :disabled="!replacement.trim() || editing || disabled" @click="applyReplacement">Apply replacement</button>
            </div>
          </div>
        </div>
      </div>
    </template>
    <div v-else class="viewer-empty"><FileSearch :size="44"/><strong>Document preview</strong><span>Select a file to review its pages</span></div>
  </div>
</template>
