<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, reactive, ref, watch } from 'vue'
import { Maximize2, Minimize2, Archive, BookMarked, ChevronLeft, ChevronRight, RotateCcw, RotateCw, Trash2, FileSearch, FilePenLine, LayoutGrid, LoaderCircle, Type, Pencil, ScanText, PanelRightOpen, PanelRightClose, Link2, Stamp, ExternalLink, Images, Highlighter, MessageSquare, Eraser, Undo2, Redo2, X, Search } from 'lucide-vue-next'
import * as pdfjs from 'pdfjs-dist'
import workerSrc from 'pdfjs-dist/build/pdf.worker.min.mjs?url'
import type { PDFDocumentProxy, RenderTask } from 'pdfjs-dist'
import type { MatterDocument, OutputMarkPreview, OutputMarks, PageAnnotation, ReviewedLink } from '../lib/types'
import { api, pageAnnotationAsset, pdfDocumentSource, uploadPageAnnotationImage, uploadWorkingPageImage } from '../lib/api'
import { friendlyError } from '../lib/format'
import { confirmSignatureTransformation } from '../lib/signatures'

pdfjs.GlobalWorkerOptions.workerSrc = workerSrc
const props = defineProps<{ expanded?: boolean, focusMode?: boolean, document: MatterDocument | null, documents?: MatterDocument[], disabled?: boolean, canUndo?: boolean, canRedo?: boolean, detailsOpen?: boolean, detailsDirty?: boolean, bookmarkRefreshKey?: number, pageTarget?: {documentId:string, page:number, sequence:number} }>()
const emit = defineEmits<{ toggleExpanded: [], toggleFocus: [], modified: [], undo: [], redo: [], error: [message: string], pageTools: [], toggleDetails: [], ocr: [language: 'eng' | 'hin' | 'eng+hin'], archiveDocument: [], openInternalLink: [documentId:string,page:number], openExternalLink: [uri:string] }>()
const ocrLanguage = ref<'eng' | 'hin' | 'eng+hin'>('eng')
const canvas = ref<HTMLCanvasElement | null>(null)
const textLayerContainer = ref<HTMLDivElement | null>(null)
const canvasWrap = ref<HTMLDivElement | null>(null)
const thumbnailWrap = ref<HTMLDivElement | null>(null)
const current = ref(1)
const pageJump = ref(1)
const loading = ref(false)
const editing = ref(false)
const switchingPage = ref(false)
const rotatingPage = ref(false)
const viewerError = ref('')
const scalePct = ref(100)
const zoomLevels = [25, 50, 75, 100, 125, 150]
function zoomBy(step: number) {
  const currentIndex = zoomLevels.indexOf(scalePct.value)
  scalePct.value = zoomLevels[Math.max(0, Math.min(zoomLevels.length - 1,
    (currentIndex < 0 ? 3 : currentIndex) + step))]
}
const pageZoom = ref(1)
const showThumbnails = ref(true)
type EditorCategory = 'edit' | 'review' | 'pages' | 'output'
const activeCategory = ref<EditorCategory>('edit')
type DocumentSearchMatch = {document_id:string,title:string,revision:number,page:number,snippet:string}
const findOpen = ref(false)
const findInput = ref<HTMLInputElement | null>(null)
const findQuery = ref('')
const findMatches = ref<DocumentSearchMatch[]>([])
const findTruncated = ref(false)
const findBusy = ref(false)
const findError = ref('')
const findIndex = ref(0)
let findSequence = 0
let findTimer: ReturnType<typeof setTimeout> | null = null
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
type EditorTool = 'select' | 'add-text' | 'edit-text' | 'redact' | 'notes' | 'links' | 'marks'
const activeTool = ref<EditorTool>('select')
const textMode = computed(() => activeTool.value === 'add-text')
const editTextMode = computed(() => activeTool.value === 'edit-text')
const movingRun = ref<TextRun | null>(null)
const verticalOffset = ref(0)
const pendingMove = ref(false)
let pointerRun: TextRun | null = null
let pointerId: number | null = null
let dragStartY = 0
let suppressNextClick = false
const overlayText = ref('')
const overlaySize = ref(12)
const overlayError = ref('')
const overlayValidation = computed(() => {
  if (!overlayText.value.trim()) return 'Enter text to add.'
  if (/[^\x20-\x7E\n]/.test(overlayText.value)) return 'Add text currently supports printable ASCII and line breaks only.'
  if (!Number.isFinite(overlaySize.value) || overlaySize.value < 6 || overlaySize.value > 36)
    return 'Choose a font size from 6 to 36 pt.'
  return ''
})
watch([overlayText, overlaySize], () => { overlayError.value = '' })
const placement = ref<{x:number,y:number,page:number} | null>(null)
const inlineAddInput = ref<HTMLTextAreaElement | null>(null)
const inlineAddStyle = computed(() => {
  const point = placement.value
  if (!point) return {}
  return {
    left: `${Math.min(point.x * 100, 60)}%`,
    ...(point.y > .65 ? {bottom:`${Math.max(0,(1-point.y)*100)}%`} : {top:`${Math.min(95,point.y*100+3)}%`}),
  }
})
type TextRun = { index:number, text:string, relative_bbox:[number,number,number,number] }
const textRuns = ref<TextRun[]>([])
const runsRevision = ref(0)
const runsError = ref('')
const editNotice = ref('')
const selectedRun = ref<TextRun | null>(null)
const replacement = ref('')
const selectedParagraph = ref('')
let paragraphPreviewSequence = 0
const replacementEngine = ref<'pymupdf' | 'pdfium-trial'>('pymupdf')
const paragraphTrialAvailable = ref(false)
const inlineReplacementInput = ref<HTMLTextAreaElement | null>(null)
const editSnapshot = ref('')
const editSnapshotFading = ref(false)
const replacingText = ref(false)
const linkPanelOpen = computed(() => activeTool.value === 'links')
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
const annotationPanelOpen = computed(() => activeTool.value === 'notes')
const redactionOpen = computed(() => activeTool.value === 'redact')
const redactionRegion = ref<{x:number,y:number,width:number,height:number} | null>(null)
const redactionReviewed = ref(false)
const redactionBusy = ref(false)
const redactionError = ref('')
const redactionNotice = ref('')
let redactionStart: {x:number,y:number} | null = null
let redactionPointerId: number | null = null
let redactionRevision = 0
const annotationDrawing = ref(false)
const annotationRegion = ref<{x:number,y:number,width:number,height:number} | null>(null)
let annotationStart: {x:number,y:number} | null = null
let annotationPointerId: number | null = null
const pageAnnotations = ref<PageAnnotation[]>([])
const annotationBusy = ref(false)
const annotationError = ref('')
const annotationNotice = ref('')
const annotationImage = ref<File | null>(null)
const annotationPreviewUrl = ref('')
const imageDestination = ref<'working' | 'build'>('working')
const annotationFileInput = ref<HTMLInputElement | null>(null)
const annotationImageUrls = reactive<Record<string,string>>({})
const annotationDraft = reactive({kind:'highlight' as 'image'|'highlight'|'comment', body:'', color:'yellow' as PageAnnotation['color']})
const editingAnnotationId = ref<string | null>(null)
const currentPageAnnotations = computed(() => pageAnnotations.value.filter(item =>
  item.document_id === props.document?.id && item.page_index === current.value - 1))
const marksPanelOpen = computed(() => activeTool.value === 'marks')
const inspectorVisible = computed(() => marksPanelOpen.value ||
  (editTextMode.value && (pendingMove.value || !!runsError.value || !!editNotice.value)) ||
  (redactionOpen.value && !!redactionRegion.value) ||
  (annotationPanelOpen.value && (!!annotationRegion.value || annotationDraft.kind === 'image' || !!editingAnnotationId.value)) ||
  (linkPanelOpen.value && !!linkRegion.value))
const toolHint = computed(() => ({
  'add-text': 'Click the page where text should start, then type directly on the page.',
  'edit-text': 'Click selectable text to edit it; drag a highlighted run to preview a move.',
  redact: 'Drag over the area to remove, then review and apply on the page.',
  notes: annotationDraft.kind === 'image' ? 'Choose an image, then drag its placement on the page.' : 'Drag over the passage to highlight or comment.',
  links: 'Drag a rectangle on the page to create a link.',
  marks: 'Preview output marks on this page; new builds will include saved settings.',
  select: '',
})[activeTool.value])
const toolFeedback = computed(() => activeTool.value === 'redact' ? (redactionError.value || redactionNotice.value) :
  activeTool.value === 'notes' ? (annotationError.value || annotationNotice.value) :
  activeTool.value === 'links' ? (linkError.value || linkNotice.value) :
  activeTool.value === 'edit-text' ? (runsError.value || editNotice.value) : '')
const marksBusy = ref(false)
const marksError = ref('')
const marksNotice = ref('')
const markPreview = ref<OutputMarkPreview[]>([])
const defaultMarks: OutputMarks = {
  watermark_enabled:false,watermark_text:'DRAFT',watermark_position:'center',
  watermark_font_size:32,watermark_opacity:.16,timestamp_enabled:false,
  timestamp_format:'datetime',timestamp_timezone:'UTC',timestamp_position:'bottom_right',
  timestamp_font_size:8,timestamp_prefix:'Built',
  bates_enabled:false,bates_prefix:'',bates_start:1,bates_padding:6,
  bates_position:'bottom_left',bates_font_size:8,bates_include_index:false,
  bates_include_separator:false,bates_restart_per_volume:false,
  annexure_mode:'none',annexure_stamp_scope:'first',
  annexure_stamp_position:'top_right',annexure_stamp_font_size:10,
  annexure_overrides:{},
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
let textLayer: pdfjs.TextLayer | null = null
let renderQueue: Promise<void> = Promise.resolve()
let renderRequest = 0
let loadingTask: ReturnType<typeof pdfjs.getDocument> | null = null
let thumbnailTasks: RenderTask[] = []
let thumbnailSequence = 0
const renderedThumbnails = new Set<number>()
let epoch = 0
let pageRenderSequence = 0
let activeDocumentId: string | null = null
const displayedRevision = ref<number | null>(null)
const displayedPage = ref(0)
let resizeObserver: ResizeObserver | null = null
const pageReady = computed(() => !loading.value && !switchingPage.value &&
  displayedPage.value === current.value && displayedRevision.value === props.document?.revision &&
  activeDocumentId === props.document?.id)
function jumpToPage() {
  const number = Math.floor(Number(pageJump.value))
  if (Number.isFinite(number) && total.value) current.value = Math.max(1, Math.min(total.value, number))
  pageJump.value = current.value
}
function closeDraftWithEscape(event: KeyboardEvent) {
  if (findOpen.value) findOpen.value = false
  else if (selectedRun.value) cancelReplacement()
  else if (pendingMove.value) cancelMove()
  else if (activeTool.value !== 'select') toggleTool(activeTool.value)
  else return
  event.preventDefault()
  event.stopPropagation()
}
function handleViewerKeydown(event: KeyboardEvent) {
  if ((event.ctrlKey || event.metaKey) && ['+','=','-','0'].includes(event.key)) {
    event.preventDefault()
    event.stopPropagation()
    if (event.key === '0') scalePct.value = 100
    else zoomBy(event.key === '-' ? -1 : 1)
    return
  }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'f') {
    event.preventDefault()
    event.stopPropagation()
    findOpen.value = true
    void nextTick(() => findInput.value?.focus())
    return
  }
  if (event.key === 'Escape') { closeDraftWithEscape(event); return }
  if (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey ||
      (event.target instanceof HTMLElement && event.target.closest('input, textarea, select, [contenteditable="true"]'))) return
  switch (event.key.toLowerCase()) {
    case 'v': activeCategory.value = 'edit'; activeTool.value = 'select'; break
    case 'e': activeCategory.value = 'edit'; if (activeTool.value !== 'edit-text') toggleTool('edit-text'); break
    case 't': activeCategory.value = 'edit'; if (activeTool.value !== 'add-text') toggleTool('add-text'); break
    case 'h': activeCategory.value = 'review'; if (activeTool.value !== 'notes' || annotationDraft.kind !== 'highlight') openAnnotationTool('highlight'); break
    default: return
  }
  event.preventDefault()
}
function selectCategory(category: EditorCategory) {
  if (activeCategory.value === category) return
  activeCategory.value = category
  if (activeTool.value !== 'select') toggleTool(activeTool.value)
}
function showFind() {
  findOpen.value = !findOpen.value
  if (findOpen.value) void nextTick(() => findInput.value?.focus())
}
function highlightFindText() {
  const term = findOpen.value ? findQuery.value.trim().toLocaleLowerCase() : ''
  for (const span of textLayerContainer.value?.querySelectorAll('span') || [])
    span.classList.toggle('pdf-find-match', term.length >= 2 && (span.textContent || '').toLocaleLowerCase().includes(term))
}
async function searchWorkingDocument() {
  const sequence = ++findSequence
  const doc = props.document
  const query = findQuery.value.trim()
  findMatches.value = []
  findIndex.value = 0
  findTruncated.value = false
  findError.value = ''
  highlightFindText()
  if (!doc || query.length < 2) { findBusy.value = false; return }
  findBusy.value = true
  try {
    const result = await api<{matches:DocumentSearchMatch[],truncated:boolean}>(
      `/documents/${encodeURIComponent(doc.id)}/search?q=${encodeURIComponent(query)}&revision=${doc.revision}`,
      'GET', undefined, 5 * 60_000)
    if (sequence !== findSequence || props.document?.id !== doc.id || props.document?.revision !== doc.revision) return
    findMatches.value = result.matches
    findTruncated.value = result.truncated
    const onPage = result.matches.findIndex(item => item.page === current.value)
    findIndex.value = Math.max(0, onPage)
  } catch (err) { if (sequence === findSequence) findError.value = friendlyError(err) }
  finally { if (sequence === findSequence) findBusy.value = false }
}
function stepFind(step: number) {
  if (!findMatches.value.length) return
  findIndex.value = (findIndex.value + step + findMatches.value.length) % findMatches.value.length
  current.value = findMatches.value[findIndex.value].page
}
watch([findQuery, () => props.document?.id, () => props.document?.revision], () => {
  findSequence++
  if (findTimer) clearTimeout(findTimer)
  findMatches.value = []
  findIndex.value = 0
  findError.value = ''
  findBusy.value = !!props.document && findQuery.value.trim().length >= 2
  highlightFindText()
  findTimer = setTimeout(() => void searchWorkingDocument(), 250)
})
watch(findOpen, () => highlightFindText())
function toggleTool(tool: EditorTool) {
  activeTool.value = activeTool.value === tool ? 'select' : tool
  placement.value = null
  overlayError.value = ''
  selectedRun.value = null
  paragraphPreviewSequence++
  cancelMove()
  linkRegion.value = null
  redactionRegion.value = null
  redactionReviewed.value = false
  resetAnnotationDraft()
  if (activeTool.value === 'links' && props.document)
    linkDraft.target_document_id ||= props.document.id
}
function openAnnotationTool(kind: 'image' | 'highlight' | 'comment') {
  if (activeTool.value === 'notes' && annotationDraft.kind === kind) {
    toggleTool('notes')
    return
  }
  if (activeTool.value === 'notes') resetAnnotationDraft()
  else toggleTool('notes')
  annotationNotice.value = ''
  annotationDraft.kind = kind
}
const toolTitle = computed(() => ({
  'add-text': 'Add text',
  'edit-text': 'Edit text',
  redact: 'Redact',
  notes: annotationDraft.kind === 'image' ? 'Image / stamp' : 'Review notes',
  links: 'Reviewed links',
  marks: 'Output marks',
  select: 'PDF tool',
})[activeTool.value])
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
  if (!currentPdf || !thumbnailWrap.value || !showThumbnails.value) return
  await nextTick()
  if (currentPdf !== pdf || !thumbnailWrap.value) return
  for (const element of thumbnailWrap.value.querySelectorAll<HTMLCanvasElement>('canvas[data-page]')) {
    if (currentPdf !== pdf || sequence !== thumbnailSequence) return
    const number = Number(element.dataset.page)
    if (renderedThumbnails.has(number) && element.width && element.height) continue
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
      if (currentPdf === pdf && sequence === thumbnailSequence) renderedThumbnails.add(number)
    } catch (err) {
    if (currentPdf === pdf && sequence === thumbnailSequence &&
        !(err instanceof Error && err.name === 'RenderingCancelledException'))
      viewerError.value = friendlyError(err)
    }
  }
}

function render(): Promise<void> {
  const request = ++renderRequest
  renderTask?.cancel()
  textLayer?.cancel()
  thumbnailSequence++
  thumbnailTasks.forEach(task => task.cancel())
  thumbnailTasks = []
  const queued = renderQueue.catch(() => {}).then(async () => {
    if (request === renderRequest) {
      await renderPage(request)
      if (request === renderRequest && showThumbnails.value) void renderThumbnails()
    }
  })
  renderQueue = queued
  return queued
}

async function renderSelectableText(page: Awaited<ReturnType<PDFDocumentProxy['getPage']>>,
                                    viewport: ReturnType<Awaited<ReturnType<PDFDocumentProxy['getPage']>>['getViewport']>,
                                    request: number) {
  const layerElement = textLayerContainer.value
  if (!layerElement) return
  layerElement.replaceChildren()
  layerElement.style.width = `${viewport.width}px`
  layerElement.style.height = `${viewport.height}px`
  layerElement.style.setProperty('--scale-factor', String(viewport.scale))
  try {
    const content = await page.getTextContent()
    if (request !== renderRequest || layerElement !== textLayerContainer.value) return
    const layer = new pdfjs.TextLayer({textContentSource:content,container:layerElement,viewport})
    textLayer = layer
    await layer.render()
    if (request === renderRequest && textLayer === layer) highlightFindText()
    if (request !== renderRequest && textLayer === layer) layerElement.replaceChildren()
  } catch (err) {
    if (request === renderRequest) {
      layerElement.replaceChildren()
      console.warn('Selectable text layer unavailable:', err)
    }
  }
}

async function renderPage(request: number) {
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
    // Render offscreen so page switches and zoom never erase the last good page.
    const maxPixels = 16_000_000
    const deviceScale = Math.min(window.devicePixelRatio || 1, 2,
      Math.sqrt(maxPixels / (viewport.width * viewport.height)))
    const staged = document.createElement('canvas')
    staged.width = Math.max(1, Math.round(viewport.width * deviceScale))
    staged.height = Math.max(1, Math.round(viewport.height * deviceScale))
    const ctx = staged.getContext('2d')
    if (!ctx) return
    const transform: [number,number,number,number,number,number] | undefined =
      deviceScale === 1 ? undefined : [deviceScale, 0, 0, deviceScale, 0, 0]
    renderTask = page.render({ canvasContext: ctx, viewport, transform })
    await renderTask.promise
    if (request !== renderRequest || pdf !== currentPdf || pageNum !== current.value || !canvas.value) return
    const element = canvas.value
    element.width = staged.width
    element.height = staged.height
    element.style.width = `${viewport.width}px`
    element.style.height = `${viewport.height}px`
    element.getContext('2d')?.drawImage(staged, 0, 0)
    displayedPage.value = pageNum
    pageZoom.value = zoom
    void renderSelectableText(page, viewport, request)
  } catch (err) {
    if (request === renderRequest && currentPdf === pdf &&
        !(err instanceof Error && err.name === 'RenderingCancelledException'))
      viewerError.value = friendlyError(err)
  }
}

async function loadPdf(doc: MatterDocument | null) {
  if (doc && pdf && doc.id === activeDocumentId && doc.revision === displayedRevision.value) return
  const call = ++epoch
  if (loadingTask) { void loadingTask.destroy(); loadingTask = null }
  renderRequest++
  textLayer?.cancel()
  textLayerContainer.value?.replaceChildren()
  const sameDocument = doc?.id === activeDocumentId
  if (!sameDocument) activeTool.value = 'select'
  const retainedPage = sameDocument ? current.value : 1
  activeDocumentId = doc?.id ?? null
  loading.value = true
  switchingPage.value = false
  viewerError.value = ''
  current.value = retainedPage
  loadedPageCount.value = 0
  placement.value = null
  selectedRun.value = null
  paragraphPreviewSequence++
  cancelMove()
  if (renderTask) { renderTask.cancel(); renderTask = null }
  thumbnailSequence++
  thumbnailTasks.forEach(task => task.cancel())
  thumbnailTasks = []
  renderedThumbnails.clear()
  if (pdf) { const previous = pdf; pdf = null; void previous.destroy() }
  displayedRevision.value = null
  displayedPage.value = 0
  if (!doc) { loading.value = false; return }
  try {
    const source = await pdfDocumentSource(doc.id, doc.revision)
    if (call !== epoch) return
    const task = pdfjs.getDocument(source)
    loadingTask = task
    const loaded = await task.promise
    if (call !== epoch) { void loaded.destroy(); return }
    loadingTask = null
    pdf = loaded
    loadedPageCount.value = pdf.numPages
    current.value = Math.min(current.value, pdf.numPages)
    if (!sameDocument && props.pageTarget?.documentId === doc.id) current.value = Math.min(Math.max(1, props.pageTarget.page), pdf.numPages)
    await nextTick()
    if (call !== epoch) return
    await render()
    if (!viewerError.value) displayedRevision.value = doc.revision
  } catch (err) { if (call === epoch) viewerError.value = friendlyError(err) }
  finally { if (call === epoch) loading.value = false }
}
watch(() => `${props.document?.id || ''}|${props.document?.revision || ''}`, () => void loadPdf(props.document), { immediate: true })
watch(() => props.document?.id, async id => {
  paragraphTrialAvailable.value = false
  replacementEngine.value = 'pymupdf'
  if (!id) return
  try {
    const status = await api<{available:boolean}>('/pdfium-paragraph-trial/status')
    if (props.document?.id === id) paragraphTrialAvailable.value = status.available
  } catch { /* ordinary text replacement remains available */ }
}, {immediate:true})
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
watch(current, value => { pageJump.value = value })
watch(canvasWrap, element => {
  resizeObserver?.disconnect()
  resizeObserver = null
  if (!element) return
  resizeObserver = new ResizeObserver(() => void render())
  resizeObserver.observe(element)
})
watch(current, () => { placement.value = null; selectedRun.value = null; cancelMove() })
watch(placement, point => { if (point) void nextTick(() => inlineAddInput.value?.focus()) })
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
watch(showThumbnails, shown => {
  if (!shown) renderedThumbnails.clear()
  else void renderQueue.catch(() => {}).then(() => renderThumbnails())
})
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
watch(() => [props.document?.matter_id || '', props.bookmarkRefreshKey || 0] as const, async ([matterId]) => {
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
watch([current, () => props.document?.id, () => props.document?.revision], () => {
  redactionRegion.value = null; redactionReviewed.value = false; redactionError.value = ''
})
onBeforeUnmount(() => { epoch++; findSequence++; if (findTimer) clearTimeout(findTimer); if (loadingTask) void loadingTask.destroy(); resizeObserver?.disconnect(); thumbnailSequence++; renderTask?.cancel(); textLayer?.cancel(); thumbnailTasks.forEach(task => task.cancel()); if (markPreviewTimer) clearTimeout(markPreviewTimer); clearAnnotationImageUrls(); if (annotationPreviewUrl.value) URL.revokeObjectURL(annotationPreviewUrl.value); if (pdf) void pdf.destroy() })

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
  if (!rect || !Number.isFinite(rect.width) || !Number.isFinite(rect.height) ||
      rect.width <= 0 || rect.height <= 0) return null
  const x = (event.clientX-rect.left)/rect.width
  const y = (event.clientY-rect.top)/rect.height
  if (!Number.isFinite(x) || !Number.isFinite(y)) return null
  return {x:Math.max(0,Math.min(1,x)), y:Math.max(0,Math.min(1,y))}
}
function beginLinkRegion(event: PointerEvent) {
  if (!pageReady.value || !linkPanelOpen.value || props.disabled || editing.value || event.button !== 0) return
  const point = normalizedPointer(event)
  if (!point) return
  linkNotice.value = ''
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
  if (!pageReady.value || !annotationPanelOpen.value || props.disabled || annotationBusy.value || event.button !== 0) return
  const point = normalizedPointer(event)
  if (!point) return
  annotationNotice.value = ''
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
function beginRedactionRegion(event: PointerEvent) {
  if (!pageReady.value || !redactionOpen.value || props.disabled || editing.value || loading.value ||
      switchingPage.value || event.button !== 0) return
  const point = normalizedPointer(event)
  if (!point) return
  redactionNotice.value = ''
  redactionStart = point
  redactionPointerId = event.pointerId
  redactionRegion.value = {x:point.x,y:point.y,width:0,height:0}
  redactionReviewed.value = false
  redactionRevision = props.document?.revision || 0
  ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
  event.preventDefault()
}
function updateRedactionRegion(event: PointerEvent) {
  if (redactionPointerId !== event.pointerId || !redactionStart) return
  const point = normalizedPointer(event)
  if (!point) return
  redactionRegion.value = {x:Math.min(redactionStart.x,point.x),y:Math.min(redactionStart.y,point.y),
    width:Math.abs(point.x-redactionStart.x),height:Math.abs(point.y-redactionStart.y)}
  event.preventDefault()
}
function finishRedactionRegion(event: PointerEvent) {
  if (redactionPointerId !== event.pointerId) return
  redactionPointerId = null
  redactionStart = null
  if (!redactionRegion.value || !Number.isFinite(redactionRegion.value.x) ||
      !Number.isFinite(redactionRegion.value.y) || !Number.isFinite(redactionRegion.value.width) ||
      !Number.isFinite(redactionRegion.value.height) || redactionRegion.value.width < .01 ||
      redactionRegion.value.height < .01) {
    redactionRegion.value = null
    redactionError.value = 'Drag a visible rectangular area on the page.'
  } else redactionError.value = ''
}
async function applyRedaction() {
  const doc = props.document
  const region = redactionRegion.value
  if (!doc || !region || !redactionReviewed.value || redactionBusy.value || props.disabled) return
  if (!Number.isFinite(region.x) || !Number.isFinite(region.y) ||
      !Number.isFinite(region.width) || !Number.isFinite(region.height) ||
      region.x < 0 || region.y < 0 || region.width <= 0 || region.height <= 0 ||
      region.x + region.width > 1 || region.y + region.height > 1 ||
      !Number.isInteger(redactionRevision) || redactionRevision < 1) {
    redactionError.value = 'The marked area is no longer valid. Reload the page and draw it again.'
    return
  }
  const signature = confirmSignatureTransformation([doc], 'Permanently redacting a working page')
  if (!signature.proceed) return
  redactionBusy.value = true
  editing.value = true
  redactionError.value = ''
  try {
    const result = await api<MatterDocument>(`/documents/${doc.id}/pages/redact`, 'POST', {
      page_index: current.value - 1, expected_revision: redactionRevision,
      rect_x: region.x, rect_y: region.y, rect_width: region.width, rect_height: region.height,
      acknowledge_signature_risk: signature.acknowledged,
    }, 120_000)
    redactionRegion.value = null
    redactionReviewed.value = false
    redactionNotice.value = 'Applied to the working copy. Inspect the page and rebuild exports. Original and undo history remain recoverable.'
    await loadPdf(result)
    emit('modified')
  } catch (err) { redactionError.value = friendlyError(err) }
  finally { editing.value = false; redactionBusy.value = false }
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
  if (annotationPreviewUrl.value) URL.revokeObjectURL(annotationPreviewUrl.value)
  annotationPreviewUrl.value = ''
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
  if (annotationPreviewUrl.value) URL.revokeObjectURL(annotationPreviewUrl.value)
  annotationImage.value = (event.target as HTMLInputElement).files?.[0] || null
  annotationPreviewUrl.value = annotationImage.value ? URL.createObjectURL(annotationImage.value) : ''
}
function editPageAnnotation(item: PageAnnotation) {
  if (!item.resolved) { annotationError.value = 'This annotation needs review because the document revision changed.'; return }
  activeTool.value = 'notes'
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
  const workingImage = annotationDraft.kind === 'image' && !editingAnnotationId.value && imageDestination.value === 'working'
  const signature = workingImage ? confirmSignatureTransformation([doc], 'Inserting an image or stamp into the working PDF') : null
  if (signature && !signature.proceed) return
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
    } else if (workingImage && annotationImage.value) {
      const updated = await uploadWorkingPageImage(doc.id, annotationImage.value, {
        page_index:current.value-1,expected_revision:doc.revision,rect_x:region.x,
        rect_y:region.y,rect_width:region.width,rect_height:region.height,
        acknowledge_signature_risk:signature?.acknowledged || false,
      })
      annotationNotice.value = 'Image inserted into a new working revision. Review the page and rebuild exports; Undo can restore the prior revision.'
      await loadPdf(updated)
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
  if (!pageReady.value || !textMode.value || linkPanelOpen.value || props.disabled || editing.value || loading.value || !canvas.value) return
  const rect = canvas.value.getBoundingClientRect()
  const x = (event.clientX - rect.left) / rect.width
  const y = (event.clientY - rect.top) / rect.height
  if (x >= 0 && x < 1 && y >= 0 && y < 1) {
    placement.value = { x, y, page: current.value }
    overlayError.value = ''
  }
}
async function placeText() {
  if (!props.document || !placement.value || editing.value || props.disabled) return
  if (overlayValidation.value) { overlayError.value = overlayValidation.value; return }
  const signature = confirmSignatureTransformation([props.document], 'Adding visible text')
  if (!signature.proceed) return
  editing.value = true
  overlayError.value = ''
  try {
    await api(`/documents/${props.document.id}/pages/text-overlay`, 'POST', {
      page_index: placement.value.page - 1, x: placement.value.x, y: placement.value.y,
      text: overlayText.value, font_size: overlaySize.value,
      acknowledge_signature_risk: signature.acknowledged,
    })
    placement.value = null
    overlayText.value = ''
    emit('modified')
  } catch (err) { overlayError.value = friendlyError(err) }
  finally { editing.value = false }
}
async function selectRun(run: TextRun) {
  if (!pageReady.value || props.disabled || editing.value || pendingMove.value) return
  const previewSequence = ++paragraphPreviewSequence
  selectedRun.value = run
  replacement.value = run.text
  selectedParagraph.value = ''
  runsError.value = ''
  editNotice.value = ''
  if (replacementEngine.value === 'pdfium-trial' && props.document) {
    const documentId = props.document.id
    const revision = runsRevision.value
    const page = current.value
    try {
      const preview = await api<{text:string}>(`/documents/${documentId}/pages/paragraph-trial-preview`, 'POST', {
        page_index:page-1, span_index:run.index, expected_revision:revision, expected_text:run.text,
      })
      if (previewSequence !== paragraphPreviewSequence || selectedRun.value?.index !== run.index || props.document?.id !== documentId ||
          current.value !== page || runsRevision.value !== revision || replacementEngine.value !== 'pdfium-trial') return
      selectedParagraph.value = preview.text
      replacement.value = preview.text
    } catch (err) { if (previewSequence === paragraphPreviewSequence && selectedRun.value?.index === run.index) runsError.value = friendlyError(err) }
  }
  await nextTick()
  inlineReplacementInput.value?.focus()
  inlineReplacementInput.value?.select()
}
function cancelReplacement() {
  paragraphPreviewSequence++
  selectedRun.value = null
  replacement.value = ''
  selectedParagraph.value = ''
  runsError.value = ''
}
watch(replacementEngine, () => { if (selectedRun.value) void selectRun(selectedRun.value) })
function beginMove(event: PointerEvent, run: TextRun) {
  if (!pageReady.value || event.button !== 0 || pendingMove.value || props.disabled || editing.value || loading.value || !canvas.value) return
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
  suppressNextClick = true
  window.setTimeout(() => { suppressNextClick = false }, 0)
  if (event.type === 'pointerup' && Math.abs(verticalOffset.value) >= .0001)
    pendingMove.value = true
  else cancelMove()
}
function cancelMove() {
  movingRun.value = null
  verticalOffset.value = 0
  pendingMove.value = false
}
function confirmMove() {
  if (movingRun.value && pendingMove.value)
    void applyMove(movingRun.value, verticalOffset.value)
}
function clickRun(run: TextRun) {
  if (suppressNextClick) { suppressNextClick = false; return }
  if (pendingMove.value) return
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
    editNotice.value = 'Text moved by removing and redrawing the selected glyphs in the working copy. Review font and position; Undo can restore the prior revision.'
    emit('modified')
  } catch (err) { runsError.value = friendlyError(err) }
  finally { editing.value = false; replacingText.value = false; editSnapshot.value = ''; cancelMove() }
}
async function applyReplacement() {
  if (!props.document || !selectedRun.value || !replacement.value.trim() || props.disabled || editing.value) return
  if (replacementEngine.value === 'pdfium-trial' && !selectedParagraph.value) {
    runsError.value = 'Select a supported paragraph before applying the PDFium trial.'
    return
  }
  const signature = confirmSignatureTransformation([props.document], 'Replacing visible text')
  if (!signature.proceed) return
  editing.value = true
  replacingText.value = true
  runsError.value = ''
  try {
    const trial = replacementEngine.value === 'pdfium-trial'
    const result = await api<MatterDocument & {edit_layout:{font_size?:number,line_count?:number,font_size_reduced?:boolean,edit_method:string,font_substituted?:boolean,font_name?:string}}>(`/documents/${props.document.id}/pages/${trial ? 'paragraph-trial' : 'replace-text-run'}`, 'POST', {
      page_index: current.value - 1, span_index: selectedRun.value.index,
      expected_revision: runsRevision.value, expected_text: selectedRun.value.text,
      ...(trial ? {expected_paragraph:selectedParagraph.value} : {}),
      replacement: replacement.value,
      acknowledge_signature_risk: signature.acknowledged,
    })
    editNotice.value = trial
      ? `Experimental PDFium paragraph edit saved in ${result.edit_layout.line_count} line(s)${result.edit_layout.font_substituted ? ` with substituted ${result.edit_layout.font_name}` : ''}. Inspect the page, extraction, and export.`
      : `Removed and redrew selected text in ${result.edit_layout.line_count} line(s) at ${result.edit_layout.font_size} pt${result.edit_layout.font_size_reduced ? ' (reduced to fit)' : ''}. Review font, layout and extraction.`
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
  <div class="viewer-root" tabindex="0" @keydown="handleViewerKeydown">
    <template v-if="document">
      <div class="viewer-toolbar">
        <div class="toolbar-group reader-document"><FileSearch :size="16"/><div><strong :title="document.title">{{document.title}}</strong><span>Working copy · Original preserved</span></div></div>
        <div class="toolbar-group"><button class="icon-btn small" aria-label="Undo last matter change" title="Undo last matter change" :disabled="!canUndo || disabled || editing" @click="emit('undo')"><Undo2 :size="17"/></button><button class="icon-btn small" aria-label="Redo last matter change" title="Redo last matter change" :disabled="!canRedo || disabled || editing" @click="emit('redo')"><Redo2 :size="17"/></button></div>
        <div class="toolbar-group">
          <button class="icon-btn small" :disabled="current <= 1 || loading" title="Previous page" @click="current--"><ChevronLeft :size="17" /></button>
          <span class="page-counter"><input v-model.number="pageJump" type="number" min="1" :max="total" aria-label="Go to page" @change="jumpToPage" @keydown.enter.prevent="jumpToPage"/> / {{ total }}</span>
          <button class="icon-btn small" :disabled="current >= total || loading" title="Next page" @click="current++"><ChevronRight :size="17" /></button>
        </div>
        <div class="toolbar-group"><button class="icon-btn small" aria-label="Find in this PDF" title="Find in this PDF (Ctrl/⌘+F)" :aria-pressed="findOpen" @click="showFind"><Search :size="16"/></button><button class="btn secondary small editor-expand-button" :aria-pressed="!!expanded" :title="expanded ? 'Exit full screen (Esc)' : 'Expand PDF editor to fill the window'" @click="emit('toggleExpanded')"><Minimize2 v-if="expanded" :size="16"/><Maximize2 v-else :size="16"/><span>{{expanded ? 'Exit full screen' : 'Full screen'}}</span></button><button class="icon-btn small" :aria-label="focusMode ? 'Show document list' : 'Focus on PDF'" :title="focusMode ? 'Show document list' : 'Focus on PDF'" :aria-pressed="!!focusMode" @click="emit('toggleFocus')"><PanelRightClose :size="16"/></button><button class="icon-btn small" :aria-label="showThumbnails?'Hide thumbnails':'Show thumbnails'" :aria-pressed="showThumbnails" title="Toggle page thumbnails" @click="showThumbnails=!showThumbnails"><LayoutGrid :size="16"/></button><button class="icon-btn small" aria-label="Zoom out" title="Zoom out (Ctrl/⌘−)" :disabled="scalePct===25" @click="zoomBy(-1)">−</button><select v-model.number="scalePct" class="zoom-select" aria-label="Zoom"><option :value="25">25%</option><option :value="50">50%</option><option :value="75">75%</option><option :value="100">Fit width</option><option :value="125">125%</option><option :value="150">150%</option></select><button class="icon-btn small" aria-label="Zoom in" title="Zoom in (Ctrl/⌘+)" :disabled="scalePct===150" @click="zoomBy(1)">+</button></div>
      </div>
      <div v-if="findOpen" class="pdf-findbar" role="search" aria-label="Find in working PDF">
        <Search :size="16" aria-hidden="true"/>
        <input ref="findInput" v-model="findQuery" type="search" placeholder="Find text in this PDF" aria-label="Find text in this PDF" @keydown.enter.prevent="stepFind($event.shiftKey ? -1 : 1)"/>
        <span v-if="findBusy">Searching…</span><span v-else-if="findError" class="error-msg" role="alert">{{findError}}</span><span v-else-if="findQuery.trim().length>=2">{{findMatches.length}}{{findTruncated ? '+' : ''}} matching page{{findMatches.length===1?'':'s'}}<template v-if="findMatches.length"> · {{findIndex+1}}/{{findMatches.length}}</template></span>
        <button class="icon-btn small" aria-label="Previous search result" :disabled="!findMatches.length" @click="stepFind(-1)"><ChevronLeft :size="16"/></button>
        <button class="icon-btn small" aria-label="Next search result" :disabled="!findMatches.length" @click="stepFind(1)"><ChevronRight :size="16"/></button>
        <button class="icon-btn small" aria-label="Close PDF search" @click="findOpen=false"><X :size="16"/></button>
      </div>
      <div v-if="findOpen && findMatches.length" class="pdf-find-results" aria-label="Matching PDF pages">
        <button v-for="(match,index) in findMatches" :key="match.page" :class="{active:index===findIndex}" :title="match.snippet" @click="findIndex=index;current=match.page">Page {{match.page}} · {{match.snippet}}</button>
      </div>
      <div class="pdf-tool-tabs" role="tablist" aria-label="PDF tools">
        <button v-for="category in (['edit','review','pages','output'] as const)" :key="category" role="tab" :aria-selected="activeCategory===category" :class="{active:activeCategory===category}" @click="selectCategory(category)">{{category==='edit'?'Edit':category==='review'?'Review':category==='pages'?'Pages':'Output'}}</button>
        <span class="pdf-tool-shortcuts">V Select · E Edit text · T Add text · H Highlight</span>
      </div>
      <div class="page-editbar">
        <div v-if="activeCategory==='edit'" class="editor-tool-group" role="group" aria-label="Edit PDF">
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="activeTool==='select'" @click="toggleTool('select')">Select</button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="textMode" @click="toggleTool('add-text')"><Type :size="15"/> Add text</button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="editTextMode" @click="toggleTool('edit-text')"><Pencil :size="15"/> Edit text</button>
        <select v-if="editTextMode" v-model="replacementEngine" class="pdf-edit-method" aria-label="Text editing method" :disabled="editing || disabled"><option value="pymupdf">Fit and redraw</option><option value="pdfium-trial" :disabled="!paragraphTrialAvailable">PDFium paragraph{{paragraphTrialAvailable ? '' : ' (unavailable)'}}</option></select>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="redactionOpen" @click="toggleTool('redact')"><Eraser :size="15"/> Redact</button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="annotationPanelOpen && annotationDraft.kind==='image'" @click="activeCategory='edit';openAnnotationTool('image')"><Images :size="15"/> Image / stamp</button>
        </div><div v-if="activeCategory==='review'" class="editor-tool-group" role="group" aria-label="Review PDF">
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-expanded="bookmarkOpen" @click="bookmarkOpen=!bookmarkOpen;bookmarkTitle=document?.title+' — page '+current;bookmarkError=''"><BookMarked :size="15"/> Bookmark page<span v-if="pageBookmarks.length" class="page-bookmark-count">{{pageBookmarks.length}}</span></button>
        <button class="text-tool" :disabled="editing || loading || disabled" title="Private review highlight" :aria-pressed="annotationPanelOpen && annotationDraft.kind==='highlight'" @click="openAnnotationTool('highlight')"><Highlighter :size="15"/> Highlight<span v-if="currentPageAnnotations.length" class="page-bookmark-count">{{currentPageAnnotations.length}}</span></button>
        <button class="text-tool" :disabled="editing || loading || disabled" title="Private review comment" :aria-pressed="annotationPanelOpen && annotationDraft.kind==='comment'" @click="openAnnotationTool('comment')"><MessageSquare :size="15"/> Comment</button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="linkPanelOpen" @click="toggleTool('links')"><Link2 :size="15"/> Links<span v-if="currentPageLinks.length" class="page-bookmark-count">{{currentPageLinks.length}}</span></button>
        </div><div v-if="activeCategory==='output'" class="editor-tool-group" role="group" aria-label="Output PDF">
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="marksPanelOpen" @click="toggleTool('marks')"><Stamp :size="15"/> Output marks<span v-if="marksDirty" class="details-unsaved-indicator">Unsaved</span></button>
        </div><div v-if="activeCategory==='pages'" class="editor-tool-group" role="group" aria-label="Organise pages">
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
      <div class="viewer-edit-workspace" :class="{'has-tool-inspector':inspectorVisible}">
      <aside v-if="inspectorVisible" class="viewer-tool-inspector" :class="{'marks-inspector':marksPanelOpen}" aria-label="PDF editing controls">
        <div class="viewer-tool-inspector-heading"><strong>{{toolTitle}}</strong><button class="icon-btn small" aria-label="Close PDF tool" title="Close tool" @click="toggleTool(activeTool)"><X :size="16"/></button></div>
      <Transition name="soft"><div v-if="editTextMode" class="text-overlay-panel" aria-label="Edit existing text">
        <strong>Edit selectable text</strong>
        <p>Click highlighted text to edit it, or drag it up or down to preview a move. Apply or cancel before changing the working copy. Scanned text cannot be moved. This is not a confidential redaction tool.</p>
        <p v-if="replacementEngine==='pdfium-trial'">Experimental PDFium paragraph edit. Selecting a run loads its whole paragraph. Unsupported pages fail without a new revision; there is no automatic visual replacement.</p>
        <div v-if="pendingMove" class="page-tool-actions"><span>Previewing a move on page {{current}}.</span><div><button class="btn secondary small" :disabled="editing" @click="cancelMove">Cancel</button><button class="btn primary small" :disabled="editing || disabled" @click="confirmMove">Apply move</button></div></div>
        <p v-if="runsError && !selectedRun" class="error-msg" role="alert">{{runsError}}</p>
        <p class="edit-status-slot" role="status">{{editNotice}}</p>
        <p v-if="!textRuns.length && !editNotice">No selectable horizontal text found on this page. Scanned text needs OCR and review.</p>
      </div></Transition>
      <Transition name="soft"><div v-if="redactionOpen" class="text-overlay-panel" aria-label="Permanent redaction">
        <strong>Permanent redaction · page {{current}}</strong>
        <p>Drag over content, inspect the marked area, then apply. Text and image pixels in the area are removed from a new working copy. Applying also strips imported PDF metadata, attachments, bookmarks, links and annotations from this working document.</p>
        <p>Imported originals, prior revisions, existing exports and matter history can still contain the content. Review linked titles, notes and document details separately before sharing.</p>
        <label class="checkbox-line" :style="{visibility:redactionRegion ? 'visible' : 'hidden'}"><input v-model="redactionReviewed" type="checkbox" :disabled="!redactionRegion"/> I reviewed this area and the document-wide cleanup.</label>
        <div class="page-tool-actions"><span>{{redactionRegion ? `Marked ${Math.round(redactionRegion.width*100)}% × ${Math.round(redactionRegion.height*100)}% of page` : 'Drag a region on the page'}}</span><button class="btn secondary small" :disabled="!redactionRegion || !redactionReviewed || redactionBusy || disabled" @click="applyRedaction">{{redactionBusy ? 'Applying…' : 'Apply redaction'}}</button></div>
        <p v-if="redactionError" class="error-msg" role="alert">{{redactionError}}</p><p v-if="redactionNotice" class="review-tool-notice" role="status">{{redactionNotice}}</p>
      </div></Transition>
      <Transition name="soft"><div v-if="annotationPanelOpen" class="page-annotation-panel" aria-label="Image overlays, highlights and comments">
        <div class="review-tool-heading"><strong>Images and review notes</strong><span>Draw a region on the page. Choose whether an image changes the working PDF or only a later build. Private highlights/comments are excluded from filing builds.</span></div>
        <div class="annotation-kind-row" role="group" aria-label="Annotation type">
          <button :class="{active:annotationDraft.kind==='image'}" :disabled="!!editingAnnotationId && annotationDraft.kind!=='image'" @click="annotationDraft.kind='image'"><Images :size="15"/> Image / stamp</button>
          <button :class="{active:annotationDraft.kind==='highlight'}" :disabled="!!editingAnnotationId && annotationDraft.kind!=='highlight'" @click="annotationDraft.kind='highlight'"><Highlighter :size="15"/> Highlight</button>
          <button :class="{active:annotationDraft.kind==='comment'}" :disabled="!!editingAnnotationId && annotationDraft.kind!=='comment'" @click="annotationDraft.kind='comment'"><MessageSquare :size="15"/> Comment</button>
        </div>
        <div class="annotation-form-grid">
          <label v-if="annotationDraft.kind==='image' && !editingAnnotationId">PNG or JPEG<input ref="annotationFileInput" type="file" accept="image/png,image/jpeg,.png,.jpg,.jpeg" :disabled="annotationBusy || disabled" @change="chooseAnnotationImage"/></label>
          <label v-if="annotationDraft.kind==='image' && !editingAnnotationId">Destination<select v-model="imageDestination" :disabled="annotationBusy || disabled"><option value="working">Insert into working PDF</option><option value="build">Only in new bundle builds</option></select></label>
          <label v-if="annotationDraft.kind!=='image'">Colour<select v-model="annotationDraft.color" :disabled="annotationBusy || disabled"><option value="yellow">Yellow</option><option value="green">Green</option><option value="blue">Blue</option><option value="pink">Pink</option></select></label>
          <label v-if="annotationDraft.kind!=='image'" class="annotation-note">{{annotationDraft.kind==='comment'?'Comment text':'Optional highlight note'}}<textarea v-model="annotationDraft.body" maxlength="4000" rows="2" :disabled="annotationBusy || disabled"/></label>
        </div>
        <div class="page-tool-actions"><span>{{annotationRegion ? `${Math.round(annotationRegion.width*100)}% × ${Math.round(annotationRegion.height*100)}% region selected` : 'Drag a region on the page'}}</span><div><button v-if="editingAnnotationId" class="btn secondary small" :disabled="annotationBusy" @click="resetAnnotationDraft()">Cancel edit</button><button class="btn secondary small" :disabled="annotationBusy || disabled || !annotationRegion || (annotationDraft.kind==='comment' && !annotationDraft.body.trim()) || (annotationDraft.kind==='image' && !editingAnnotationId && !annotationImage)" @click="savePageAnnotation">{{annotationBusy?'Saving…':editingAnnotationId?'Update annotation':annotationDraft.kind==='image'?(imageDestination==='working'?'Insert into PDF':'Save for builds'):'Save annotation'}}</button></div></div>
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
      </aside>
      <div class="viewer-page-column">
      <div v-if="activeTool!=='select'" class="pdf-tool-hint" role="status"><strong>{{toolTitle}}</strong><span>{{toolFeedback || toolHint}}</span><button class="icon-btn small" aria-label="Cancel PDF tool" title="Cancel tool (Esc)" @click="toggleTool(activeTool)"><X :size="14"/></button></div>
      <Transition name="soft"><div v-if="total > 0 && showThumbnails" ref="thumbnailWrap" class="thumbnail-strip" aria-label="Page thumbnails">
        <span class="thumbnail-hint">Drag to reorder working-copy pages</span>
        <button v-for="number in thumbnailPages" :key="number" class="thumbnail-button" :class="{selected:number===current,dragging:draggingPage===number,'drop-target':dropTargetPage===number}" :aria-label="'Go to page '+number" :draggable="!disabled && !editing" @dragstart="startPageDrag($event,number)" @dragend="draggingPage=null;dropTargetPage=null" @dragover="overPage($event,number)" @drop.prevent="dropPage($event,number)" @click="current=number"><canvas :data-page="number"></canvas><span>{{number}}</span></button>
      </div></Transition>
      <div ref="canvasWrap" class="canvas-scroll" :aria-busy="loading || editing">
        <div v-if="(loading || editing) && !(replacingText && editSnapshot)" class="preview-status"><LoaderCircle :size="18" class="spin"/> {{ editing ? 'Updating PDF…' : 'Loading PDF…' }}</div>
        <div v-if="viewerError" class="preview-status error-msg">{{ viewerError }}</div>
        <div class="pdf-placement-surface" :class="{'selecting-text':activeTool==='select' && !disabled, 'placing-text':textMode && !disabled, 'drawing-link':linkPanelOpen && !disabled, 'drawing-annotation':annotationPanelOpen && !disabled, 'drawing-redaction':redactionOpen && !disabled, 'is-busy':(loading || editing) && !replacingText, 'is-edit-refreshing':replacingText, 'is-switching':switchingPage, 'is-stale':!pageReady, 'is-rotating':rotatingPage}" @click="choosePlacement" @pointerdown="beginLinkRegion($event);beginAnnotationRegion($event);beginRedactionRegion($event)" @pointermove="updateLinkRegion($event);updateAnnotationRegion($event);updateRedactionRegion($event)" @pointerup="finishLinkRegion($event);finishAnnotationRegion($event);finishRedactionRegion($event)" @pointercancel="finishLinkRegion($event);finishAnnotationRegion($event);finishRedactionRegion($event)">
          <canvas ref="canvas" class="pdf-paper" :style="{visibility: viewerError ? 'hidden' : 'visible'}" />
          <div ref="textLayerContainer" class="pdf-text-layer textLayer" aria-label="Selectable PDF text"></div>
          <span v-if="textMode && placement && placement.page===current && overlayText" class="pdf-add-text-preview" :style="{left:`${placement.x*100}%`,top:`${placement.y*100}%`,fontSize:`${overlaySize*pageZoom}px`}">{{overlayText}}</span>
          <div v-if="textMode && placement && placement.page===current" class="pdf-inline-add-card" :style="inlineAddStyle" @click.stop @pointerdown.stop>
            <label>Add text on page {{current}}<textarea ref="inlineAddInput" v-model="overlayText" maxlength="1000" rows="3" :disabled="editing || disabled" placeholder="Type text here" @keydown.esc.stop.prevent="placement=null" @keydown.meta.enter.prevent="placeText" @keydown.ctrl.enter.prevent="placeText"></textarea></label>
            <label>Size <input v-model.number="overlaySize" type="number" min="6" max="36" :disabled="editing || disabled"/> pt</label>
            <p v-if="overlayError || (overlayText && overlayValidation)" class="error-msg" role="alert">{{overlayError || overlayValidation}}</p>
            <div class="pdf-inline-add-actions"><button class="btn secondary small" :disabled="editing" @click="placement=null;overlayError=''">Cancel</button><button class="btn primary small" :disabled="!!overlayValidation || editing || disabled" @click="placeText">Apply text</button></div>
          </div>
          <img v-if="editSnapshot" class="pdf-edit-snapshot" :class="{'is-fading':editSnapshotFading}" :src="editSnapshot" alt="" aria-hidden="true" />
          <span v-if="placement && placement.page===current && !overlayText" class="pdf-placement-marker" :style="{left:`${placement.x*100}%`,top:`${placement.y*100}%`}" aria-hidden="true"></span>
          <button v-for="item in currentPageLinks" :key="item.id" type="button" :class="['pdf-reviewed-link',{unresolved:!item.resolved}]" :style="{left:`${item.rect_x*100}%`,top:`${item.rect_y*100}%`,width:`${item.rect_width*100}%`,height:`${item.rect_height*100}%`}" :title="item.resolved ? item.title : item.title+' — needs review'" @pointerdown.stop @click.stop="followReviewedLink(item)"><span>{{item.title}}</span></button>
          <span v-if="linkPanelOpen && linkRegion" class="pdf-link-region" :style="{left:`${linkRegion.x*100}%`,top:`${linkRegion.y*100}%`,width:`${linkRegion.width*100}%`,height:`${linkRegion.height*100}%`}" aria-hidden="true"></span>
          <template v-for="item in currentPageAnnotations" :key="item.id">
            <img v-if="item.kind==='image' && item.resolved && annotationImageUrls[item.id]" class="pdf-image-overlay" :src="annotationImageUrls[item.id]" :alt="item.body || 'User-added image overlay'" role="button" tabindex="0" :aria-label="item.body || 'Edit image overlay'" :style="{left:`${item.rect_x*100}%`,top:`${item.rect_y*100}%`,width:`${item.rect_width*100}%`,height:`${item.rect_height*100}%`}" @pointerdown.stop @click.stop="editPageAnnotation(item)" @keydown.enter.stop.prevent="editPageAnnotation(item)" @keydown.space.stop.prevent="editPageAnnotation(item)"/>
            <button v-else-if="item.kind==='highlight'" type="button" :class="['pdf-review-highlight',item.color,{unresolved:!item.resolved}]" :style="{left:`${item.rect_x*100}%`,top:`${item.rect_y*100}%`,width:`${item.rect_width*100}%`,height:`${item.rect_height*100}%`}" :title="item.body || 'Private highlight'" @pointerdown.stop @click.stop="editPageAnnotation(item)"></button>
            <button v-else-if="item.kind==='comment'" type="button" :class="['pdf-review-comment',item.color,{unresolved:!item.resolved}]" :style="{left:`${item.rect_x*100}%`,top:`${item.rect_y*100}%`,width:`${item.rect_width*100}%`,height:`${item.rect_height*100}%`}" :title="item.body" @pointerdown.stop @click.stop="editPageAnnotation(item)"><MessageSquare :size="Math.max(12,Math.min(22,item.rect_width*pageZoom*300))"/></button>
            <button v-else-if="item.kind==='image'" type="button" class="pdf-stale-image-overlay" :style="{left:`${item.rect_x*100}%`,top:`${item.rect_y*100}%`,width:`${item.rect_width*100}%`,height:`${item.rect_height*100}%`}" title="Image overlay needs review after a revision change" @pointerdown.stop @click.stop="editPageAnnotation(item)">Image needs review</button>
          </template>
          <span v-if="annotationPanelOpen && annotationRegion" class="pdf-annotation-region" :style="{left:`${annotationRegion.x*100}%`,top:`${annotationRegion.y*100}%`,width:`${annotationRegion.width*100}%`,height:`${annotationRegion.height*100}%`}" aria-hidden="true"></span>
          <img v-if="annotationPanelOpen && annotationRegion && annotationDraft.kind==='image' && annotationPreviewUrl" class="pdf-image-overlay pdf-image-draft" :src="annotationPreviewUrl" alt="Image placement preview" :style="{left:`${annotationRegion.x*100}%`,top:`${annotationRegion.y*100}%`,width:`${annotationRegion.width*100}%`,height:`${annotationRegion.height*100}%`}"/>
          <span v-if="redactionOpen && redactionRegion" class="pdf-redaction-region" :style="{left:`${redactionRegion.x*100}%`,top:`${redactionRegion.y*100}%`,width:`${redactionRegion.width*100}%`,height:`${redactionRegion.height*100}%`}" aria-hidden="true"></span>
          <span v-for="item in markPreview" :key="item.kind" :class="['pdf-output-mark-preview',item.kind,{overlap:item.overlaps_content}]" :style="{left:`${item.relative_rect[0]*100}%`,top:`${item.relative_rect[1]*100}%`,width:`${item.relative_rect[2]*100}%`,height:`${item.relative_rect[3]*100}%`,fontSize:`${item.font_size*pageZoom}px`,opacity:item.opacity,textAlign:item.position.endsWith('left')?'left':item.position.endsWith('right')?'right':'center'}"><span>{{item.text}}</span></span>
          <template v-if="editTextMode">
            <button v-for="run in textRuns" :key="run.index" class="pdf-text-run" :class="{selected:selectedRun?.index===run.index, 'move-target':movingRun?.index===run.index}"
              :style="{left:`${run.relative_bbox[0]*100}%`,top:`${(run.relative_bbox[1]+(movingRun?.index===run.index ? verticalOffset : 0))*100}%`,width:`${(run.relative_bbox[2]-run.relative_bbox[0])*100}%`,height:`${(run.relative_bbox[3]-run.relative_bbox[1])*100}%`}"
              :aria-label="`Edit text: ${run.text}`" :disabled="disabled || editing" @click.stop="clickRun(run)" @pointerdown="beginMove($event, run)" @pointermove="updateMove" @pointerup="finishMove" @pointercancel="finishMove"><span v-if="movingRun?.index===run.index" class="pdf-move-label">{{run.text}}</span></button>
          </template>
          <div v-if="editTextMode && selectedRun" class="pdf-inline-editor" :style="inlineEditorStyle" @click.stop>
            <label :for="`replacement-${selectedRun.index}`">Replacement</label>
            <textarea :id="`replacement-${selectedRun.index}`" ref="inlineReplacementInput" v-model="replacement" maxlength="2000" rows="2" :disabled="editing || disabled || (replacementEngine==='pdfium-trial' && !selectedParagraph)" aria-describedby="inline-edit-hint" @keydown.esc.stop.prevent="cancelReplacement" @keydown.meta.enter.prevent="applyReplacement" @keydown.ctrl.enter.prevent="applyReplacement"></textarea>
            <span id="inline-edit-hint" class="pdf-inline-hint">{{replacementEngine==='pdfium-trial' ? (selectedParagraph ? 'Editing the whole detected paragraph' : 'Loading whole paragraph…') : 'Apply change'}} · Esc to cancel</span>
            <p v-if="runsError" class="error-msg" role="alert">{{runsError}}</p>
            <div class="pdf-inline-actions">
              <button class="btn secondary small" :disabled="editing" @click="cancelReplacement">Cancel</button>
              <button class="btn secondary small" :disabled="!replacement.trim() || editing || disabled || (replacementEngine==='pdfium-trial' && !selectedParagraph)" @click="applyReplacement">Apply replacement</button>
            </div>
          </div>
        </div>
      </div>
      </div>
      </div>
      <div class="document-actionbar" role="group" aria-label="Document tools"><label for="ocr-language">OCR language</label><select id="ocr-language" v-model="ocrLanguage" :disabled="editing || loading || disabled"><option value="eng">English</option><option value="hin">Hindi</option><option value="eng+hin">English + Hindi</option></select><button class="text-tool" :disabled="editing || loading || disabled" @click="emit('ocr', ocrLanguage)"><ScanText :size="15"/> Run {{ ocrLanguage === 'eng' ? 'English' : ocrLanguage === 'hin' ? 'Hindi' : 'English + Hindi' }} OCR</button>
        <button class="text-tool page-icon-tool danger" :disabled="editing || loading || disabled" aria-label="Archive document" title="Archive document" @click="emit('archiveDocument')"><Archive :size="16"/></button>
        <span class="page-editbar-divider" aria-hidden="true"></span>
        <button class="text-tool" :aria-expanded="!!detailsOpen" @click="emit('toggleDetails')"><PanelRightClose v-if="detailsOpen" :size="15"/><PanelRightOpen v-else :size="15"/> Document details<span v-if="detailsDirty" class="details-unsaved-indicator">Unsaved</span></button>
      </div>
    </template>
    <div v-else class="viewer-empty"><FileSearch :size="44"/><strong>Document preview</strong><span>Select a file to review its pages</span></div>
  </div>
</template>
