<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, reactive, ref, watch } from 'vue'
import { Maximize2, Minimize2, Archive, ArrowDown, ArrowUp, BookMarked, ChevronLeft, ChevronRight, RotateCcw, RotateCw, Trash2, FileSearch, FilePenLine, LayoutGrid, LoaderCircle, Type, Pencil, ScanText, PanelRightOpen, PanelRightClose, Link2, Stamp, ExternalLink, Images, Highlighter, MessageSquare } from 'lucide-vue-next'
import * as pdfjs from 'pdfjs-dist'
import workerSrc from 'pdfjs-dist/build/pdf.worker.min.mjs?url'
import type { PDFDocumentProxy, RenderTask } from 'pdfjs-dist'
import type { MatterDocument, OutputMarkPreview, OutputMarks, PageAnnotation, ReviewedLink } from '../lib/types'
import { api, pageAnnotationAsset, pdfBytes, uploadPageAnnotationImage } from '../lib/api'
import { friendlyError } from '../lib/format'
import { confirmSignatureTransformation } from '../lib/signatures'

pdfjs.GlobalWorkerOptions.workerSrc = workerSrc
const props = defineProps<{ expanded?: boolean, focusMode?: boolean, document: MatterDocument | null, documents?: MatterDocument[], disabled?: boolean, detailsOpen?: boolean, detailsDirty?: boolean, documentPosition?: number, documentCount?: number, bookmarkRefreshKey?: number, pageTarget?: {documentId:string, page:number, sequence:number} }>()
const emit = defineEmits<{ toggleExpanded: [], toggleFocus: [], modified: [], error: [message: string], pageTools: [], toggleDetails: [], ocr: [], moveDocument: [direction: number], archiveDocument: [], openInternalLink: [documentId:string,page:number], openExternalLink: [uri:string] }>()
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
const pageZoom = ref(1)
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
const linkPanelOpen = ref(false)
const linkDrawing = ref(false)
const linkRegion = ref<{x:number,y:number,width:number,height:number} | null>(null)
let linkStart: {x:number,y:number} | null = null
let linkPointerId: number | null = null
const reviewedLinks = ref<ReviewedLink[]>([])
const linksBusy = ref(false)
const linkError = ref('')
const linkNotice = ref('')
const linkDraft = reactive({title:'',kind:'internal' as 'internal'|'external',target_document_id:'',target_page:1,uri:''})
const currentPageLinks = computed(() => reviewedLinks.value.filter(item =>
  item.source_document_id === props.document?.id && item.source_page_index === current.value - 1))
const annotationPanelOpen = ref(false)
const annotationDrawing = ref(false)
const annotationRegion = ref<{x:number,y:number,width:number,height:number} | null>(null)
let annotationStart: {x:number,y:number} | null = null
let annotationPointerId: number | null = null
const pageAnnotations = ref<PageAnnotation[]>([])
const annotationBusy = ref(false)
const annotationError = ref('')
const annotationNotice = ref('')
const annotationImage = ref<File | null>(null)
const annotationFileInput = ref<HTMLInputElement | null>(null)
const annotationImageUrls = reactive<Record<string,string>>({})
const annotationDraft = reactive({kind:'highlight' as 'image'|'highlight'|'comment', body:'', color:'yellow' as PageAnnotation['color']})
const editingAnnotationId = ref<string | null>(null)
const currentPageAnnotations = computed(() => pageAnnotations.value.filter(item =>
  item.document_id === props.document?.id && item.page_index === current.value - 1))
const marksPanelOpen = ref(false)
const marksBusy = ref(false)
const marksError = ref('')
const marksNotice = ref('')
const markPreview = ref<OutputMarkPreview[]>([])
const defaultMarks: OutputMarks = {
  watermark_enabled:false,watermark_text:'DRAFT',watermark_position:'center',
  watermark_font_size:32,watermark_opacity:.16,timestamp_enabled:false,
  timestamp_format:'datetime',timestamp_timezone:'UTC',timestamp_position:'bottom_right',
  timestamp_font_size:8,timestamp_prefix:'Built',
}
const markSettings = reactive<OutputMarks>({...defaultMarks})
let savedMarks = JSON.stringify(defaultMarks)
let markPreviewTimer: ReturnType<typeof setTimeout> | null = null
let markLoadSequence = 0
let markPreviewSequence = 0
const marksDirty = computed(() => JSON.stringify(markSettings) !== savedMarks)
const markPositions = [
  ['top_left','Top left'],['top_center','Top centre'],['top_right','Top right'],
  ['center','Centre'],['bottom_left','Bottom left'],['bottom_center','Bottom centre'],
  ['bottom_right','Bottom right'],
] as const
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
    pageZoom.value = zoom
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
watch(() => `${props.document?.matter_id || ''}|${(props.documents || []).map(item => `${item.id}:${item.revision}:${item.archived_at || ''}`).join(',')}|${props.bookmarkRefreshKey || 0}`, async () => {
  const doc = props.document
  if (!doc) { reviewedLinks.value = []; return }
  try { reviewedLinks.value = await api<ReviewedLink[]>(`/matters/${doc.matter_id}/links`) }
  catch { reviewedLinks.value = [] }
}, {immediate:true})
watch(() => `${props.document?.matter_id || ''}|${props.document?.revision || ''}|${props.bookmarkRefreshKey || 0}`, () => void loadPageAnnotations(), {immediate:true})
watch([current, pageAnnotations], () => void loadCurrentAnnotationImages())
watch(() => props.document?.matter_id || '', async matterId => {
  const sequence = ++markLoadSequence
  if (!matterId) { Object.assign(markSettings, defaultMarks); savedMarks = JSON.stringify(defaultMarks); return }
  try {
    const settings = await api<OutputMarks>(`/matters/${matterId}/output-marks`)
    if (sequence !== markLoadSequence) return
    Object.assign(markSettings, settings)
    savedMarks = JSON.stringify(settings)
  } catch (err) { if (sequence === markLoadSequence) marksError.value = friendlyError(err) }
}, {immediate:true})
watch(() => [marksPanelOpen.value, props.document?.id, props.document?.revision, current.value,
  JSON.stringify(markSettings)] as const, () => scheduleMarkPreview())
watch(current, () => { bookmarkOpen.value = false; bookmarkError.value = ''; bookmarkNotice.value = '' })
watch(current, () => { linkRegion.value = null; linkDrawing.value = false; linkError.value = ''; linkNotice.value = ''; resetAnnotationDraft(false) })
onBeforeUnmount(() => { epoch++; resizeObserver?.disconnect(); thumbnailSequence++; renderTask?.cancel(); thumbnailTasks.forEach(task => task.cancel()); if (markPreviewTimer) clearTimeout(markPreviewTimer); clearAnnotationImageUrls(); if (pdf) void pdf.destroy() })

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

function linkPayload(item: ReviewedLink) {
  return {
    id:item.id,title:item.title,source_document_id:item.source_document_id,
    source_page_index:item.source_page_index,source_revision:item.source_revision,
    rect_x:item.rect_x,rect_y:item.rect_y,rect_width:item.rect_width,
    rect_height:item.rect_height,kind:item.kind,target_document_id:item.target_document_id,
    target_page_index:item.target_page_index,target_revision:item.target_revision,uri:item.uri,
  }
}

function normalizedPointer(event: PointerEvent) {
  const rect = canvas.value?.getBoundingClientRect()
  if (!rect) return null
  return {x:Math.max(0,Math.min(1,(event.clientX-rect.left)/rect.width)),
    y:Math.max(0,Math.min(1,(event.clientY-rect.top)/rect.height))}
}
function beginLinkRegion(event: PointerEvent) {
  if (!linkPanelOpen.value || props.disabled || editing.value || event.button !== 0) return
  const point = normalizedPointer(event)
  if (!point) return
  linkStart = point
  linkPointerId = event.pointerId
  linkDrawing.value = true
  linkRegion.value = {x:point.x,y:point.y,width:0,height:0}
  ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
  event.preventDefault()
}
function updateLinkRegion(event: PointerEvent) {
  if (!linkDrawing.value || linkPointerId !== event.pointerId || !linkStart) return
  const point = normalizedPointer(event)
  if (!point) return
  linkRegion.value = {x:Math.min(linkStart.x,point.x),y:Math.min(linkStart.y,point.y),
    width:Math.abs(point.x-linkStart.x),height:Math.abs(point.y-linkStart.y)}
  event.preventDefault()
}
function finishLinkRegion(event: PointerEvent) {
  if (linkPointerId !== event.pointerId) return
  linkDrawing.value = false
  linkPointerId = null
  linkStart = null
  if (!linkRegion.value || linkRegion.value.width < .01 || linkRegion.value.height < .01) {
    linkRegion.value = null
    linkError.value = 'Drag a visible rectangular link area on the page.'
  } else linkError.value = ''
}

function beginAnnotationRegion(event: PointerEvent) {
  if (!annotationPanelOpen.value || props.disabled || annotationBusy.value || event.button !== 0) return
  const point = normalizedPointer(event)
  if (!point) return
  annotationStart = point
  annotationPointerId = event.pointerId
  annotationDrawing.value = true
  annotationRegion.value = {x:point.x,y:point.y,width:0,height:0}
  ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
  event.preventDefault()
}
function updateAnnotationRegion(event: PointerEvent) {
  if (!annotationDrawing.value || annotationPointerId !== event.pointerId || !annotationStart) return
  const point = normalizedPointer(event)
  if (!point) return
  annotationRegion.value = {x:Math.min(annotationStart.x,point.x),y:Math.min(annotationStart.y,point.y),
    width:Math.abs(point.x-annotationStart.x),height:Math.abs(point.y-annotationStart.y)}
  event.preventDefault()
}
function finishAnnotationRegion(event: PointerEvent) {
  if (annotationPointerId !== event.pointerId) return
  annotationDrawing.value = false
  annotationPointerId = null
  annotationStart = null
  if (!annotationRegion.value || annotationRegion.value.width < .01 || annotationRegion.value.height < .01) {
    annotationRegion.value = null
    annotationError.value = 'Drag a visible rectangular area on the page.'
  } else annotationError.value = ''
}
function clearAnnotationImageUrls() {
  for (const url of Object.values(annotationImageUrls)) URL.revokeObjectURL(url)
  for (const key of Object.keys(annotationImageUrls)) delete annotationImageUrls[key]
}
async function loadPageAnnotations() {
  const doc = props.document
  if (!doc) { pageAnnotations.value = []; clearAnnotationImageUrls(); return }
  try {
    pageAnnotations.value = await api<PageAnnotation[]>(`/matters/${doc.matter_id}/annotations`)
    await loadCurrentAnnotationImages()
  } catch { pageAnnotations.value = []; clearAnnotationImageUrls() }
}
async function loadCurrentAnnotationImages() {
  const ids = new Set(currentPageAnnotations.value.filter(item => item.kind === 'image' && item.resolved).map(item => item.id))
  for (const [id,url] of Object.entries(annotationImageUrls)) {
    if (!ids.has(id)) { URL.revokeObjectURL(url); delete annotationImageUrls[id] }
  }
  await Promise.all([...ids].map(async id => {
    if (annotationImageUrls[id]) return
    try { annotationImageUrls[id] = URL.createObjectURL(await pageAnnotationAsset(id)) } catch { /* list shows missing asset */ }
  }))
}
function resetAnnotationDraft(clearKind = true) {
  annotationRegion.value = null
  annotationDrawing.value = false
  annotationPointerId = null
  annotationStart = null
  annotationDraft.body = ''
  annotationDraft.color = 'yellow'
  if (clearKind) annotationDraft.kind = 'highlight'
  annotationImage.value = null
  if (annotationFileInput.value) annotationFileInput.value.value = ''
  editingAnnotationId.value = null
  annotationError.value = ''
}
function chooseAnnotationImage(event: Event) {
  annotationImage.value = (event.target as HTMLInputElement).files?.[0] || null
}
function editPageAnnotation(item: PageAnnotation) {
  if (!item.resolved) { annotationError.value = 'This annotation needs review because the document revision changed.'; return }
  annotationPanelOpen.value = true
  textMode.value = editTextMode.value = linkPanelOpen.value = marksPanelOpen.value = false
  editingAnnotationId.value = item.id
  annotationDraft.kind = item.kind
  annotationDraft.body = item.body
  annotationDraft.color = item.color
  annotationRegion.value = {x:item.rect_x,y:item.rect_y,width:item.rect_width,height:item.rect_height}
  annotationNotice.value = ''
}
async function savePageAnnotation() {
  const doc = props.document
  const region = annotationRegion.value
  if (!doc || !region || annotationBusy.value) return
  if (annotationDraft.kind === 'comment' && !annotationDraft.body.trim()) {
    annotationError.value = 'Enter comment text.'; return
  }
  if (annotationDraft.kind === 'image' && !editingAnnotationId.value && !annotationImage.value) {
    annotationError.value = 'Choose a PNG or JPEG image.'; return
  }
  annotationBusy.value = true
  annotationError.value = ''
  try {
    if (editingAnnotationId.value) {
      pageAnnotations.value = await api<PageAnnotation[]>(`/annotations/${editingAnnotationId.value}`, 'PATCH', {
        expected_revision:doc.revision,rect_x:region.x,rect_y:region.y,
        rect_width:region.width,rect_height:region.height,
        body:annotationDraft.body,color:annotationDraft.color,
      })
      annotationNotice.value = 'Annotation updated. Existing exports remain unchanged.'
    } else if (annotationDraft.kind === 'image' && annotationImage.value) {
      pageAnnotations.value = await uploadPageAnnotationImage(doc.id, annotationImage.value, {
        page_index:current.value-1,expected_revision:doc.revision,rect_x:region.x,
        rect_y:region.y,rect_width:region.width,rect_height:region.height,
      })
      annotationNotice.value = 'Image overlay saved. It will be flattened into new builds while this revision remains current.'
    } else {
      pageAnnotations.value = await api<PageAnnotation[]>(`/documents/${doc.id}/annotations`, 'POST', {
        kind:annotationDraft.kind,page_index:current.value-1,expected_revision:doc.revision,
        rect_x:region.x,rect_y:region.y,rect_width:region.width,rect_height:region.height,
        body:annotationDraft.body,color:annotationDraft.color,
      })
      annotationNotice.value = `${annotationDraft.kind === 'comment' ? 'Comment' : 'Highlight'} saved as a private review annotation.`
    }
    resetAnnotationDraft()
    await loadCurrentAnnotationImages()
    emit('modified')
  } catch (err) { annotationError.value = friendlyError(err) }
  finally { annotationBusy.value = false }
}
async function removePageAnnotation(item: PageAnnotation) {
  if (annotationBusy.value || !window.confirm(`Remove this ${item.kind}?`)) return
  annotationBusy.value = true
  annotationError.value = ''
  try {
    pageAnnotations.value = await api<PageAnnotation[]>(`/annotations/${item.id}`, 'DELETE')
    resetAnnotationDraft()
    await loadCurrentAnnotationImages()
    emit('modified')
  } catch (err) { annotationError.value = friendlyError(err) }
  finally { annotationBusy.value = false }
}
async function saveReviewedLink() {
  const doc = props.document
  const region = linkRegion.value
  if (!doc || !region || !linkDraft.title.trim() || linksBusy.value) return
  linkError.value = ''
  linksBusy.value = true
  try {
    const existing = await api<ReviewedLink[]>(`/matters/${doc.matter_id}/links`)
    let target: MatterDocument | undefined
    if (linkDraft.kind === 'internal') {
      target = (props.documents || []).find(item => item.id === linkDraft.target_document_id)
      if (!target) throw new Error('Choose a target document.')
      if (!Number.isInteger(linkDraft.target_page) || linkDraft.target_page < 1 || linkDraft.target_page > target.page_count)
        throw new Error(`Choose target page 1 to ${target.page_count}.`)
    }
    const entries = existing.map(linkPayload)
    entries.push({id:crypto.randomUUID().replace(/-/g,''),title:linkDraft.title.trim(),
      source_document_id:doc.id,source_page_index:current.value-1,source_revision:doc.revision,
      rect_x:region.x,rect_y:region.y,rect_width:region.width,rect_height:region.height,
      kind:linkDraft.kind,target_document_id:target?.id || null,
      target_page_index:target ? linkDraft.target_page-1 : null,
      target_revision:target?.revision || null,
      uri:linkDraft.kind === 'external' ? linkDraft.uri.trim() : ''})
    reviewedLinks.value = await api<ReviewedLink[]>(`/matters/${doc.matter_id}/links`, 'PUT', {entries})
    linkRegion.value = null
    linkDraft.title = ''; linkDraft.uri = ''
    linkNotice.value = 'Reviewed link saved. It will export only while its revision-anchored endpoints resolve.'
    emit('modified')
  } catch (err) { linkError.value = friendlyError(err) }
  finally { linksBusy.value = false }
}
async function removeReviewedLink(id: string) {
  const doc = props.document
  if (!doc || linksBusy.value || !window.confirm('Remove this reviewed link?')) return
  linksBusy.value = true
  try {
    const existing = await api<ReviewedLink[]>(`/matters/${doc.matter_id}/links`)
    reviewedLinks.value = await api<ReviewedLink[]>(`/matters/${doc.matter_id}/links`, 'PUT',
      {entries:existing.filter(item => item.id !== id).map(linkPayload)})
    emit('modified')
  } catch (err) { linkError.value = friendlyError(err) }
  finally { linksBusy.value = false }
}
function followReviewedLink(item: ReviewedLink) {
  if (!item.resolved) { linkError.value = 'This link needs review because a source or target revision changed.'; return }
  if (item.kind === 'external') emit('openExternalLink', item.uri)
  else if (item.target_document_id !== null && item.target_page_index !== null)
    emit('openInternalLink', item.target_document_id, item.target_page_index + 1)
}

function scheduleMarkPreview() {
  if (markPreviewTimer) clearTimeout(markPreviewTimer)
  if (!marksPanelOpen.value || !props.document) { markPreview.value = []; return }
  markPreviewTimer = setTimeout(() => void previewMarks(), 250)
}
async function previewMarks() {
  const doc = props.document
  if (!doc || !marksPanelOpen.value) return
  const sequence = ++markPreviewSequence
  marksError.value = ''
  try {
    const result = await api<{revision:number,marks:OutputMarkPreview[]}>(
      `/documents/${doc.id}/pages/${current.value-1}/output-marks-preview`, 'POST', {...markSettings})
    if (sequence === markPreviewSequence && result.revision === doc.revision) markPreview.value = result.marks
  } catch (err) { if (sequence === markPreviewSequence) { markPreview.value = []; marksError.value = friendlyError(err) } }
}
async function saveMarks() {
  const doc = props.document
  if (!doc || marksBusy.value) return
  marksBusy.value = true
  marksError.value = ''
  try {
    const saved = await api<OutputMarks>(`/matters/${doc.matter_id}/output-marks`, 'PUT', {...markSettings})
    Object.assign(markSettings, saved)
    savedMarks = JSON.stringify(saved)
    marksNotice.value = 'Output marks saved. Existing exports remain unchanged; rebuild to apply them.'
    emit('modified')
  } catch (err) { marksError.value = friendlyError(err) }
  finally { marksBusy.value = false }
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
  if (!textMode.value || linkPanelOpen.value || props.disabled || editing.value || loading.value || !canvas.value) return
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
        <div class="toolbar-group reader-document"><FileSearch :size="16"/><div><strong :title="document.title">{{document.title}}</strong><span>Working copy · Original preserved</span></div></div>
        <div class="toolbar-group">
          <button class="icon-btn small" :disabled="current <= 1 || loading" title="Previous page" @click="current--"><ChevronLeft :size="17" /></button>
          <span class="page-counter">{{ current }} / {{ total }}</span>
          <button class="icon-btn small" :disabled="current >= total || loading" title="Next page" @click="current++"><ChevronRight :size="17" /></button>
        </div>
        <div class="toolbar-group"><button class="btn secondary small editor-expand-button" :aria-pressed="!!expanded" :title="expanded ? 'Exit full screen (Esc)' : 'Expand PDF editor to fill the window'" @click="emit('toggleExpanded')"><Minimize2 v-if="expanded" :size="16"/><Maximize2 v-else :size="16"/><span>{{expanded ? 'Exit full screen' : 'Full screen'}}</span></button><button class="icon-btn small" :aria-label="focusMode ? 'Show document list' : 'Focus on PDF'" :title="focusMode ? 'Show document list' : 'Focus on PDF'" :aria-pressed="!!focusMode" @click="emit('toggleFocus')"><PanelRightClose :size="16"/></button><button class="icon-btn small" :aria-label="showThumbnails?'Hide thumbnails':'Show thumbnails'" :aria-pressed="showThumbnails" title="Toggle page thumbnails" @click="showThumbnails=!showThumbnails"><LayoutGrid :size="16"/></button><select v-model.number="scalePct" class="zoom-select" aria-label="Zoom"><option :value="25">25%</option><option :value="50">50%</option><option :value="75">75%</option><option :value="100">100%</option><option :value="125">125%</option><option :value="150">150%</option></select></div>
      </div>
      <div class="page-editbar">
        <div class="editor-tool-group" role="group" aria-label="Read and annotate"><span class="editor-group-label">Read & annotate</span>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-expanded="bookmarkOpen" @click="bookmarkOpen=!bookmarkOpen;bookmarkTitle=document?.title+' — page '+current;bookmarkError=''"><BookMarked :size="15"/> Bookmark page<span v-if="pageBookmarks.length" class="page-bookmark-count">{{pageBookmarks.length}}</span></button>
        </div><div class="editor-tool-group" role="group" aria-label="Edit PDF"><span class="editor-group-label">Edit & mark</span>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="textMode" @click="textMode=!textMode; editTextMode=false; linkPanelOpen=false; marksPanelOpen=false; annotationPanelOpen=false; placement=null"><Type :size="15"/> Add text</button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="editTextMode" @click="editTextMode=!editTextMode; textMode=false; linkPanelOpen=false; marksPanelOpen=false; annotationPanelOpen=false; placement=null"><Pencil :size="15"/> Edit text</button>
        <button class="text-tool" :disabled="editing || loading || disabled" aria-label="Images & notes" title="Image overlays, highlights and comments" :aria-pressed="annotationPanelOpen" @click="annotationPanelOpen=!annotationPanelOpen; textMode=false; editTextMode=false; linkPanelOpen=false; marksPanelOpen=false; placement=null; resetAnnotationDraft()"><Highlighter :size="15"/> Notes<span v-if="currentPageAnnotations.length" class="page-bookmark-count">{{currentPageAnnotations.length}}</span></button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="linkPanelOpen" @click="linkPanelOpen=!linkPanelOpen; textMode=false; editTextMode=false; marksPanelOpen=false; annotationPanelOpen=false; placement=null; linkDraft.target_document_id ||= document.id"><Link2 :size="15"/> Links<span v-if="currentPageLinks.length" class="page-bookmark-count">{{currentPageLinks.length}}</span></button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="marksPanelOpen" @click="marksPanelOpen=!marksPanelOpen; textMode=false; editTextMode=false; linkPanelOpen=false; annotationPanelOpen=false; placement=null"><Stamp :size="15"/> Output marks<span v-if="marksDirty" class="details-unsaved-indicator">Unsaved</span></button>
        </div><div class="editor-tool-group" role="group" aria-label="Organise pages"><span class="editor-group-label">Pages</span>
        <button class="text-tool" :disabled="editing || loading || disabled" title="Open page tools for this document" @click="emit('pageTools')"><FilePenLine :size="15"/> Page tools</button>
        <button class="text-tool" :disabled="editing || loading || disabled" title="Rotate this page 90° counterclockwise in the working PDF" @click="edit('rotate', {page_index:current-1,degrees:270})"><RotateCcw :size="16"/> Rotate left</button>
        <button class="text-tool" :disabled="editing || loading || disabled" title="Rotate this page 90° clockwise in the working PDF" @click="edit('rotate', {page_index:current-1,degrees:90})"><RotateCw :size="16"/> Rotate right</button>
        <button class="text-tool page-icon-tool" :disabled="editing || loading || disabled || current<=1" aria-label="Move page left" title="Move page left" @click="move(-1)"><ChevronLeft :size="16"/></button>
        <button class="text-tool page-icon-tool" :disabled="editing || loading || disabled || current>=total" aria-label="Move page right" title="Move page right" @click="move(1)"><ChevronRight :size="16"/></button>
        <button class="text-tool page-icon-tool danger" :disabled="editing || loading || disabled || total<=1" aria-label="Delete page from working copy" title="Delete page from working copy" @click="deleteCurrent"><Trash2 :size="16"/></button>
        </div>
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
      <Transition name="soft"><div v-if="annotationPanelOpen" class="page-annotation-panel" aria-label="Image overlays, highlights and comments">
        <div class="review-tool-heading"><strong>Images and review notes</strong><span>Draw a region on the page. Images are flattened into new builds; private highlights/comments are excluded by default and recorded in the report.</span></div>
        <div class="annotation-kind-row" role="group" aria-label="Annotation type">
          <button :class="{active:annotationDraft.kind==='image'}" :disabled="!!editingAnnotationId && annotationDraft.kind!=='image'" @click="annotationDraft.kind='image'"><Images :size="15"/> Image overlay</button>
          <button :class="{active:annotationDraft.kind==='highlight'}" :disabled="!!editingAnnotationId && annotationDraft.kind!=='highlight'" @click="annotationDraft.kind='highlight'"><Highlighter :size="15"/> Highlight</button>
          <button :class="{active:annotationDraft.kind==='comment'}" :disabled="!!editingAnnotationId && annotationDraft.kind!=='comment'" @click="annotationDraft.kind='comment'"><MessageSquare :size="15"/> Comment</button>
        </div>
        <div class="annotation-form-grid">
          <label v-if="annotationDraft.kind==='image' && !editingAnnotationId">PNG or JPEG<input ref="annotationFileInput" type="file" accept="image/png,image/jpeg,.png,.jpg,.jpeg" :disabled="annotationBusy || disabled" @change="chooseAnnotationImage"/></label>
          <label v-if="annotationDraft.kind!=='image'">Colour<select v-model="annotationDraft.color" :disabled="annotationBusy || disabled"><option value="yellow">Yellow</option><option value="green">Green</option><option value="blue">Blue</option><option value="pink">Pink</option></select></label>
          <label v-if="annotationDraft.kind!=='image'" class="annotation-note">{{annotationDraft.kind==='comment'?'Comment text':'Optional highlight note'}}<textarea v-model="annotationDraft.body" maxlength="4000" rows="2" :disabled="annotationBusy || disabled"/></label>
        </div>
        <div class="page-tool-actions"><span>{{annotationRegion ? `${Math.round(annotationRegion.width*100)}% × ${Math.round(annotationRegion.height*100)}% region selected` : 'Drag a region on the page'}}</span><div><button v-if="editingAnnotationId" class="btn secondary small" :disabled="annotationBusy" @click="resetAnnotationDraft()">Cancel edit</button><button class="btn secondary small" :disabled="annotationBusy || disabled || !annotationRegion || (annotationDraft.kind==='comment' && !annotationDraft.body.trim()) || (annotationDraft.kind==='image' && !editingAnnotationId && !annotationImage)" @click="savePageAnnotation">{{annotationBusy?'Saving…':editingAnnotationId?'Update annotation':'Save annotation'}}</button></div></div>
        <p v-if="annotationError" class="error-msg" role="alert">{{annotationError}}</p><p v-if="annotationNotice" class="review-tool-notice" role="status">{{annotationNotice}}</p>
        <div v-if="currentPageAnnotations.length" class="page-annotation-list"><div v-for="item in currentPageAnnotations" :key="item.id" :class="['page-annotation-row',{unresolved:!item.resolved}]"><component :is="item.kind==='image'?Images:item.kind==='highlight'?Highlighter:MessageSquare" :size="15"/><span><strong>{{item.kind==='image'?'Image overlay':item.kind==='highlight'?'Highlight':'Comment'}}</strong><small>{{item.resolved ? (item.body || 'No note text') : 'Needs review after a revision change'}}</small></span><button class="icon-btn small" :aria-label="`Edit ${item.kind}`" :disabled="annotationBusy || !item.resolved" @click="editPageAnnotation(item)"><Pencil :size="14"/></button><button class="icon-btn small danger" :aria-label="`Remove ${item.kind}`" :disabled="annotationBusy" @click="removePageAnnotation(item)"><Trash2 :size="14"/></button></div></div>
      </div></Transition>
      <Transition name="soft"><div v-if="linkPanelOpen" class="review-link-panel" aria-label="Reviewed PDF links">
        <div class="review-tool-heading"><strong>Reviewed link</strong><span>Drag a rectangle on this page, then choose its destination. Imported PDF actions remain excluded.</span></div>
        <div class="review-link-grid">
          <label>Readable label<input v-model="linkDraft.title" maxlength="200" :disabled="linksBusy || disabled" placeholder="Open supporting passage"/></label>
          <label>Destination<select v-model="linkDraft.kind" :disabled="linksBusy || disabled"><option value="internal">Matter document page</option><option value="external">Web or email address</option></select></label>
          <template v-if="linkDraft.kind==='internal'">
            <label>Target document<select v-model="linkDraft.target_document_id" :disabled="linksBusy || disabled"><option value="">Choose document</option><option v-for="item in documents || []" :key="item.id" :value="item.id">{{item.title}}</option></select></label>
            <label>Target page<input v-model.number="linkDraft.target_page" type="number" min="1" :max="(documents || []).find(item=>item.id===linkDraft.target_document_id)?.page_count || 1" :disabled="linksBusy || disabled"/></label>
          </template>
          <label v-else class="review-link-uri">HTTPS, HTTP or mailto address<input v-model="linkDraft.uri" maxlength="2000" :disabled="linksBusy || disabled" placeholder="https://example.org/source"/></label>
        </div>
        <div class="page-tool-actions"><span>{{linkRegion ? `${Math.round(linkRegion.width*100)}% × ${Math.round(linkRegion.height*100)}% region selected` : 'Drag a link region on the page'}}</span><button class="btn secondary small" :disabled="linksBusy || disabled || !linkRegion || !linkDraft.title.trim() || (linkDraft.kind==='internal' ? !linkDraft.target_document_id : !linkDraft.uri.trim())" @click="saveReviewedLink">{{linksBusy?'Saving…':'Save reviewed link'}}</button></div>
        <p v-if="linkError" class="error-msg" role="alert">{{linkError}}</p><p v-if="linkNotice" class="review-tool-notice" role="status">{{linkNotice}}</p>
        <div v-if="currentPageLinks.length" class="reviewed-link-list"><div v-for="item in currentPageLinks" :key="item.id" :class="['reviewed-link-row',{unresolved:!item.resolved}]"><button class="reviewed-link-open" :disabled="!item.resolved" @click="followReviewedLink(item)"><ExternalLink v-if="item.kind==='external'" :size="14"/><Link2 v-else :size="14"/><span><strong>{{item.title}}</strong><small>{{item.resolved ? (item.kind==='external' ? item.uri : 'Matter page target') : 'Needs review after a revision change'}}</small></span></button><button class="icon-btn small danger" aria-label="Remove reviewed link" :disabled="linksBusy" @click="removeReviewedLink(item.id)"><Trash2 :size="14"/></button></div></div>
      </div></Transition>
      <Transition name="soft"><div v-if="marksPanelOpen" class="output-marks-panel" aria-label="Output watermark and timestamp settings">
        <div class="review-tool-heading"><strong>Output watermark and timestamp</strong><span>Previewed on the actual working page; inserted only into newly built source-document pages. Originals, working copies and completed exports stay unchanged.</span></div>
        <div class="output-marks-grid">
          <label class="checkbox-line"><input v-model="markSettings.watermark_enabled" type="checkbox"/><span><strong>Watermark</strong><small>Visible technical mark, not court approval.</small></span></label>
          <label>Watermark text<input v-model="markSettings.watermark_text" maxlength="120" :disabled="!markSettings.watermark_enabled"/></label>
          <label>Watermark position<select v-model="markSettings.watermark_position" :disabled="!markSettings.watermark_enabled"><option v-for="item in markPositions" :key="item[0]" :value="item[0]">{{item[1]}}</option></select></label>
          <label>Watermark size<input v-model.number="markSettings.watermark_font_size" type="number" min="8" max="72" :disabled="!markSettings.watermark_enabled"/></label>
          <label>Opacity<input v-model.number="markSettings.watermark_opacity" type="number" min=".05" max=".8" step=".05" :disabled="!markSettings.watermark_enabled"/></label>
          <label class="checkbox-line"><input v-model="markSettings.timestamp_enabled" type="checkbox"/><span><strong>Build timestamp</strong><small>Uses the chosen timezone.</small></span></label>
          <label>Timezone<input v-model="markSettings.timestamp_timezone" maxlength="64" :disabled="!markSettings.timestamp_enabled" placeholder="Asia/Kolkata"/></label>
          <label>Format<select v-model="markSettings.timestamp_format" :disabled="!markSettings.timestamp_enabled"><option value="date">Date</option><option value="datetime">Date, time and zone</option></select></label>
          <label>Timestamp position<select v-model="markSettings.timestamp_position" :disabled="!markSettings.timestamp_enabled"><option v-for="item in markPositions" :key="item[0]" :value="item[0]">{{item[1]}}</option></select></label>
          <label>Timestamp prefix<input v-model="markSettings.timestamp_prefix" maxlength="40" :disabled="!markSettings.timestamp_enabled"/></label>
        </div>
        <div v-if="markPreview.some(item=>item.overlaps_content)" class="notice amber"><span>One or more preview boxes intersect detectable page content. Choose another position or accept only after visual review; signatures and seals cannot be reliably detected.</span></div>
        <div class="page-tool-actions"><span>Preview time is current; the export records one fixed build time.</span><button class="btn secondary small" :disabled="marksBusy || disabled || !marksDirty" @click="saveMarks">{{marksBusy?'Saving…':'Save output marks'}}</button></div>
        <p v-if="marksError" class="error-msg" role="alert">{{marksError}}</p><p v-if="marksNotice" class="review-tool-notice" role="status">{{marksNotice}}</p>
      </div></Transition>
      <Transition name="soft"><div v-if="total > 0 && showThumbnails" ref="thumbnailWrap" class="thumbnail-strip" aria-label="Page thumbnails">
        <span class="thumbnail-hint">Drag to reorder working-copy pages</span>
        <button v-for="number in thumbnailPages" :key="number" class="thumbnail-button" :class="{selected:number===current,dragging:draggingPage===number,'drop-target':dropTargetPage===number}" :aria-label="'Go to page '+number" :draggable="!disabled && !editing" @dragstart="startPageDrag($event,number)" @dragend="draggingPage=null;dropTargetPage=null" @dragover="overPage($event,number)" @drop.prevent="dropPage($event,number)" @click="current=number"><canvas :data-page="number"></canvas><span>{{number}}</span></button>
      </div></Transition>
      <div ref="canvasWrap" class="canvas-scroll" :aria-busy="loading || editing">
        <div v-if="(loading || editing) && !(replacingText && editSnapshot)" class="preview-status"><LoaderCircle :size="18" class="spin"/> {{ editing ? 'Updating PDF…' : 'Loading PDF…' }}</div>
        <div v-if="viewerError" class="preview-status error-msg">{{ viewerError }}</div>
        <div class="pdf-placement-surface" :class="{'placing-text':textMode && !disabled, 'drawing-link':linkPanelOpen && !disabled, 'drawing-annotation':annotationPanelOpen && !disabled, 'is-busy':(loading || editing) && !replacingText, 'is-edit-refreshing':replacingText, 'is-switching':switchingPage, 'is-rotating':rotatingPage}" @click="choosePlacement" @pointerdown="beginLinkRegion($event);beginAnnotationRegion($event)" @pointermove="updateLinkRegion($event);updateAnnotationRegion($event)" @pointerup="finishLinkRegion($event);finishAnnotationRegion($event)" @pointercancel="finishLinkRegion($event);finishAnnotationRegion($event)">
          <canvas ref="canvas" class="pdf-paper" :style="{visibility: viewerError ? 'hidden' : 'visible'}" />
          <img v-if="editSnapshot" class="pdf-edit-snapshot" :class="{'is-fading':editSnapshotFading}" :src="editSnapshot" alt="" aria-hidden="true" />
          <span v-if="placement && placement.page===current" class="pdf-placement-marker" :style="{left:`${placement.x*100}%`,top:`${placement.y*100}%`}" aria-hidden="true"></span>
          <button v-for="item in currentPageLinks" :key="item.id" type="button" :class="['pdf-reviewed-link',{unresolved:!item.resolved}]" :style="{left:`${item.rect_x*100}%`,top:`${item.rect_y*100}%`,width:`${item.rect_width*100}%`,height:`${item.rect_height*100}%`}" :title="item.resolved ? item.title : item.title+' — needs review'" @pointerdown.stop @click.stop="followReviewedLink(item)"><span>{{item.title}}</span></button>
          <span v-if="linkPanelOpen && linkRegion" class="pdf-link-region" :style="{left:`${linkRegion.x*100}%`,top:`${linkRegion.y*100}%`,width:`${linkRegion.width*100}%`,height:`${linkRegion.height*100}%`}" aria-hidden="true"></span>
          <template v-for="item in currentPageAnnotations" :key="item.id">
            <img v-if="item.kind==='image' && item.resolved && annotationImageUrls[item.id]" class="pdf-image-overlay" :src="annotationImageUrls[item.id]" :alt="item.body || 'User-added image overlay'" role="button" tabindex="0" :aria-label="item.body || 'Edit image overlay'" :style="{left:`${item.rect_x*100}%`,top:`${item.rect_y*100}%`,width:`${item.rect_width*100}%`,height:`${item.rect_height*100}%`}" @pointerdown.stop @click.stop="editPageAnnotation(item)" @keydown.enter.stop.prevent="editPageAnnotation(item)" @keydown.space.stop.prevent="editPageAnnotation(item)"/>
            <button v-else-if="item.kind==='highlight'" type="button" :class="['pdf-review-highlight',item.color,{unresolved:!item.resolved}]" :style="{left:`${item.rect_x*100}%`,top:`${item.rect_y*100}%`,width:`${item.rect_width*100}%`,height:`${item.rect_height*100}%`}" :title="item.body || 'Private highlight'" @pointerdown.stop @click.stop="editPageAnnotation(item)"></button>
            <button v-else-if="item.kind==='comment'" type="button" :class="['pdf-review-comment',item.color,{unresolved:!item.resolved}]" :style="{left:`${item.rect_x*100}%`,top:`${item.rect_y*100}%`,width:`${item.rect_width*100}%`,height:`${item.rect_height*100}%`}" :title="item.body" @pointerdown.stop @click.stop="editPageAnnotation(item)"><MessageSquare :size="Math.max(12,Math.min(22,item.rect_width*pageZoom*300))"/></button>
            <button v-else-if="item.kind==='image'" type="button" class="pdf-stale-image-overlay" :style="{left:`${item.rect_x*100}%`,top:`${item.rect_y*100}%`,width:`${item.rect_width*100}%`,height:`${item.rect_height*100}%`}" title="Image overlay needs review after a revision change" @pointerdown.stop @click.stop="editPageAnnotation(item)">Image needs review</button>
          </template>
          <span v-if="annotationPanelOpen && annotationRegion" class="pdf-annotation-region" :style="{left:`${annotationRegion.x*100}%`,top:`${annotationRegion.y*100}%`,width:`${annotationRegion.width*100}%`,height:`${annotationRegion.height*100}%`}" aria-hidden="true"></span>
          <span v-for="item in markPreview" :key="item.kind" :class="['pdf-output-mark-preview',item.kind,{overlap:item.overlaps_content}]" :style="{left:`${item.relative_rect[0]*100}%`,top:`${item.relative_rect[1]*100}%`,width:`${item.relative_rect[2]*100}%`,height:`${item.relative_rect[3]*100}%`,fontSize:`${item.font_size*pageZoom}px`,opacity:item.opacity,textAlign:item.position.endsWith('left')?'left':item.position.endsWith('right')?'right':'center'}"><span>{{item.text}}</span></span>
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
      <div class="document-actionbar" role="group" aria-label="Document tools"><button class="text-tool" :disabled="editing || loading || disabled" @click="emit('ocr')"><ScanText :size="15"/> Run English OCR</button>
        <button class="text-tool page-icon-tool" :disabled="editing || loading || disabled || (documentPosition ?? 0)<=0" aria-label="Move document up" title="Move document up in bundle order" @click="emit('moveDocument',-1)"><ArrowUp :size="16"/></button>
        <button class="text-tool page-icon-tool" :disabled="editing || loading || disabled || (documentPosition ?? 0)>=(documentCount ?? 1)-1" aria-label="Move document down" title="Move document down in bundle order" @click="emit('moveDocument',1)"><ArrowDown :size="16"/></button>
        <button class="text-tool page-icon-tool danger" :disabled="editing || loading || disabled" aria-label="Archive document" title="Archive document" @click="emit('archiveDocument')"><Archive :size="16"/></button>
        <span class="page-editbar-divider" aria-hidden="true"></span>
        <button class="text-tool" :aria-expanded="!!detailsOpen" @click="emit('toggleDetails')"><PanelRightClose v-if="detailsOpen" :size="15"/><PanelRightOpen v-else :size="15"/> Document details<span v-if="detailsDirty" class="details-unsaved-indicator">Unsaved</span></button>
      </div>
    </template>
    <div v-else class="viewer-empty"><FileSearch :size="44"/><strong>Document preview</strong><span>Select a file to review its pages</span></div>
  </div>
</template>
