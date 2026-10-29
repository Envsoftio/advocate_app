<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, reactive, ref, watch } from 'vue'
import { ArrowLeft, Save, Shield, PanelLeft, CheckCircle2, Ellipsis, Maximize2, Minimize2, Archive, BookMarked, ChevronLeft, ChevronRight, RotateCcw, RotateCw, Trash2, FileSearch, FilePenLine, FilePlus2, LayoutGrid, LoaderCircle, Type, Pencil, ScanText, PanelRightOpen, PanelRightClose, Link2, Stamp, ExternalLink, Images, Highlighter, MessageSquare, Eraser, Undo2, Redo2, X, Search, Square, Circle, Minus, ArrowUpRight, Check, CalendarDays, Printer, BookOpen, Info, Focus } from 'lucide-vue-next'
import type { PDFDocumentProxy } from 'pdfjs-dist'
import type { MatterDocument, OutputMarkPreview, OutputMarks, PageAnnotation, ReviewedLink } from '../lib/types'
import { ApiError, api, downloadWorkingPdf, inTauri, pageAnnotationAsset, pdfDocumentSource, uploadPageAnnotationImage } from '../lib/api'
import { engineBlocked } from '../lib/engineConnection'
import { friendlyError } from '../lib/format'
import { confirmSignatureTransformation } from '../lib/signatures'

import PdfReaderViewport from './PdfReaderViewport.vue'
import PdfReaderControls from './PdfReaderControls.vue'
import PdfThumbnailRail from './PdfThumbnailRail.vue'
import PageOrganizer from './PageOrganizer.vue'
import { usePdfDocument, nextZoom, rememberReadingPosition, loadReadingPosition, flushReadingPositions, readingPositionError, type ReaderMode, type ReaderPosition } from '../lib/pdfReader'
import { usePdfSearch } from '../lib/pdfSearch'
import { readPdfSourceOutline, type PdfSourceOutlineEntry } from '../lib/pdfNavigation'
const pdfOwner = usePdfDocument()
const readerPdf = pdfOwner.pdf
const readerViewport = ref<InstanceType<typeof PdfReaderViewport> | null>(null)
const readerMode = ref<ReaderMode>('single')
const zoomChoice = ref('fit-width')
const readingHistory = ref<ReaderPosition[]>([])
type ReadPanel = 'search' | 'outline' | 'comments' | 'page-bookmarks' | 'information' | null
const readPanel = ref<ReadPanel>(null)
const panelElement = ref<HTMLElement | null>(null)
const viewerElement = ref<HTMLElement | null>(null)
let panelTrigger: HTMLElement | null = null
const focusedReading = ref(false)
const actionsMenu = ref<HTMLDetailsElement|null>(null)
const headerSaving = ref(false)
function closeActionMenu(event:Event) {
  if (!(event.target as HTMLElement).closest('button')) return
  actionsMenu.value?.removeAttribute('open')
  actionsMenu.value?.querySelector<HTMLElement>('summary')?.focus({preventScroll:true})
}
async function saveHeaderDraft() {
  if (headerSaving.value) return
  headerSaving.value = true
  try { if (!(await savePendingDraft())) emit('error','Finish the current tool draft before saving. Your draft is retained.') }
  finally { headerSaving.value = false }
}
let focusWasExpanded = false
let focusTrigger: HTMLElement | null = null
function panelFlag(name: ReadPanel) {
  return computed({get: () => readPanel.value === name, set: (open: boolean) => {
    if (open) readPanel.value = name
    else if (readPanel.value === name) readPanel.value = null
  }})
}
const outlineOpen = panelFlag('outline')
async function closeReadPanel() {
  readPanel.value = null
  await nextTick()
  ;(panelTrigger?.isConnected ? panelTrigger : viewerElement.value)?.focus({preventScroll:true})
}
async function openReadPanel(panel: ReadPanel, event?: Event) {
  if (readPanel.value === panel) { await closeReadPanel(); return }
  panelTrigger = event?.currentTarget instanceof HTMLElement ? event.currentTarget : window.document.activeElement as HTMLElement
  readPanel.value = panel
  await nextTick()
  if (panel === 'search') findInput.value?.focus({preventScroll:true})
  else panelElement.value?.querySelector<HTMLButtonElement>('button')?.focus({preventScroll:true})
}
async function toggleFocusedReading(event?: Event) {
  if (!focusedReading.value) {
    focusTrigger = event?.currentTarget instanceof HTMLElement ? event.currentTarget : window.document.activeElement as HTMLElement
    focusWasExpanded = !!props.expanded
    focusedReading.value = true
    if (!props.expanded) emit('toggleExpanded')
    await nextTick()
    viewerElement.value?.querySelector<HTMLButtonElement>('[aria-label="Exit focused reading"]')?.focus({preventScroll:true})
  } else {
    focusedReading.value = false
    if (!focusWasExpanded && props.expanded) emit('toggleExpanded')
    await nextTick()
    ;(focusTrigger?.isConnected ? focusTrigger : viewerElement.value)?.focus({preventScroll:true})
  }
}
function resultKey(event: KeyboardEvent, index: number) {
  const target = event.key === 'ArrowDown' ? Math.min(index+1,findMatches.value.length-1) :
    event.key === 'ArrowUp' ? Math.max(index-1,0) : event.key === 'Home' ? 0 :
    event.key === 'End' ? findMatches.value.length-1 : null
  if (target === null) return
  event.preventDefault();event.stopPropagation()
  chooseFind(target)
  panelElement.value?.querySelectorAll<HTMLButtonElement>('.pdf-find-results button')[target]?.focus({preventScroll:true})
}

const sourceOutline = ref<Array<{title:string,page:number|null,url:string|null,level:number}>>([])
const outlineError = ref('')
const outlineLoading = ref(false)
const commentsOpen = panelFlag('comments')
const commentQuery = ref('')
const pageBookmarksOpen = panelFlag('page-bookmarks')
const organizerOpen = ref(false)
const workingSaveBusy = ref(false)
const workingSaveNotice = ref('')
const workingSaveError = ref('')
async function saveWorkingCopy() {
  const document = props.document
  if (!document || workingSaveBusy.value || props.disabled || editing.value || loading.value) return
  workingSaveBusy.value = true
  workingSaveNotice.value = ''
  workingSaveError.value = ''
  try {
    const hasPdfMarks = pageAnnotations.value.some(item => item.document_id===document.id &&
      item.output_scope==='pdf' && item.resolved)
    const signature = hasPdfMarks
      ? confirmSignatureTransformation([document], 'Saving a PDF with editable marks')
      : {proceed:true,acknowledged:false}
    if (!signature.proceed) return
    const saved = await downloadWorkingPdf(document.id, document.revision, document.title,
      signature.acknowledged)
    workingSaveNotice.value = saved
      ? `${inTauri() ? 'Saved' : 'Download started for'} “${document.title}”, working revision ${document.revision}, with editable PDF marks flattened and staged redactions applied. Private notes are omitted.`
      : 'Save cancelled. No file was written.'
  } catch (cause) { workingSaveError.value = friendlyError(cause) }
  finally { workingSaveBusy.value = false }
}
function printWorkingCopy() {
  const document = props.document
  if (!document || props.disabled || editing.value || loading.value) return
  const hasPdfMarks = pageAnnotations.value.some(item => item.document_id===document.id &&
    item.output_scope==='pdf' && item.resolved)
  const signature = hasPdfMarks
    ? confirmSignatureTransformation([document], 'Printing a PDF with editable marks')
    : {proceed:true,acknowledged:false}
  if (signature.proceed) emit('printWorking', {
    documentId:document.id, revision:document.revision, title:document.title,
    acknowledged:signature.acknowledged,
  })
}
function rememberPosition(position?:ReaderPosition) {
  const key = activeDocumentId && displayedRevision.value ? `${activeDocumentId}:${displayedRevision.value}` : ''
  const at = position || readerViewport.value?.capture()
  if (at) rememberReadingPosition(key,{...at,zoom:zoomChoice.value,mode:readerMode.value})
}
function navigateToPage(page:number, record=true) {
  if (!Number.isInteger(page) || page<1 || page>total.value) return false
  if (page !== current.value && hasPendingDraft()) {
    if (!window.confirm('Discard the unfinished PDF tool draft and change pages? Choose Cancel to keep editing.')) return false
    discardPendingDraft()
  }
  if (record) {
    const at=readerViewport.value?.capture()
    if(at) readingHistory.value=[...readingHistory.value.slice(-49),at]
  }
  current.value=page
  return true
}
async function goBack() {
  const at=readingHistory.value.pop()
  if (!at) return
  current.value=at.page
  await nextTick(); await readerViewport.value?.restore(at)
}
function pageRendered(data:{page:number,scale:number,width:number,height:number}) {
  if(data.page!==current.value)return
  canvas.value=readerViewport.value?.getCanvas() || null
  pageZoom.value=data.scale
  displayedPage.value=data.page
  switchingPage.value=false
  viewerError.value=''
}
async function loadOutline(document:PDFDocumentProxy, call:number) {
  outlineLoading.value=true
  try {
    const result=await readPdfSourceOutline(document,{isCancelled:()=>call!==epoch})
    if(call!==epoch)return
    const rows:Array<{title:string,page:number|null,url:string|null,level:number}>=[]
    function flatten(items:PdfSourceOutlineEntry[],level:number){for(const item of items){rows.push({...item,level});flatten(item.children,level+1)}}
    flatten(result.entries,0);sourceOutline.value=rows
    if(result.truncated)outlineError.value='Showing the first available outline entries.'
  }catch{if(call===epoch)outlineError.value='The source PDF outline could not be read.'}
  finally{if(call===epoch)outlineLoading.value=false}
}
function tabKey(event:KeyboardEvent, category:EditorCategory) {
  const categories:EditorCategory[]=['read','edit','review','pages','ocr','redact']
  let index=categories.indexOf(category)
  if(event.key==='ArrowRight')index=(index+1)%categories.length
  else if(event.key==='ArrowLeft')index=(index+categories.length-1)%categories.length
  else if(event.key==='Home')index=0
  else if(event.key==='End')index=categories.length-1
  else return
  event.preventDefault();selectCategory(categories[index])
  ;(event.currentTarget as HTMLElement).parentElement?.querySelector<HTMLButtonElement>(`[data-category="${categories[index]}"]`)?.focus()
}

const props = defineProps<{ preparation?:boolean, artifactOptions?: Array<{value:string,label:string}>, expanded?: boolean, focusMode?: boolean, document: MatterDocument | null, documents?: MatterDocument[], busy?: boolean, disabled?: boolean, canUndo?: boolean, canRedo?: boolean, undoLabel?: string | null, redoLabel?: string | null, detailsOpen?: boolean, detailsDirty?: boolean, bookmarkRefreshKey?: number, pageTarget?: {documentId:string, page:number, sequence:number, revision?:number} }>()
const emit = defineEmits<{ openEditor:[], showDocumentList:[], browseDocuments: [], saveDetails: [], retryReader: [], artifact: [value:string], toggleExpanded: [], toggleFocus: [], modified: [], undo: [], redo: [], error: [message: string], pageTools: [], toggleDetails: [], openBookmarks: [], ocr: [language: 'eng' | 'hin' | 'eng+hin'], archiveDocument: [], openInternalLink: [documentId:string,page:number], openExternalLink: [uri:string], printWorking: [request:{documentId:string,revision:number,title:string,acknowledged:boolean}] }>()
const undoDescription = computed(() => props.undoLabel
  ? `Undo: ${props.undoLabel}. Applies to the last change anywhere in this matter.`
  : 'Undo last matter change. No change available.')
const redoDescription = computed(() => props.redoLabel
  ? `Redo: ${props.redoLabel}. Restores the next undone change anywhere in this matter.`
  : 'Redo matter change. No change available.')
const ocrLanguage = ref<'eng' | 'hin' | 'eng+hin'>('eng')
const canvas = ref<HTMLCanvasElement | null>(null)
const current = ref(1)
const loading = ref(false)
const editing = ref(false)
const switchingPage = ref(false)
const rotatingPage = ref(false)
const viewerError = ref('')
function zoomBy(step:number) { zoomChoice.value=nextZoom(zoomChoice.value,pageZoom.value,step) }
function toggleExpandedView() {
  if (!props.expanded) zoomChoice.value = 'fit-page'
  emit('toggleExpanded')
}
const pageZoom = ref(1)
const showThumbnails = ref(true)
const thumbnailsVisible = computed(() => showThumbnails.value && !readPanel.value && !focusedReading.value)
function toggleThumbnails() {
  if(readPanel.value) { readPanel.value=null;showThumbnails.value=true }
  else showThumbnails.value=!showThumbnails.value
}
type EditorCategory = 'read' | 'edit' | 'review' | 'stamps' | 'pages' | 'output' | 'ocr' | 'redact'
const activeCategory = ref<EditorCategory>('read')
watch(()=>props.preparation,preparing=>{
  if(preparing) { selectCategory('read');readPanel.value=null;focusedReading.value=false }
})
const findOpen = panelFlag('search')
const findInput = ref<HTMLInputElement | null>(null)
const findQuery = ref('')
const {matches:findMatches,selectedIndex:findIndex,busy:findBusy,error:findError,truncated:findTruncated,select:selectFind,step:advanceFind} = usePdfSearch(readerPdf,findQuery)
function chooseFind(index:number){
  selectFind(index)
  const match=findMatches.value[index]
  if(match){navigateToPage(match.page);void nextTick(()=>readerViewport.value?.scrollToMatch(index))}
}
function stepFind(step:number){advanceFind(step);chooseFind(findIndex.value)}
const draggingPage = ref<number | null>(null)
const dropTargetPage = ref<number | null>(null)
type ManualBookmark = {id:string,title:string,document_id:string,page_index:number,target_revision:number,parent_id:string|null,resolved:boolean}
const manualBookmarks = ref<ManualBookmark[]>([])
const bookmarksLoading = ref(false), bookmarksError = ref('')
const documentBookmarks = computed(() => manualBookmarks.value.filter(item => item.document_id === props.document?.id))
const bookmarkOpen = ref(false)
const bookmarkTitle = ref('')
const bookmarkSaving = ref(false)
const bookmarkError = ref('')
const bookmarkNotice = ref('')
let bookmarkNoticeTimer: ReturnType<typeof setTimeout> | null = null
let bookmarkLoadSequence = 0
const pageBookmarks = computed(() => manualBookmarks.value.filter(item =>
  item.document_id === props.document?.id && item.page_index === current.value - 1))
const loadedPageCount = ref(0)
type EditorTool = 'select' | 'add-text' | 'edit-text' | 'redact' | 'notes' | 'links' | 'marks'
const activeTool = ref<EditorTool>('select')
const textMode = computed(() => activeTool.value === 'add-text')
const editTextMode = computed(() => activeTool.value === 'edit-text')
const movingRun = ref<TextRun | null>(null)
const horizontalOffset = ref(0)
const verticalOffset = ref(0)
const pendingMove = ref(false)
let pointerRun: TextRun | null = null
let pointerId: number | null = null
let dragStartX = 0
let dragStartY = 0
let suppressNextClick = false
const overlayText = ref('')
const overlaySize = ref(12)
const overlayValidation = computed(() => {
  if (!overlayText.value.trim()) return 'Enter text to add.'
  if (/[^\x20-\x7E\n]/.test(overlayText.value)) return 'Add text currently supports printable ASCII and line breaks only.'
  if (!Number.isFinite(overlaySize.value) || overlaySize.value < 6 || overlaySize.value > 36)
    return 'Choose a font size from 6 to 36 pt.'
  return ''
})
let lastOverlayValidation = ''
watch([overlayText, overlaySize], () => {
  const validation = overlayText.value ? overlayValidation.value : ''
  if (validation && validation !== lastOverlayValidation) emit('error', validation)
  lastOverlayValidation = validation
})
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
type SelectedOverlay = {kind:'annotation'|'link',id:string}
const selectedOverlay = ref<SelectedOverlay | null>(null)
const selectedOverlayLabel = computed(() => {
  const selected=selectedOverlay.value
  if (!selected) return ''
  if (selected.kind==='link') return 'Reviewed link'
  const item=pageAnnotations.value.find(entry=>entry.id===selected.id)
  return item ? annotationKindLabel(item.kind) : 'Page mark'
})
const overlayOffset = ref({x:0,y:0})
const overlayResizeSize = ref<{width:number,height:number}|null>(null)
const selectedOverlayItem = computed<PageAnnotation|ReviewedLink|null>(() => {
  const selected=selectedOverlay.value
  return selected?.kind==='annotation'
    ? pageAnnotations.value.find(item=>item.id===selected.id) || null
    : selected?.kind==='link' ? reviewedLinks.value.find(item=>item.id===selected.id) || null : null
})
let overlayDrag: {pointerId:number,startX:number,startY:number,item:PageAnnotation|ReviewedLink,kind:SelectedOverlay['kind']} | null = null
let overlayResize: {pointerId:number,startX:number,startY:number,item:PageAnnotation|ReviewedLink,kind:SelectedOverlay['kind']} | null = null
let suppressOverlayClick = false
function overlaySelected(kind:SelectedOverlay['kind'],id:string) {
  return selectedOverlay.value?.kind===kind && selectedOverlay.value.id===id
}
function overlayStyle(kind:SelectedOverlay['kind'],id:string) {
  if (!overlaySelected(kind,id) || !canvas.value) return {}
  const box=canvas.value.getBoundingClientRect()
  return {transform:`translate(${overlayOffset.value.x*box.width}px, ${overlayOffset.value.y*box.height}px)`,
    ...(overlayResizeSize.value?{width:`${overlayResizeSize.value.width*100}%`,height:`${overlayResizeSize.value.height*100}%`}:{})}
}
function clearOverlaySelection() {
  selectedOverlay.value=null
  overlayOffset.value={x:0,y:0}
  overlayResizeSize.value=null
  overlayDrag=null
  overlayResize=null
}
function beginOverlayDrag(event:PointerEvent,kind:SelectedOverlay['kind'],item:PageAnnotation|ReviewedLink) {
  if (event.button!==0 || props.disabled || editing.value || !pageReady.value || !item.resolved) return
  event.stopPropagation()
  selectedOverlay.value={kind,id:item.id}
  overlayOffset.value={x:0,y:0}
  overlayDrag={pointerId:event.pointerId,startX:event.clientX,startY:event.clientY,item,kind}
  ;(event.currentTarget as HTMLElement).focus()
  ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
}
function updateOverlayDrag(event:PointerEvent) {
  const drag=overlayDrag
  if (!drag || drag.pointerId!==event.pointerId || !canvas.value) return
  const box=canvas.value.getBoundingClientRect()
  if (!box.width || !box.height) return
  const dx=(event.clientX-drag.startX)/box.width
  const dy=(event.clientY-drag.startY)/box.height
  overlayOffset.value={
    x:Math.max(-drag.item.rect_x,Math.min(1-drag.item.rect_x-drag.item.rect_width,dx)),
    y:Math.max(-drag.item.rect_y,Math.min(1-drag.item.rect_y-drag.item.rect_height,dy)),
  }
  event.preventDefault()
}
async function finishOverlayDrag(event:PointerEvent) {
  const drag=overlayDrag
  if (!drag || drag.pointerId!==event.pointerId) return
  overlayDrag=null
  const {x,y}=overlayOffset.value
  if (event.type!=='pointerup' || Math.hypot(x,y)<.0001) { overlayOffset.value={x:0,y:0}; return }
  suppressOverlayClick=true
  window.setTimeout(()=>{suppressOverlayClick=false},0)
  await persistOverlayRect(drag.kind,drag.item,{
    x:drag.item.rect_x+x,y:drag.item.rect_y+y,
    width:drag.item.rect_width,height:drag.item.rect_height})
  overlayOffset.value={x:0,y:0}
}
function beginOverlayResize(event:PointerEvent) {
  const selected=selectedOverlay.value,item=selectedOverlayItem.value
  if (!selected || !item || !item.resolved || event.button!==0 || props.disabled || editing.value || !pageReady.value) return
  overlayResize={pointerId:event.pointerId,startX:event.clientX,startY:event.clientY,item,kind:selected.kind}
  ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
  event.stopPropagation()
}
function updateOverlayResize(event:PointerEvent) {
  const drag=overlayResize
  if (!drag || drag.pointerId!==event.pointerId || !canvas.value) return
  const box=canvas.value.getBoundingClientRect()
  if (!box.width || !box.height) return
  overlayResizeSize.value={
    width:Math.max(.01,Math.min(1-drag.item.rect_x,drag.item.rect_width+(event.clientX-drag.startX)/box.width)),
    height:Math.max(.01,Math.min(1-drag.item.rect_y,drag.item.rect_height+(event.clientY-drag.startY)/box.height)),
  }
  event.preventDefault()
}
async function finishOverlayResize(event:PointerEvent) {
  const drag=overlayResize
  if (!drag || drag.pointerId!==event.pointerId) return
  overlayResize=null
  const size=overlayResizeSize.value
  if (event.type==='pointerup' && size &&
      (Math.abs(size.width-drag.item.rect_width)>.0001 || Math.abs(size.height-drag.item.rect_height)>.0001)) {
    await persistOverlayRect(drag.kind,drag.item,{
      x:drag.item.rect_x,y:drag.item.rect_y,width:size.width,height:size.height})
  }
  overlayResizeSize.value=null
}
async function persistOverlayRect(kind:SelectedOverlay['kind'],item:PageAnnotation|ReviewedLink,
    rect:{x:number,y:number,width:number,height:number}) {
  const doc=props.document
  if (!doc || props.disabled) return
  try {
    if (kind==='annotation') {
      annotationBusy.value=true
      pageAnnotations.value=await api<PageAnnotation[]>(`/annotations/${item.id}`,'PATCH',{
        expected_revision:doc.revision,rect_x:rect.x,rect_y:rect.y,
        rect_width:rect.width,rect_height:rect.height,
        body:(item as PageAnnotation).body,color:(item as PageAnnotation).color,
      })
    } else {
      linksBusy.value=true
      const existing=await api<ReviewedLink[]>(`/matters/${doc.matter_id}/links`)
      if (props.document?.id!==doc.id || props.document.revision!==doc.revision) throw new Error('Document changed. Select the link again.')
      reviewedLinks.value=await api<ReviewedLink[]>(`/matters/${doc.matter_id}/links`,'PUT',{
        entries:existing.map(entry=>entry.id===item.id?{
          ...linkPayload(entry),rect_x:rect.x,rect_y:rect.y,
          rect_width:rect.width,rect_height:rect.height,
        }:linkPayload(entry)),
      })
    }
    emit('modified')
  } catch (err) {emit('error',friendlyError(err))}
  finally {annotationBusy.value=false;linksBusy.value=false}
}
async function deleteSelectedOverlay() {
  const selected=selectedOverlay.value
  if (!selected || props.disabled || editing.value || annotationBusy.value || linksBusy.value) return
  const annotation=selected.kind==='annotation'?pageAnnotations.value.find(item=>item.id===selected.id):null
  const link=selected.kind==='link'?reviewedLinks.value.find(item=>item.id===selected.id):null
  if (!annotation && !link) {clearOverlaySelection();return}
  // Only authored overlays are removed here. Working-PDF pixels and original content use guarded edit tools.
  try {
    if (annotation) {
      annotationBusy.value=true
      pageAnnotations.value=await api<PageAnnotation[]>(`/annotations/${annotation.id}`,'DELETE')
      await loadCurrentAnnotationImages()
    } else if (link && props.document) {
      linksBusy.value=true
      const doc=props.document
      const existing=await api<ReviewedLink[]>(`/matters/${doc.matter_id}/links`)
      reviewedLinks.value=await api<ReviewedLink[]>(`/matters/${doc.matter_id}/links`,'PUT',
        {entries:existing.filter(item=>item.id!==link.id).map(linkPayload)})
    }
    clearOverlaySelection()
    emit('modified')
  } catch (err) {emit('error',friendlyError(err))}
  finally {annotationBusy.value=false;linksBusy.value=false}
}
const annotationPanelOpen = computed(() => activeTool.value === 'notes')
const redactionOpen = computed(() => activeTool.value === 'redact')
const redactionCorners = ['nw','ne','sw','se'] as const
type AppliedRedaction = {id:string,document_id:string,page_index:number,rect_x:number,rect_y:number,
  rect_width:number,rect_height:number,restore_x:number,restore_y:number,
  restore_width:number,restore_height:number,before_revision:number,after_revision:number,
  structure_changed:number}
const appliedRedactions = ref<AppliedRedaction[]>([])
const selectedRedactionId = ref<string | null>(null)
const selectedRedaction = computed(() => appliedRedactions.value.find(item => item.id === selectedRedactionId.value) || null)
const redactionRegion = ref<{x:number,y:number,width:number,height:number} | null>(null)
const redactionBusy = ref(false)
const redactionError = ref('')
const redactionNotice = ref('')
let redactionStart: {x:number,y:number} | null = null
let redactionPointerId: number | null = null
let redactionResize: {pointerId:number,anchor:{x:number,y:number}} | null = null
let redactionMove: {pointerId:number,start:{x:number,y:number},region:{x:number,y:number,width:number,height:number}} | null = null
let redactionRevision = 0
const annotationDrawing = ref(false)
const annotationRegion = ref<{x:number,y:number,width:number,height:number} | null>(null)
const annotationDrawPoints = ref<Array<{x:number,y:number}>>([])
let annotationStart: {x:number,y:number} | null = null
let annotationPointerId: number | null = null
const pageAnnotations = ref<PageAnnotation[]>([])
const annotationBusy = ref(false)
const annotationError = ref('')
const annotationNotice = ref('')
const annotationImage = ref<File | null>(null)
const annotationPreviewUrl = ref('')
const annotationFileInput = ref<HTMLInputElement | null>(null)
const annotationImageUrls = reactive<Record<string,string>>({})
const annotationDraft = reactive({kind:'highlight' as PageAnnotation['kind'], body:'', color:'yellow' as PageAnnotation['color']})
const annotationOutputScope = ref<'private'|'pdf'>('pdf')
const editingAnnotationId = ref<string | null>(null)
watch(pageAnnotations, entries => {
  if (editingAnnotationId.value && !entries.some(item=>item.id===editingAnnotationId.value)) {
    resetAnnotationDraft()
    activeTool.value = 'select'
  }
  if (selectedOverlay.value?.kind === 'annotation' && !entries.some(item=>item.id===selectedOverlay.value?.id))
    clearOverlaySelection()
})
const currentPageAnnotations = computed(() => pageAnnotations.value.filter(item =>
  item.document_id === props.document?.id && item.page_index === current.value - 1))
const documentComments = computed(()=>pageAnnotations.value.filter(item=>item.document_id===props.document?.id && item.kind==='comment'))
const filteredComments = computed(() => {
  const query = commentQuery.value.trim().toLocaleLowerCase()
  return documentComments.value.filter(item => !query ||
    item.body.toLocaleLowerCase().includes(query) || String(item.page_index + 1).includes(query))
    .sort((a,b) => a.page_index - b.page_index)
})
const marksPanelOpen = computed(() => activeTool.value === 'marks')
const inspectorVisible = computed(() => marksPanelOpen.value ||
  (editTextMode.value && (pendingMove.value || !!runsError.value || !!editNotice.value)) ||
  redactionOpen.value ||
  (annotationPanelOpen.value && (annotationDraft.kind === 'image' || (!annotationDrawing.value && (!!annotationRegion.value || !!editingAnnotationId.value || !!annotationError.value)))) ||
  (linkPanelOpen.value && !!linkRegion.value))
const toolHint = computed(() => ({
  'add-text': 'Click where text should appear, type in the box, then choose Apply text.',
  'edit-text': 'Click selectable text to edit it; drag a highlighted run in any direction to preview a move.',
  redact: 'Drag over content, add the mark, then move or resize it before saving a redacted PDF.',
  notes: annotationDraft.kind === 'image' ? 'Choose an image, then drag its size and position on the page.' : `Drag on the page to place a ${annotationKindLabel(annotationDraft.kind).toLowerCase()} mark.`,
  links: 'Drag a rectangle on the page to create a link.',
  marks: 'Preview output marks on this page; new builds will include saved settings.',
  select: '',
})[activeTool.value])
const toolFeedback = computed(() => activeTool.value === 'redact' ? (redactionError.value || redactionNotice.value) :
  activeTool.value === 'notes' ? annotationError.value :
  activeTool.value === 'links' ? (linkError.value || linkNotice.value) :
  activeTool.value === 'edit-text' ? (runsError.value || editNotice.value) : '')
const marksBusy = ref(false)
const marksError = ref('')
const marksNotice = ref('')
const markPreview = ref<OutputMarkPreview[]>([])
const defaultMarks: OutputMarks = {
  volume_max_bytes:null,volume_splits:{},processing_quality:'original',cleanup_metadata:false,print_layout:'screen',office_docx:false,office_xlsx:false,office_pptx:false,
  numbering_style:'arabic',numbering_restart:'volume',numbering_start:1,numbering_prefix:'',numbering_position:'bottom_center',numbering_front_matter:'physical',numbering_pdf_labels:true,numbering_linked_index:true,numbering_back_link:false,
  index_style:'compact',part_extracts_enabled:false,front_matter:'index_only',index_date:false,cover_heading:'',cover_reference:'',cover_title:'',
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
const savedMarks = ref(JSON.stringify(defaultMarks))
let markPreviewTimer: ReturnType<typeof setTimeout> | null = null
let markLoadSequence = 0
let markPreviewSequence = 0
const marksDirty = computed(() => {
  const saved = JSON.parse(savedMarks.value) as OutputMarks
  return (Object.keys(markSettings) as Array<keyof OutputMarks>).some(key => JSON.stringify(markSettings[key]) !== JSON.stringify(saved[key]))
})
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
let epoch = 0
let activeDocumentId: string | null = null
const displayedRevision = ref<number | null>(null)
const displayedPage = ref(0)
const pageReady = computed(() => !loading.value && !switchingPage.value &&
  displayedPage.value === current.value && displayedRevision.value === props.document?.revision &&
  activeDocumentId === props.document?.id)
function closeDraftWithEscape(event: KeyboardEvent) {
  if (focusedReading.value) void toggleFocusedReading()
  else if (readPanel.value) void closeReadPanel()
  else if (selectedOverlay.value) clearOverlaySelection()
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
    if (event.key === '0') zoomChoice.value = 'fit-width'
    else zoomBy(event.key === '-' ? -1 : 1)
    return
  }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'f') {
    event.preventDefault()
    event.stopPropagation()
    if(focusedReading.value) void toggleFocusedReading().then(() => openReadPanel('search', event))
    else if(findOpen.value) findInput.value?.focus()
    else void openReadPanel('search',event)
    return
  }
  if (event.key === 'Escape') { closeDraftWithEscape(event); return }
  if (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey ||
      (event.target instanceof HTMLElement && event.target.closest('input, textarea, select, [contenteditable="true"]'))) return
  if ((event.key==='Delete' || event.key==='Backspace') && selectedOverlay.value) {
    event.preventDefault()
    event.stopPropagation()
    void deleteSelectedOverlay()
    return
  }
  if (['PageDown','PageUp','Home','End'].includes(event.key)) {
    event.preventDefault()
    navigateToPage(event.key==='Home'?1:event.key==='End'?total.value:Math.max(1,Math.min(total.value,current.value+(event.key==='PageDown'?1:-1))))
    return
  }
  if (focusedReading.value) return
  if(props.preparation && ['v','e','t','h'].includes(event.key.toLowerCase())) emit('openEditor')
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
  if(focusedReading.value && category!=='read') void toggleFocusedReading()
  if (activeCategory.value === category) return
  if (category === 'read' && hasPendingDraft()) {
    if (!window.confirm('Discard the unfinished PDF tool draft and return to Read?')) return
    discardPendingDraft()
  }
  activeCategory.value = category
  if (activeTool.value !== 'select') toggleTool(activeTool.value)
  if (category === 'redact') toggleTool('redact')
}
function showFind(event?: Event) { void openReadPanel('search',event) }
async function openReviewComment(item:PageAnnotation) {
  if (!navigateToPage(item.page_index+1)) return
  commentsOpen.value=false
  activeCategory.value='review'
  await nextTick()
  activeTool.value='notes'
  resetAnnotationDraft()
  annotationDraft.kind='comment'
  editPageAnnotation(item)
}
function toggleTool(tool: EditorTool) {
  clearOverlaySelection()
  activeTool.value = activeTool.value === tool ? 'select' : tool
  // Editing handles need enough room even when opening the inspector shrinks
  // the page after the tool switch. Numeric zoom choices remain intact.
  if (activeTool.value !== 'select' && zoomChoice.value === 'fit-page')
    zoomChoice.value = 'fit-width'
  replacementEngine.value = 'pymupdf'
  placement.value = null
  selectedRun.value = null
  paragraphPreviewSequence++
  cancelMove()
  linkRegion.value = null
  redactionRegion.value = null
  resetAnnotationDraft()
  if (activeTool.value === 'links' && props.document)
    linkDraft.target_document_id ||= props.document.id
}
function openAnnotationTool(kind: PageAnnotation['kind']) {
  if (activeTool.value === 'notes' && annotationDraft.kind === kind) {
    toggleTool('notes')
    return
  }
  if (activeTool.value === 'notes') resetAnnotationDraft()
  else toggleTool('notes')
  annotationNotice.value = ''
  annotationDraft.kind = kind
  annotationOutputScope.value = kind==='highlight' || kind==='comment'?'private':'pdf'
  if (kind==='date') {
    const today=new Date()
    annotationDraft.body=`${today.getFullYear()}-${String(today.getMonth()+1).padStart(2,'0')}-${String(today.getDate()).padStart(2,'0')}`
  }
}
function annotationKindLabel(kind:PageAnnotation['kind']) {
  return ({image:'Image overlay',highlight:'Highlight',comment:'Comment',draw:'Draw',box:'Box',
    ellipse:'Ellipse',line:'Line',arrow:'Arrow',check:'Check',cross:'Cross',date:'Date mark',
    text:'Text',redaction:'Redaction'})[kind]
}
function annotationTime(item:PageAnnotation) {
  const timestamp=item.updated_at || item.created_at
  if (!timestamp) return 'Time not recorded'
  const date=new Date(timestamp)
  return Number.isNaN(date.getTime()) ? 'Time not recorded' :
    new Intl.DateTimeFormat(undefined,{dateStyle:'medium',timeStyle:'short'}).format(date)
}
function drawPolyline(body:string) {
  try {
    const data=JSON.parse(body) as {v:number,points:number[][]}
    if (data.v!==1 || !Array.isArray(data.points)) return ''
    return data.points.filter(point=>Array.isArray(point) && point.length===2 &&
      point.every(value=>typeof value==='number' && Number.isFinite(value) && value>=0 && value<=1))
      .slice(0,160).map(([x,y])=>`${x*100},${y*100}`).join(' ')
  } catch {return ''}
}
const toolTitle = computed(() => ({
  'add-text': 'Add text',
  'edit-text': 'Edit text',
  redact: 'Redact',
  notes: annotationKindLabel(annotationDraft.kind),
  links: 'Reviewed links',
  marks: 'Output marks',
  select: 'PDF tool',
})[activeTool.value])
const total = computed(() => loadedPageCount.value || props.document?.page_count || 0)
async function loadPdf(doc: MatterDocument | null, force = false) {
  if (!force && doc && pdf && doc.id === activeDocumentId && doc.revision === displayedRevision.value) return
  rememberPosition()
  const call = ++epoch
  const sameDocument = doc?.id === activeDocumentId
  const retainedPage = sameDocument ? current.value : 1
  if (!sameDocument) {
    activeTool.value='select';readingHistory.value=[]
    editSnapshot.value='';editSnapshotFading.value=false
  }
  activeDocumentId = doc?.id ?? null
  loading.value = true
  switchingPage.value = true
  viewerError.value = ''
  current.value = retainedPage
  loadedPageCount.value = 0
  placement.value = null
  selectedRun.value = null
  paragraphPreviewSequence++
  cancelMove()
  canvas.value = null
  draggingPage.value=null;dropTargetPage.value=null
  pdf = null
  sourceOutline.value=[];outlineError.value='';outlineLoading.value=false
  displayedRevision.value = null
  displayedPage.value = 0
  // Invalidate the previous PDF, search, and canvases before navigation metadata
  // can wait on I/O. The viewport retains layout for a same-document edit snapshot.
  void pdfOwner.close()
  if (!doc) { loading.value=false; return }
  const saved = await loadReadingPosition(`${doc.id}:${doc.revision}`)
  if(call!==epoch)return
  current.value=saved?.page || retainedPage
  if(saved){zoomChoice.value=saved.zoom;readerMode.value=saved.mode}
  else if(!sameDocument){zoomChoice.value='fit-width';readerMode.value='single'}
  try {
    const loaded = await pdfOwner.open(() => pdfDocumentSource(doc.id, doc.revision))
    if(call!==epoch || !loaded){if(call===epoch)viewerError.value=pdfOwner.error.value;return}
    pdf = loaded
    loadedPageCount.value = loaded.numPages
    current.value = Math.min(current.value, loaded.numPages)
    displayedRevision.value=doc.revision
    if (props.pageTarget?.documentId === doc.id && props.pageTarget.sequence !== consumedPageTarget) {
      const target = props.pageTarget
      consumedPageTarget = target.sequence
      if ((target.revision !== undefined && target.revision !== doc.revision) || target.page < 1 || target.page > loaded.numPages) {
        viewerError.value = 'The requested finding page or revision is unavailable. Return to Preflight and run checks again.'
        pdf = null; void pdfOwner.close(); return
      }
      current.value = target.page
    }
    void loadOutline(loaded,call)
    await nextTick()
    if(call!==epoch)return
    if(saved && (!props.pageTarget || props.pageTarget.documentId!==doc.id)) await readerViewport.value?.restore(saved)
    if(call!==epoch)return
    const painted=await readerViewport.value?.waitForPage(current.value)
    if(call===epoch && !painted && !viewerError.value)viewerError.value='The page could not finish rendering. Reopen the document to retry.'
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
let consumedPageTarget = -1
watch(() => props.pageTarget?.sequence, () => {
  const target = props.pageTarget
  if (target && target.documentId === props.document?.id && pdf) {
    consumedPageTarget = target.sequence
    if ((target.revision !== undefined && target.revision !== displayedRevision.value) || target.page < 1 || target.page > total.value) {
      emit('error', 'The requested finding page or revision is unavailable. Run checks again.'); return
    }
    navigateToPage(target.page)
  }
})
watch(current, () => {switchingPage.value=true;canvas.value=null})
watch(current, () => { placement.value = null; selectedRun.value = null; cancelMove(); clearOverlaySelection() })
watch(() => `${props.document?.id || ''}|${props.document?.revision || ''}`, clearOverlaySelection)
watch(placement, (point, previous) => {
  if (point && !previous) void nextTick(() => inlineAddInput.value?.focus())
})
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
watch(() => `${props.document?.id || ''}|${props.document?.revision || ''}`, () => {
  bookmarkOpen.value = false
  pageBookmarksOpen.value = false
  bookmarkTitle.value = ''
  bookmarkNotice.value = ''
})
watch(() => props.document?.id, () => {
  // Search stays open across document changes; usePdfSearch clears stale matches
  // as soon as the old PDF closes, before position metadata can wait on I/O.
  activeCategory.value='read'
  outlineOpen.value = false
  commentsOpen.value = false
  commentQuery.value = ''
})
watch(() => `${props.document?.id || ''}|${props.document?.revision || ''}|${props.bookmarkRefreshKey || 0}`, async () => {
  const sequence = ++bookmarkLoadSequence
  const doc = props.document
  if (!doc) { manualBookmarks.value = []; return }
  bookmarksLoading.value=true;bookmarksError.value='';manualBookmarks.value=[]
  try {
    const entries = await api<ManualBookmark[]>(`/matters/${doc.matter_id}/bookmarks`)
    if (sequence === bookmarkLoadSequence) manualBookmarks.value = entries
  } catch { if (sequence === bookmarkLoadSequence) bookmarksError.value='Manual bookmarks could not be loaded. Reopen the document to retry.' }
  finally { if(sequence === bookmarkLoadSequence) bookmarksLoading.value=false }
}, {immediate:true})
let linksLoadSequence = 0
watch(() => `${props.document?.matter_id || ''}|${(props.documents || []).map(item => `${item.id}:${item.revision}:${item.archived_at || ''}`).join(',')}|${props.bookmarkRefreshKey || 0}`, async () => {
  const call = ++linksLoadSequence
  const doc = props.document
  reviewedLinks.value = []
  if (!doc) return
  try { const result = await api<ReviewedLink[]>(`/matters/${doc.matter_id}/links`); if(call === linksLoadSequence) reviewedLinks.value = result }
  catch { if(call === linksLoadSequence) reviewedLinks.value = [] }
}, {immediate:true})
watch(() => `${props.document?.matter_id || ''}|${props.document?.revision || ''}|${props.bookmarkRefreshKey || 0}`, () => void loadPageAnnotations(), {immediate:true})
watch([current, pageAnnotations], () => void loadCurrentAnnotationImages())
let marksMatterId = ''
watch(() => [props.document?.matter_id || '', props.bookmarkRefreshKey || 0] as const, async ([matterId]) => {
  const sequence = ++markLoadSequence
  if (marksMatterId !== matterId) { Object.assign(markSettings, defaultMarks); savedMarks.value = JSON.stringify(defaultMarks); marksMatterId = matterId }
  if (!matterId) { Object.assign(markSettings, defaultMarks); savedMarks.value = JSON.stringify(defaultMarks); return }
  try {
    const settings = await api<OutputMarks>(`/matters/${matterId}/output-marks`)
    if (sequence !== markLoadSequence || marksDirty.value) return
    Object.assign(markSettings, settings)
    savedMarks.value = JSON.stringify(settings)
  } catch (err) { if (sequence === markLoadSequence) marksError.value = friendlyError(err) }
}, {immediate:true})
watch(() => [marksPanelOpen.value, props.document?.id, props.document?.revision, current.value,
  JSON.stringify(markSettings)] as const, () => scheduleMarkPreview())
watch(current, () => { bookmarkOpen.value = false; pageBookmarksOpen.value = false; bookmarkError.value = ''; bookmarkNotice.value = '' })
watch(current, () => { linkRegion.value = null; linkDrawing.value = false; linkError.value = ''; linkNotice.value = ''; resetAnnotationDraft(false) })
watch([current, () => props.document?.id, () => props.document?.revision], () => {
  redactionRegion.value = null; redactionError.value = ''
  selectedRedactionId.value = null
})
watch(() => [props.document?.id, props.document?.revision] as const, async ([docId]) => {
  appliedRedactions.value = []
  if (!docId) return
  try {
    const items = await api<AppliedRedaction[]>(`/documents/${docId}/pages/redactions`)
    if (props.document?.id === docId) appliedRedactions.value = items
  } catch { /* The viewer can still display legacy PDFs without redaction records. */ }
}, {immediate:true})
function hasPendingAnnotationDraft() {
  if (editingAnnotationId.value) {
    const saved=pageAnnotations.value.find(item=>item.id===editingAnnotationId.value)
    const region=annotationRegion.value
    if (!saved || !region) return true
    return saved.rect_x!==region.x || saved.rect_y!==region.y ||
      saved.rect_width!==region.width || saved.rect_height!==region.height ||
      saved.body!==annotationDraft.body || saved.color!==annotationDraft.color ||
      saved.output_scope!==annotationOutputScope.value
  }
  return !!(annotationRegion.value || annotationImage.value ||
    ((annotationDraft.kind==='highlight' || annotationDraft.kind==='comment') && annotationDraft.body.trim()))
}
function hasPendingDraft() {
  return !!(marksDirty.value || overlayText.value.trim() || placement.value || replacement.value.trim() ||
    pendingMove.value || redactionRegion.value || hasPendingAnnotationDraft() ||
    linkRegion.value || (bookmarkOpen.value && bookmarkTitle.value.trim()))
}
function discardPendingDraft() {
  Object.assign(markSettings, JSON.parse(savedMarks.value))
  overlayText.value=''; placement.value=null; replacement.value=''; selectedRun.value=null
  cancelMove(); redactionRegion.value=null; annotationRegion.value=null
  annotationImage.value=null; annotationDraft.body=''; editingAnnotationId.value=null
  linkRegion.value=null; bookmarkTitle.value=''
  if (annotationPreviewUrl.value) URL.revokeObjectURL(annotationPreviewUrl.value)
  annotationPreviewUrl.value=''
}
async function savePendingDraft() {
  // An incomplete tool has no valid commit yet; keep its draft and origin open.
  if (marksDirty.value) { await saveMarks(); if (marksDirty.value) return false }
  if (pendingMove.value && movingRun.value) await applyMove(movingRun.value, horizontalOffset.value, verticalOffset.value)
  else if (placement.value && overlayText.value.trim()) await placeText()
  else if (selectedRun.value && replacement.value.trim() && !pendingMove.value) await applyReplacement()
  else if (redactionRegion.value) await applyRedaction()
  else if (hasPendingAnnotationDraft() && (annotationRegion.value || editingAnnotationId.value)) await savePageAnnotation()
  else if (linkRegion.value) await saveReviewedLink()
  else if (bookmarkOpen.value && bookmarkTitle.value.trim()) await addPageBookmark()
  return !hasPendingDraft()
}
const historyBusy = computed(() => editing.value || headerSaving.value || annotationBusy.value || linksBusy.value || redactionBusy.value)
defineExpose({historyBusy, hasPendingDraft, discardPendingDraft, savePendingDraft, reloadReader: () => loadPdf(props.document,true)})
onBeforeUnmount(() => { rememberPosition(); void flushReadingPositions(); epoch++; if (bookmarkNoticeTimer) clearTimeout(bookmarkNoticeTimer); if (markPreviewTimer) clearTimeout(markPreviewTimer); clearAnnotationImageUrls(); if (annotationPreviewUrl.value) URL.revokeObjectURL(annotationPreviewUrl.value) })

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
    if (bookmarkNoticeTimer) clearTimeout(bookmarkNoticeTimer)
    bookmarkNoticeTimer = setTimeout(() => { bookmarkNotice.value = ''; bookmarkNoticeTimer = null }, 5000)
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
  if (annotationDraft.kind==='draw') {annotationDrawPoints.value=[point];annotationDraft.body=''}
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
  if (annotationDraft.kind==='draw' && annotationDrawPoints.value.length<160) {
    const last=annotationDrawPoints.value[annotationDrawPoints.value.length-1]
    if (!last || Math.hypot(point.x-last.x,point.y-last.y)>.002)
      annotationDrawPoints.value=[...annotationDrawPoints.value,point]
  }
  annotationRegion.value = {x:Math.min(annotationStart.x,point.x),y:Math.min(annotationStart.y,point.y),
    width:Math.abs(point.x-annotationStart.x),height:Math.abs(point.y-annotationStart.y)}
  event.preventDefault()
}
function finishAnnotationRegion(event: PointerEvent) {
  if (annotationPointerId !== event.pointerId) return
  annotationDrawing.value = false
  annotationPointerId = null
  annotationStart = null
  if (event.type==='pointercancel') {
    annotationRegion.value=null
    annotationDrawPoints.value=[]
    annotationDraft.body=''
    return
  }
  if (annotationDraft.kind==='draw' && annotationDrawPoints.value.length>=2) {
    const points=annotationDrawPoints.value
    const x=Math.max(0,Math.min(.99,Math.min(...points.map(point=>point.x))-.002))
    const y=Math.max(0,Math.min(.99,Math.min(...points.map(point=>point.y))-.002))
    const width=Math.min(1-x,Math.max(.01,Math.max(...points.map(point=>point.x))-x+.002))
    const height=Math.min(1-y,Math.max(.01,Math.max(...points.map(point=>point.y))-y+.002))
    annotationRegion.value={x,y,width,height}
    annotationDraft.body=JSON.stringify({v:1,points:points.map(point=>[
      Math.round(Math.max(0,Math.min(1,(point.x-x)/width))*10000)/10000,
      Math.round(Math.max(0,Math.min(1,(point.y-y)/height))*10000)/10000])})
  }
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
function beginRedactionMove(event:PointerEvent) {
  if (!redactionRegion.value || redactionBusy.value || props.disabled || event.button!==0) return
  const point=normalizedPointer(event)
  if (!point) return
  redactionMove={pointerId:event.pointerId,start:point,region:{...redactionRegion.value}}
  ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
  event.stopPropagation()
}
function updateRedactionMove(event:PointerEvent) {
  const drag=redactionMove
  if (!drag || drag.pointerId!==event.pointerId) return
  const point=normalizedPointer(event)
  if (!point) return
  redactionRegion.value={...drag.region,
    x:Math.max(0,Math.min(1-drag.region.width,drag.region.x+point.x-drag.start.x)),
    y:Math.max(0,Math.min(1-drag.region.height,drag.region.y+point.y-drag.start.y))}
  event.preventDefault()
}
function finishRedactionMove(event:PointerEvent) {
  if (redactionMove?.pointerId===event.pointerId) redactionMove=null
}
function beginRedactionResize(event: PointerEvent, corner: 'nw' | 'ne' | 'sw' | 'se') {
  const region = redactionRegion.value
  if (!region || props.disabled || editing.value || redactionBusy.value || event.button !== 0) return
  const anchor = {x:corner.includes('w') ? region.x + region.width : region.x,
    y:corner.includes('n') ? region.y + region.height : region.y}
  redactionResize = {pointerId:event.pointerId,anchor}
  ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
  event.stopPropagation()
  event.preventDefault()
}
function updateRedactionResize(event: PointerEvent) {
  if (redactionResize?.pointerId !== event.pointerId) return
  const point = normalizedPointer(event)
  if (!point) return
  const anchor = redactionResize.anchor
  redactionRegion.value = {x:Math.min(anchor.x,point.x),y:Math.min(anchor.y,point.y),
    width:Math.abs(anchor.x-point.x),height:Math.abs(anchor.y-point.y)}
  redactionError.value = ''
  event.stopPropagation()
  event.preventDefault()
}
function finishRedactionResize(event: PointerEvent) {
  if (redactionResize?.pointerId !== event.pointerId) return
  redactionResize = null
  if (!redactionRegion.value || redactionRegion.value.width < .01 || redactionRegion.value.height < .01) {
    redactionRegion.value = null
    redactionError.value = 'Drag a visible rectangular area on the page.'
  }
  event.stopPropagation()
}
async function applyRedaction() {
  const doc = props.document
  const region = redactionRegion.value
  if (!doc || !region || redactionBusy.value || props.disabled) return
  if (!Number.isFinite(region.x) || !Number.isFinite(region.y) ||
      !Number.isFinite(region.width) || !Number.isFinite(region.height) ||
      region.x < 0 || region.y < 0 || region.width <= 0 || region.height <= 0 ||
      region.x + region.width > 1 || region.y + region.height > 1 ||
      !Number.isInteger(redactionRevision) || redactionRevision < 1) {
    redactionError.value = 'The marked area is no longer valid. Reload the page and draw it again.'
    return
  }
  redactionBusy.value = true
  redactionError.value = ''
  try {
    pageAnnotations.value = await api<PageAnnotation[]>(`/documents/${doc.id}/annotations`, 'POST', {
      kind:'redaction',page_index:current.value-1,expected_revision:redactionRevision,
      rect_x: region.x, rect_y: region.y, rect_width: region.width, rect_height: region.height,
      body:'',color:'yellow',output_scope:'pdf',
    })
    redactionRegion.value = null
    redactionNotice.value = 'Redaction mark saved. Drag it to move or resize it; Save copy and new bundle exports remove the covered PDF content. The editor’s underlying working revision still contains it.'
    emit('modified')
  } catch (err) {
    if (err instanceof ApiError && err.status === 409) {
      redactionError.value = 'The working PDF changed. This draft was retained; cancel navigation and review before redrawing.'
    } else redactionError.value = friendlyError(err)
  }
  finally { redactionBusy.value = false }
}
async function removeSelectedRedaction() {
  const doc = props.document
  const selected = selectedRedaction.value
  if (!doc || !selected || redactionBusy.value || props.disabled) return
  redactionBusy.value = true
  editing.value = true
  redactionError.value = ''
  try {
    const signature = await currentTextEditSignature(doc.id, doc.matter_id, doc.revision,
      'Restoring content in a previously redacted area',
      'The working PDF changed. Reload the document and select the redaction again.')
    if (!signature.proceed) return
    if (props.document?.id !== doc.id || props.document.revision !== doc.revision) {
      throw new Error('The working PDF changed. Select the redaction again.')
    }
    const result = await submitReviewedTextEdit<MatterDocument>(
      `/documents/${doc.id}/pages/redactions/${selected.id}/remove`,
      {expected_revision:doc.revision}, signature.acknowledged, doc.title,
      'Restoring a redacted area', 120_000)
    if (!result) return
    selectedRedactionId.value = null
    await loadPdf(result)
    appliedRedactions.value = await api<AppliedRedaction[]>(`/documents/${doc.id}/pages/redactions`)
    redactionNotice.value = 'Selected area restored as page pixels in a new working revision. Inspect it and rebuild exports.'
    emit('modified')
  } catch (err) {
    redactionError.value = friendlyError(err)
  } finally { editing.value = false; redactionBusy.value = false }
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
  annotationDrawPoints.value = []
  annotationDrawing.value = false
  annotationPointerId = null
  annotationStart = null
  annotationDraft.body = ''
  annotationDraft.color = 'yellow'
  if (clearKind) annotationDraft.kind = 'highlight'
  else if (annotationDraft.kind==='date') {
    const today=new Date()
    annotationDraft.body=`${today.getFullYear()}-${String(today.getMonth()+1).padStart(2,'0')}-${String(today.getDate()).padStart(2,'0')}`
  }
  annotationOutputScope.value = annotationDraft.kind==='highlight'||annotationDraft.kind==='comment'?'private':'pdf'
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
  annotationOutputScope.value = item.output_scope
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
  if (annotationDraft.kind === 'date' && !/^\d{4}-\d{2}-\d{2}$/.test(annotationDraft.body)) {
    annotationError.value = 'Choose a valid date.'; return
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
        output_scope:annotationOutputScope.value,
      })
      annotationNotice.value = 'Annotation updated. Existing exports remain unchanged.'
    } else if (annotationDraft.kind === 'image' && annotationImage.value) {
      pageAnnotations.value = await uploadPageAnnotationImage(doc.id, annotationImage.value, {
        page_index:current.value-1,expected_revision:doc.revision,rect_x:region.x,
        rect_y:region.y,rect_width:region.width,rect_height:region.height,
      })
      annotationNotice.value = 'Image mark saved. Move or resize it before Save copy or building a bundle.'
    } else {
      pageAnnotations.value = await api<PageAnnotation[]>(`/documents/${doc.id}/annotations`, 'POST', {
        kind:annotationDraft.kind,page_index:current.value-1,expected_revision:doc.revision,
        rect_x:region.x,rect_y:region.y,rect_width:region.width,rect_height:region.height,
        body:annotationDraft.body,color:annotationDraft.color,
        output_scope:annotationOutputScope.value,
      })
      annotationNotice.value = annotationOutputScope.value==='private'
        ? `${annotationKindLabel(annotationDraft.kind)} saved as a private review annotation.`
        : `${annotationKindLabel(annotationDraft.kind)} saved as an editable PDF mark. Save copy or rebuild exports to include it.`
    }
    resetAnnotationDraft(false)
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
function clickOverlay(kind:SelectedOverlay['kind'],item:PageAnnotation|ReviewedLink) {
  if (suppressOverlayClick) {suppressOverlayClick=false;return}
  selectedOverlay.value={kind,id:item.id}
  if (kind==='annotation' && activeTool.value==='notes') editPageAnnotation(item as PageAnnotation)
}
function clickPageSurface(event:MouseEvent) {
  clearOverlaySelection()
  choosePlacement(event)
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
  const draft = JSON.stringify(markSettings)
  try {
    const saved = await api<OutputMarks>(`/matters/${doc.matter_id}/output-marks`, 'PUT', JSON.parse(draft))
    if (props.document?.matter_id !== doc.matter_id) return
    if (JSON.stringify(markSettings) === draft) Object.assign(markSettings, saved)
    savedMarks.value = JSON.stringify(saved)
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
      ...(params as Record<string, unknown>), expected_revision:props.document.revision, acknowledge_signature_risk: signature.acknowledged,
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
  reorderThumbnailPage(source,target)
}
function reorderThumbnailPage(source:number,target:number) {
  if (editing.value || loading.value || props.disabled || !Number.isInteger(source) || source < 1 || source > total.value ||
      !Number.isInteger(target) || target < 1 || target > total.value || source === target) return
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
  }
}
type TextEditReviewResponse = {review_required: true, detail: string}
type TextPlacementRejected = {placement_rejected: true, detail: string}
function confirmUnexpectedTextRisk(title: string, action = 'Editing'): boolean {
  return window.confirm(`${action} “${title}” may invalidate signature protections. Certificate validity is unverified. The imported original remains unchanged. Continue?`)
}
async function submitReviewedTextEdit<T extends object>(endpoint: string,
    payload: Record<string, unknown>, acknowledged: boolean, title: string,
    action = 'Editing', timeoutMs?: number): Promise<T | null> {
  const submit = (approved: boolean) => api<T | TextEditReviewResponse | TextPlacementRejected>(endpoint, 'POST',
    {...payload, acknowledge_signature_risk: approved}, timeoutMs,
    {'X-CourtBundle-Review-Protocol': 'challenge'})
  let result: T | TextEditReviewResponse | TextPlacementRejected
  try {
    result = await submit(acknowledged)
  } catch (err) {
    if (!(err instanceof ApiError) || err.status !== 409 ||
        !err.message.startsWith('Signature-risk acknowledgment required') || acknowledged) throw err
    // A previously installed local engine may still use the 409 review contract.
    if (!confirmUnexpectedTextRisk(title, action)) return null
    result = await submit(true)
  }
  if ('review_required' in result) {
    if (!confirmUnexpectedTextRisk(title, action)) return null
    result = await submit(true)
    if ('review_required' in result) throw new Error('Signature review was not accepted. The working copy was not changed.')
  }
  if ('placement_rejected' in result) throw new Error(result.detail)
  return result
}
async function currentTextEditSignature(documentId: string, matterId: string,
    expectedRevision: number, action: string,
    staleMessage = 'Document changed. Reload the page and select text again.') {
  const documents = await api<MatterDocument[]>(`/matters/${matterId}/documents`)
  const currentDocument = documents.find(item => item.id === documentId)
  if (!currentDocument) throw new Error(staleMessage)
  if (currentDocument.revision !== expectedRevision)
    throw new Error(staleMessage)
  return confirmSignatureTransformation([currentDocument], action)
}
function conciseTextEditError(err: unknown): string {
  const message = friendlyError(err)
  if (message.startsWith('Signature-risk acknowledgment required'))
    return 'Signature review is required. Select the text and try again.'
  if (message.includes('cannot fit in clear space'))
    return 'No clear room for this replacement. Try Edit whole paragraph or shorten the text.'
  if (message.includes('Replacement paragraph would overlap another text object'))
    return 'The new paragraph would cover nearby text. Shorten it or move the text to a clear area first.'
  if (message.includes('Following text cannot fit on this page after reflow'))
    return 'The paragraph needs more room than this page has below it. Shorten the text or use a larger page.'
  if (message.includes('Replacement would cover text outside this paragraph flow') ||
      message.includes('Following text cannot be moved without covering other text'))
    return 'This page has text outside the paragraph flow that cannot be moved safely. Review its layout before editing.'
  if (message.startsWith('Document changed'))
    return 'Document changed. Reload the page and select text again.'
  return message.length > 140 ? `${message.slice(0, 137)}…` : message
}
async function placeText() {
  if (!props.document || !placement.value || editing.value || props.disabled) return
  if (overlayValidation.value) { emit('error', overlayValidation.value); return }
  if (!canvas.value) return
  editing.value = true
  try {
    const doc=props.document
    const canvasBox=canvas.value.getBoundingClientRect()
    const lines=overlayText.value.split('\n')
    const width=Math.min(1-placement.value.x,(Math.max(...lines.map(line=>line.length))*overlaySize.value*.65*pageZoom.value+14)/canvasBox.width)
    const height=Math.min(1-placement.value.y,(lines.length*overlaySize.value*1.6*pageZoom.value+14)/canvasBox.height)
    if (width<.01 || height<.01) throw new Error('Text does not fit here. Choose a position farther from the page edge.')
    pageAnnotations.value=await api<PageAnnotation[]>(`/documents/${doc.id}/annotations`,'POST',{
      kind:'text',page_index:placement.value.page-1,expected_revision:doc.revision,
      rect_x:placement.value.x,rect_y:placement.value.y,rect_width:width,rect_height:height,
      body:overlayText.value,color:'blue',output_scope:'pdf',
    })
    placement.value = null
    overlayText.value = ''
    emit('modified')
  } catch (err) { emit('error', conciseTextEditError(err)) }
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
  dragStartX = event.clientX
  dragStartY = event.clientY
  ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
}
function updateMove(event: PointerEvent) {
  if (!pointerRun || pointerId !== event.pointerId || !canvas.value) return
  if (!movingRun.value && Math.hypot(event.clientX - dragStartX, event.clientY - dragStartY) < 5) return
  if (!movingRun.value) {
    movingRun.value = pointerRun
    selectedRun.value = null
    runsError.value = ''
    editNotice.value = ''
  }
  const box = pointerRun.relative_bbox
  const offsetX = (event.clientX - dragStartX) / canvas.value.getBoundingClientRect().width
  const offset = (event.clientY - dragStartY) / canvas.value.getBoundingClientRect().height
  horizontalOffset.value = Math.max(-box[0], Math.min(1 - box[2], offsetX))
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
  if (event.type === 'pointerup' && Math.hypot(horizontalOffset.value, verticalOffset.value) >= .0001)
    pendingMove.value = true
  else cancelMove()
}
function cancelMove() {
  movingRun.value = null
  horizontalOffset.value = 0
  verticalOffset.value = 0
  pendingMove.value = false
}
function confirmMove() {
  if (movingRun.value && pendingMove.value)
    void applyMove(movingRun.value, horizontalOffset.value, verticalOffset.value)
}
function clickRun(run: TextRun) {
  if (suppressNextClick) { suppressNextClick = false; return }
  if (pendingMove.value) return
  void selectRun(run)
}
async function applyMove(run: TextRun, offsetX: number, offsetY: number) {
  if (!props.document || editing.value || props.disabled) return
  const documentId = props.document.id
  const matterId = props.document.matter_id
  const title = props.document.title
  const expectedRevision = runsRevision.value
  try { editSnapshot.value = canvas.value?.toDataURL('image/png') || '' } catch { editSnapshot.value = '' }
  editSnapshotFading.value = false
  replacingText.value = true
  editing.value = true
  runsError.value = ''
  try {
    const signature = await currentTextEditSignature(documentId, matterId,
      expectedRevision, 'Moving visible text')
    if (!signature.proceed) return
    const updated = await submitReviewedTextEdit<MatterDocument>(`/documents/${documentId}/pages/move-text-run`, {
      page_index: current.value - 1, span_index: run.index,
      expected_revision: expectedRevision, expected_text: run.text,
      horizontal_offset: offsetX, vertical_offset: offsetY,
    }, signature.acknowledged, title)
    if (!updated) return
    await loadPdf(updated)
    if (!viewerError.value && editSnapshot.value) {
      editSnapshotFading.value = true
      if (!window.matchMedia('(prefers-reduced-motion: reduce)').matches)
        await new Promise(resolve => window.setTimeout(resolve, 200))
    }
    editNotice.value = 'Text moved by removing and redrawing the selected glyphs in the working copy. Review font and position; Undo can restore the prior revision.'
    emit('modified')
  } catch (err) { emit('error', conciseTextEditError(err)) }
  finally { editing.value = false; replacingText.value = false; editSnapshot.value = '' }
}
async function applyReplacement() {
  if (!props.document || !selectedRun.value || !replacement.value.trim() || props.disabled || editing.value) return
  if (replacementEngine.value === 'pdfium-trial' && !selectedParagraph.value) {
    emit('error', 'Wait for the whole paragraph to load before applying the edit.')
    return
  }
  const documentId = props.document.id
  const matterId = props.document.matter_id
  const title = props.document.title
  const expectedRevision = runsRevision.value
  const run = selectedRun.value
  editing.value = true
  replacingText.value = true
  runsError.value = ''
  try {
    const signature = await currentTextEditSignature(documentId, matterId,
      expectedRevision, 'Replacing visible text')
    if (!signature.proceed) return
    const trial = replacementEngine.value === 'pdfium-trial'
    const result = await submitReviewedTextEdit<MatterDocument & {edit_layout:{font_size?:number,line_count?:number,font_size_reduced?:boolean,edit_method:string,font_substituted?:boolean,font_name?:string,following_text_objects_moved?:number}}>(`/documents/${documentId}/pages/${trial ? 'paragraph-trial' : 'replace-text-run'}`, {
      page_index: current.value - 1, span_index: run.index,
      expected_revision: expectedRevision, expected_text: run.text,
      ...(trial ? {expected_paragraph:selectedParagraph.value} : {}),
      replacement: replacement.value,
    }, signature.acknowledged, title)
    if (!result) return
    editNotice.value = trial
      ? `Whole paragraph updated in ${result.edit_layout.line_count} line(s)${result.edit_layout.following_text_objects_moved ? `; ${result.edit_layout.following_text_objects_moved} following text line(s) moved down to make room` : ''}${result.edit_layout.font_substituted ? ` with substituted ${result.edit_layout.font_name}` : ''}. Inspect the page, extraction, and export.`
      : `Removed and redrew selected text in ${result.edit_layout.line_count} line(s) at ${result.edit_layout.font_size} pt${result.edit_layout.font_size_reduced ? ' (reduced to fit)' : ''}. Review font, layout and extraction.`
    selectedRun.value = null
    replacement.value = ''
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
  } catch (err) { emit('error', conciseTextEditError(err)) }
  finally { editing.value = false; replacingText.value = false; editSnapshot.value = '' }
}
</script>

<template>
  <div ref="viewerElement" class="viewer-root read-workspace" :class="{'focused-reading':focusedReading, 'preparation-preview':preparation}" tabindex="0" @keydown="handleViewerKeydown">
    <template v-if="document">
      <div class="viewer-toolbar document-header">
        <button v-if="!preparation" class="btn secondary small document-browser-trigger" aria-label="Browse documents" @click="emit('browseDocuments')"><ArrowLeft :size="16"/> Documents</button>
        <button v-if="preparation" class="icon-btn small preparation-list-toggle" aria-label="Toggle documents panel" title="Toggle documents panel" @click="emit('showDocumentList')"><PanelLeft :size="16"/></button>
        <div class="toolbar-group reader-document"><div><strong :title="document.title">{{document.title}}</strong><div class="document-version-line"><select v-if="artifactOptions" class="artifact-select" aria-label="Document version" value="working" :disabled="editing || loading" @change="emit('artifact',($event.target as HTMLSelectElement).value); ($event.target as HTMLSelectElement).value='working'"><option v-for="option in artifactOptions" :key="option.value" :value="option.value">{{preparation && option.value==='working' ? 'Working Copy' : option.label}}</option></select><span v-else>Working Copy · Editable</span><span class="document-revision">Revision {{document.revision}} · {{preparation ? 'Original preserved' : `${document.page_count} source pages`}}</span></div></div></div>
        <template v-if="preparation"><button class="btn secondary small open-editor" @click="emit('openEditor')">Open editor</button><button class="icon-btn small" aria-label="Document details" :aria-expanded="!!detailsOpen" aria-controls="document-details-panel" title="Document details" @click="emit('toggleDetails')"><PanelRightOpen :size="16"/></button></template>
        <span v-if="!preparation" class="document-save-state" role="status"><CheckCircle2 v-if="!hasPendingDraft() && !detailsDirty && !editing && !headerSaving" :size="14"/>{{editing || headerSaving ? 'Saving…' : hasPendingDraft() || detailsDirty ? 'Unsaved changes' : 'Saved locally'}}</span>
        <button v-if="!preparation" class="btn primary small document-save" :disabled="busy || headerSaving || engineBlocked || editing || loading || (!detailsDirty && (!hasPendingDraft() || disabled))" @click="detailsDirty ? emit('saveDetails') : saveHeaderDraft()"><Save :size="15"/> Save</button>
      </div>
      <div v-if="workingSaveNotice || workingSaveError" class="preview-status" :class="{'error-msg':!!workingSaveError}" :role="workingSaveError ? 'alert' : 'status'">{{workingSaveError || workingSaveNotice}}<button class="icon-btn small" aria-label="Dismiss save notice" @click="workingSaveNotice='';workingSaveError=''"><X :size="14"/></button></div>
      <div v-show="!preparation" class="reader-tool-navigation">
        <div class="pdf-tool-tabs" role="tablist" aria-label="PDF tools">
          <button v-for="category in (['read','edit','review','pages','ocr','redact'] as const)" :key="category" role="tab" :aria-selected="activeCategory===category" :class="{active:activeCategory===category}" :data-category="category" :id="`pdf-tab-${category}`" :tabindex="activeCategory===category?0:-1" aria-controls="pdf-tool-panel" @keydown="tabKey($event,category)" @click="selectCategory(category)"><component :is="category==='read'?BookOpen:category==='edit'?Pencil:category==='review'?Highlighter:category==='pages'?LayoutGrid:category==='ocr'?ScanText:Shield" :size="15"/>{{category==='read'?'Read':category==='edit'?'Edit':category==='review'?'Annotate':category==='pages'?'Pages':category==='ocr'?'OCR':'Redact'}}</button>
        </div>
        <div class="reader-mode-actions">
          <button v-if="expanded" class="icon-btn small" aria-label="Undo last matter change" :title="undoDescription" :aria-description="undoDescription" :disabled="!canUndo || disabled || editing" @click="emit('undo')"><Undo2 :size="16"/></button>
          <button v-if="expanded" class="icon-btn small" aria-label="Redo last matter change" :title="redoDescription" :aria-description="redoDescription" :disabled="!canRedo || disabled || editing" @click="emit('redo')"><Redo2 :size="16"/></button>
          <button class="icon-btn small" :aria-label="focusedReading ? 'Exit focused reading' : 'Focused reading'" :title="focusedReading ? 'Exit focused reading' : 'Focused reading'" @click="toggleFocusedReading"><Maximize2 :size="16"/></button>
          <button class="btn secondary small index-trigger" aria-label="Open bundle index" @click="emit('openBookmarks')">Index &amp; Bookmarks</button>
          <details ref="actionsMenu" class="reader-more" @keydown.esc.stop="actionsMenu?.removeAttribute('open'); actionsMenu?.querySelector('summary')?.focus()"><summary role="button" class="icon-btn small" aria-label="More document actions" title="More document actions"><Ellipsis :size="17"/></summary><div class="reader-more-menu" @click.capture="closeActionMenu" role="tablist" aria-label="Additional PDF tools">
            <button class="text-tool" role="tab" :aria-selected="activeCategory==='stamps'" data-category="stamps" id="pdf-tab-stamps" @click="selectCategory('stamps')"><Stamp :size="15"/> Stamps</button>
            <button class="text-tool" role="tab" :aria-selected="activeCategory==='output'" data-category="output" id="pdf-tab-output" @click="selectCategory('output')"><FilePenLine :size="15"/> Output</button>
            <button class="text-tool" aria-label="Save working copy" :disabled="workingSaveBusy || disabled || editing || loading" @click="saveWorkingCopy"><Save :size="15"/> {{workingSaveBusy ? 'Saving…' : 'Save copy'}}</button>
            <button class="text-tool" aria-label="Print working copy" :disabled="disabled || editing || loading" @click="printWorkingCopy"><Printer :size="15"/> Print</button>
            <button class="text-tool" :disabled="editing || loading || disabled" aria-label="Archive document" @click="emit('archiveDocument')"><Archive :size="15"/> Archive document</button>
            <button class="text-tool" aria-label="Document details" :aria-expanded="!!detailsOpen" aria-controls="document-details-panel" @click="emit('toggleDetails')"><PanelRightOpen :size="15"/> Details</button>
            <button class="text-tool" :aria-label="expanded ? 'Exit full screen' : 'Full screen'" @click="toggleExpandedView"><Maximize2 :size="15"/> {{expanded ? 'Exit expanded editor' : 'Expand editor'}}</button>
          </div></details>
        </div>
      </div>
      <div v-show="!focusedReading && !preparation && activeCategory!=='read'" id="pdf-tool-panel" class="page-editbar" role="tabpanel" :aria-labelledby="`pdf-tab-${activeCategory}`">
        <div v-if="activeCategory==='ocr'" class="document-actionbar" role="group" aria-label="Document tools"><label for="ocr-language">OCR language</label><select id="ocr-language" v-model="ocrLanguage" :disabled="editing || loading || disabled"><option value="eng">English</option><option value="hin">Hindi</option><option value="eng+hin">English + Hindi</option></select><button class="text-tool" :disabled="editing || loading || disabled" @click="emit('ocr', ocrLanguage)"><ScanText :size="15"/> Run {{ocrLanguage==='eng'?'English':ocrLanguage==='hin'?'Hindi':'English + Hindi'}} OCR</button><span>Review recognized text before use.</span></div>
        <div v-if="activeCategory==='edit'" class="editor-tool-group" role="group" aria-label="Edit PDF">
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="activeTool==='select'" @click="toggleTool('select')">Select</button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="textMode" @click="toggleTool('add-text')"><Type :size="15"/> Add text</button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="editTextMode" @click="toggleTool('edit-text')"><Pencil :size="15"/> Edit text</button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="redactionOpen" @click="toggleTool('redact')"><Eraser :size="15"/> Redact</button>
        </div><div v-if="activeCategory==='review'" class="editor-tool-group" role="group" aria-label="Review PDF">
        <button class="text-tool" :disabled="editing || loading || disabled" title="Private review highlight" :aria-pressed="annotationPanelOpen && annotationDraft.kind==='highlight'" @click="openAnnotationTool('highlight')"><Highlighter :size="15"/> Highlight<span v-if="currentPageAnnotations.some(item=>item.kind==='highlight')" class="page-bookmark-count">{{currentPageAnnotations.filter(item=>item.kind==='highlight').length}}</span></button>
        <button class="text-tool" disabled title="Underline is planned for Phase 03B"><Minus :size="15"/> Underline</button>
        <button class="text-tool" disabled title="Strikeout is planned for Phase 03B"><Minus :size="15"/> Strikeout</button>
        <button class="text-tool" :disabled="editing || loading || disabled" title="Private review comment" :aria-pressed="annotationPanelOpen && annotationDraft.kind==='comment'" @click="openAnnotationTool('comment')"><MessageSquare :size="15"/> Comment</button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="annotationPanelOpen && annotationDraft.kind==='draw'" @click="openAnnotationTool('draw')"><Pencil :size="15"/> Draw</button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="annotationPanelOpen && annotationDraft.kind==='box'" @click="openAnnotationTool('box')"><Square :size="15"/> Box</button>
        <details class="annotation-more"><summary role="button" class="text-tool" aria-label="More annotation tools"><Ellipsis :size="15"/> More</summary><div class="annotation-more-menu">
          <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="activeTool==='select'" @click="toggleTool('select')">Select</button>
          <button class="text-tool" :disabled="editing || loading || disabled" :aria-expanded="bookmarkOpen" @click="bookmarkOpen=!bookmarkOpen;bookmarkTitle=document?.title+' — page '+current;bookmarkError=''"><BookMarked :size="15"/> Bookmark page</button>
          <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="annotationPanelOpen && annotationDraft.kind==='ellipse'" @click="openAnnotationTool('ellipse')"><Circle :size="15"/> Ellipse</button>
          <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="annotationPanelOpen && annotationDraft.kind==='line'" @click="openAnnotationTool('line')"><Minus :size="15"/> Line</button>
          <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="annotationPanelOpen && annotationDraft.kind==='arrow'" @click="openAnnotationTool('arrow')"><ArrowUpRight :size="15"/> Arrow</button>
          <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="linkPanelOpen" @click="toggleTool('links')"><Link2 :size="15"/> Links</button>
        </div></details>
        </div><div v-if="activeCategory==='stamps'" class="editor-tool-group" role="group" aria-label="Visual stamps">
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="activeTool==='select'" @click="toggleTool('select')">Select</button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="annotationPanelOpen && annotationDraft.kind==='image'" @click="openAnnotationTool('image')"><Images :size="15"/> Image / stamp</button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="annotationPanelOpen && annotationDraft.kind==='check'" @click="openAnnotationTool('check')"><Check :size="15"/> Check</button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="annotationPanelOpen && annotationDraft.kind==='cross'" @click="openAnnotationTool('cross')"><X :size="15"/> Cross</button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="annotationPanelOpen && annotationDraft.kind==='date'" @click="openAnnotationTool('date')"><CalendarDays :size="15"/> Date</button>
        <span class="stamp-scope-hint">Visual marks only · no verified signature or form filling</span>
        </div><div v-if="activeCategory==='output'" class="editor-tool-group" role="group" aria-label="Output PDF">
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="marksPanelOpen" @click="toggleTool('marks')"><Stamp :size="15"/> Output marks<span v-if="marksDirty" class="details-unsaved-indicator">Unsaved</span></button>
        </div><div v-if="activeCategory==='pages'" class="editor-tool-group" role="group" aria-label="Change pages">
        <button class="text-tool" :disabled="editing || loading || disabled" title="Copy pages from a document in this matter into this PDF" @click="emit('pageTools')"><FilePlus2 :size="15"/> Add pages</button>
        <button class="text-tool" :disabled="editing || loading || disabled" title="Select, reorder, rotate or delete several pages" @click="organizerOpen=true"><LayoutGrid :size="15"/> Organize pages</button>
        <button class="text-tool" :disabled="editing || loading || disabled" title="Replace, crop, extract, split or compress pages" @click="emit('pageTools')"><FilePenLine :size="15"/> Page tools</button>
        <button class="text-tool" :disabled="editing || loading || disabled" title="Rotate this page 90° counterclockwise in the working PDF" @click="edit('rotate', {page_index:current-1,degrees:270})"><RotateCcw :size="16"/> Rotate left</button>
        <button class="text-tool" :disabled="editing || loading || disabled" title="Rotate this page 90° clockwise in the working PDF" @click="edit('rotate', {page_index:current-1,degrees:90})"><RotateCw :size="16"/> Rotate right</button>
        <button class="text-tool page-icon-tool" :disabled="editing || loading || disabled || current<=1" aria-label="Move page left" title="Move page left" @click="move(-1)"><ChevronLeft :size="16"/></button>
        <button class="text-tool page-icon-tool" :disabled="editing || loading || disabled || current>=total" aria-label="Move page right" title="Move page right" @click="move(1)"><ChevronRight :size="16"/></button>
        <button class="text-tool danger" :disabled="editing || loading || disabled || total<=1" :title="total<=1 ? 'A PDF must keep at least one page' : `Delete page ${current} from the editable copy`" @click="deleteCurrent"><Trash2 :size="16"/> Delete page {{current}}</button>
        </div>
      </div>
      <div v-if="selectedOverlayItem" class="pdf-selected-overlay-actions" role="status"><span>{{selectedOverlayLabel}} selected · drag to move · drag corner to resize · Delete to remove</span><button class="btn secondary small" :disabled="annotationBusy || linksBusy || disabled" @click="deleteSelectedOverlay">Remove selected</button><button class="icon-btn small" aria-label="Clear selection" @click="clearOverlaySelection"><X :size="14"/></button></div>
      <p v-if="activeCategory==='pages'" class="page-action-guidance">Changes here affect the editable copy; the imported original stays unchanged. Drag page thumbnails to reorder, or use Organize pages for arrow controls.</p>
      <div v-if="bookmarkOpen" class="page-bookmark-editor" aria-label="Bookmark current page">
        <strong>Bookmark {{document.title}}, page {{current}}</strong>
        <label>Bookmark title<input v-model="bookmarkTitle" maxlength="200" :disabled="bookmarkSaving || disabled" @keydown.enter.prevent="addPageBookmark"/></label>
        <button class="btn primary small" :disabled="bookmarkSaving || disabled || !bookmarkTitle.trim()" @click="addPageBookmark">{{bookmarkSaving ? 'Saving…' : 'Add bookmark'}}</button>
        <p v-if="bookmarkError" class="error-msg" role="alert">{{bookmarkError}}</p>
      </div>
      <p v-if="bookmarkNotice" class="page-bookmark-notice" role="status">{{bookmarkNotice}}</p>
      <Transition name="soft"><div v-if="detailsDirty" class="preview-draft-warning" role="status">Save document details to use PDF tools.<button v-if="!detailsOpen" class="text-tool" @click="emit('toggleDetails')">Open details</button></div></Transition>

      <PdfReaderControls :compact="preparation" editing-tools :page="current" :count="total" v-model:zoom="zoomChoice" :scale="pageZoom" v-model:mode="readerMode" :busy="loading || editing" :can-back="!!readingHistory.length" @update:page="navigateToPage($event)" @back="goBack"><template #actions><button class="icon-btn small" aria-label="Search" title="Search document" :aria-expanded="findOpen" @click="showFind"><Search :size="16"/></button><button class="icon-btn small reader-info-trigger" aria-label="Document information" title="Document information" :aria-expanded="readPanel==='information'" @click="openReadPanel('information',$event)"><Info :size="16"/></button></template></PdfReaderControls>
      <div class="viewer-edit-workspace" :class="{'has-tool-inspector':inspectorVisible,'expanded-pages':expanded,'has-read-panel':!!readPanel && !focusedReading, 'has-document-information':readPanel==='information' && !focusedReading}">
      <div v-show="!focusedReading && !preparation" class="reader-panel-tabs" aria-label="Reader panels">
        <button class="icon-btn small" :class="{active:thumbnailsVisible}" :aria-label="thumbnailsVisible?'Hide thumbnails':'Show thumbnails'" title="Pages" @click="toggleThumbnails"><PanelLeft :size="16"/></button>
        <button class="icon-btn small" :class="{active:findOpen}" aria-label="Search panel" title="Search" @click="showFind"><Search :size="16"/></button>
        <button class="icon-btn small" :class="{active:outlineOpen}" aria-label="Document bookmarks" title="PDF outline and document bookmarks" @click="openReadPanel('outline',$event)"><BookMarked :size="16"/></button>
        <button class="icon-btn small" :class="{active:commentsOpen}" :aria-label="`Comments (${documentComments.length})`" title="Comments" @click="openReadPanel('comments',$event)"><MessageSquare :size="16"/></button>
        <button v-if="pageBookmarks.length" class="icon-btn small" :aria-label="`Page ${current} bookmarks${pageBookmarks.some(item=>!item.resolved)?' need review':''}`" @click="openReadPanel('page-bookmarks',$event)"><BookOpen :size="16"/></button>
      </div>
      <PdfThumbnailRail v-if="!preparation && total > 0 && showThumbnails && (!readPanel || readPanel==='information') && !focusedReading && (expanded || activeTool==='select' || activeCategory==='review')" :pdf="readerPdf" :count="total" :page="current" :title="document.title"
        :disabled="disabled || editing || loading" :dragging-page="draggingPage" :drop-target-page="dropTargetPage" @select="navigateToPage($event)" @reorder="reorderThumbnailPage"
        @dragstart="startPageDrag" @dragover="overPage" @drop="dropPage" @dragend="draggingPage=null;dropTargetPage=null"/>
      <aside v-if="readPanel && !focusedReading" ref="panelElement" id="pdf-read-panel" class="reader-side-panel" :class="{'document-information-panel':readPanel==='information'}" role="region" :aria-label="findOpen ? 'Search document' : readPanel==='information' ? 'Document information' : outlineOpen ? 'Source PDF outline' : commentsOpen ? 'Private review comments' : `Bookmarks on page ${current}`">
        <div class="reader-side-panel-heading"><strong>{{findOpen ? 'Search document' : readPanel==='information' ? 'Document information' : outlineOpen ? 'Document bookmarks' : commentsOpen ? `Comments (${documentComments.length})` : `Page ${current} bookmarks`}}</strong><button v-if="!findOpen" class="icon-btn small" :aria-label="readPanel==='information' ? 'Close document information' : outlineOpen ? 'Close PDF outline' : commentsOpen ? 'Close comments' : 'Close page bookmarks'" @click="closeReadPanel"><X :size="16"/></button></div>
      <div v-if="findOpen" class="pdf-findbar" role="search" aria-label="Find in working PDF">
        <Search :size="16" aria-hidden="true"/>
        <input ref="findInput" v-model="findQuery" :disabled="!readerPdf" type="search" placeholder="Find text in this PDF" aria-label="Find text in this PDF" @keydown.enter.prevent="stepFind($event.shiftKey ? -1 : 1)"/>
        <span role="status" v-if="findBusy">Searching…</span><span v-else-if="findError" class="error-msg" role="alert">{{findError}}</span><span role="status" v-else-if="findQuery.trim().length">{{findMatches.length}}{{findTruncated ? '+' : ''}} match{{findMatches.length===1?'':'es'}}<template v-if="findMatches.length"> · {{findIndex+1}}/{{findMatches.length}}</template></span>
        <button class="icon-btn small" aria-label="Previous search result" :disabled="!findMatches.length" @click="stepFind(-1)"><ChevronLeft :size="16"/></button>
        <button class="icon-btn small" aria-label="Next search result" :disabled="!findMatches.length" @click="stepFind(1)"><ChevronRight :size="16"/></button>
        <button class="icon-btn small" aria-label="Close PDF search" @click="closeReadPanel"><X :size="16"/></button>
      </div>
      <div v-if="findOpen && findMatches.length" class="pdf-find-results" aria-label="PDF search occurrences">
        <button v-for="(match,index) in findMatches" :key="match.index" :class="{active:index===findIndex}" :title="match.snippet" :aria-current="index===findIndex?'true':undefined" @keydown="resultKey($event,index)" @click="chooseFind(index)">Page {{match.page}} · {{match.snippet}}</button>
      </div>
      <p v-if="findOpen && !findQuery.trim()" class="reader-side-panel-intro">Search text in this working PDF. Image-only pages need OCR before their text can be searched.</p>
      <p v-if="findOpen && !readerPdf" class="reader-side-panel-intro" role="status">{{loading ? 'Opening PDF…' : 'Search unavailable until the PDF loads.'}}</p>
        <dl v-if="readPanel==='information'" class="reader-document-information">
          <dt>Document</dt><dd>{{document.title}}</dd>
          <dt>Imported original</dt><dd>{{document.original_name}}<small>Preserved separately; reading and editing use the working copy.</small></dd>
          <dt>Working copy</dt><dd>Revision {{document.revision}} · {{total || document.page_count}} source pages</dd>
          <dt>Document date</dt><dd>{{document.document_date || 'Not recorded'}}<small>{{document.date_status}} · source: {{document.date_source}}</small></dd>
          <dt>OCR processing</dt><dd>{{document.ocr_status}}<small>OCR text is not legally verified. Search checks text available in this PDF.</small></dd>
          <dt>Source SHA-256</dt><dd class="source-hash">{{document.source_sha256 || 'Unavailable'}}</dd>
        </dl>
        <p v-if="outlineOpen" class="reader-side-panel-intro">Navigation embedded in the source PDF. It does not change the bundle index.</p>
        <p v-if="commentsOpen" class="reader-side-panel-intro">Private review notes, omitted from Save copy and bundles.</p>
        <p v-if="pageBookmarksOpen" class="reader-side-panel-intro">Manual bookmarks for this source page. Check their bundle destinations under Index & bookmarks.</p>
        <label v-if="commentsOpen && documentComments.length" class="reader-comment-search"><Search :size="15" aria-hidden="true"/><input v-model="commentQuery" type="search" aria-label="Search private comments" placeholder="Search comments or page"/></label>
        <div v-if="commentsOpen && commentQuery.trim()" class="reader-filter-count">{{filteredComments.length}} of {{documentComments.length}} comments</div>
        <div class="reader-side-panel-list">
          <template v-if="outlineOpen"><p v-if="outlineError" role="status">{{outlineError}}</p><p v-if="outlineLoading || loading" role="status">Loading PDF outline…</p><p v-else-if="!readerPdf" role="status">Outline unavailable until the PDF loads.</p><p v-else-if="!outlineError && !sourceOutline.length">No outline in this source PDF.</p><button v-for="(item,index) in sourceOutline" :key="index" class="text-tool" :disabled="!item.page && !item.url" :style="{paddingLeft:`${10+item.level*12}px`}" @click="item.page?navigateToPage(item.page):item.url&&emit('openExternalLink',item.url)">{{item.title}}<small v-if="item.page"> · p{{item.page}}</small></button>
            <h4 class="reader-section-label">Manual document bookmarks</h4>
            <p v-if="bookmarksLoading" role="status">Loading manual bookmarks…</p><p v-else-if="bookmarksError" role="alert">{{bookmarksError}}</p><p v-else-if="!documentBookmarks.length">No manual bookmarks for this document.</p>
            <button v-for="item in documentBookmarks" :key="item.id" class="text-tool" :disabled="!item.resolved || item.target_revision!==document.revision || item.page_index>=total" @click="navigateToPage(item.page_index+1)">{{item.title}} · Source {{item.page_index+1}}<small v-if="!item.resolved || item.target_revision!==document.revision"> · Needs review</small></button>
          </template>
          <template v-if="commentsOpen"><p v-if="!documentComments.length">No private comments in this document.</p><p v-else-if="!filteredComments.length">No matching comments.</p><button v-for="item in filteredComments" :key="item.id" class="text-tool reader-comment-entry" :class="{selected:item.page_index===current-1}" :title="item.body" @click="openReviewComment(item)"><span class="reader-comment-entry-head"><MessageSquare :size="14"/> Source {{item.page_index+1}} <small>{{item.resolved?'Current':'Needs review'}}</small></span><span>{{item.body}}</span><small>{{annotationTime(item)}}</small></button></template>
          <template v-if="pageBookmarksOpen"><div v-for="item in pageBookmarks" :key="item.id" class="reader-bookmark-row"><strong>{{item.title}}</strong><span v-if="!item.resolved">Needs review after a source revision or page change</span><span v-else>Destination currently linked</span></div></template>
        </div>
        <button v-if="pageBookmarksOpen || outlineOpen" class="text-tool reader-panel-footer" @click="emit('openBookmarks')">Open Index & bookmarks</button>
      </aside>
      <aside v-if="inspectorVisible" class="viewer-tool-inspector" :class="{'marks-inspector':marksPanelOpen}" aria-label="PDF editing controls">
        <div class="viewer-tool-inspector-heading"><strong>{{annotationPanelOpen && editingAnnotationId ? 'Highlight & comment' : toolTitle}}</strong><button class="icon-btn small" aria-label="Close PDF tool" title="Close tool" @click="toggleTool(activeTool)"><X :size="16"/></button></div>
      <Transition name="soft"><div v-if="editTextMode" class="text-overlay-panel" aria-label="Edit existing text">
        <strong>Edit selectable text</strong>
        <p>Click text on the page to change its words. Drag highlighted text anywhere in a clear area of this page to move it. Review the position before applying.</p>
        <p>Scanned text needs OCR and review before editing. Redact creates a movable mark; only a generated Save copy or bundle removes covered content.</p>
        <p v-if="replacementEngine==='pdfium-trial'">The whole detected paragraph is selected. This mode changes all of its lines; if the page is unsupported, the working PDF is left unchanged.</p>
        <div v-if="pendingMove" class="page-tool-actions"><span>Previewing a move on page {{current}}. The destination must be clear.</span><div><button class="btn secondary small" :disabled="editing" @click="cancelMove">Cancel</button><button class="btn primary small" :disabled="editing || disabled" @click="confirmMove">Apply move</button></div></div>
        <p v-if="runsError && !selectedRun" class="error-msg" role="alert">{{runsError}}</p>
        <p class="edit-status-slot" role="status">{{editNotice}}</p>
        <p v-if="!textRuns.length && !editNotice">No selectable horizontal text found on this page. Scanned text needs OCR and review.</p>
      </div></Transition>
      <Transition name="soft"><div v-if="redactionOpen" class="text-overlay-panel" aria-label="Permanent redaction">
        <strong>Permanent redaction · page {{current}}</strong>
        <ol class="pdf-tool-steps"><li :class="{done:!!redactionRegion}">Drag over the content to remove.</li><li :class="{done:!!redactionRegion}">Adjust the black mark, then click Add redaction mark.</li><li>Move or resize the saved mark. Save copy or rebuild the bundle to remove content in the exported PDF.</li></ol>
        <p v-if="redactionRegion" class="notice amber" role="status">This is an editable mark. The underlying working PDF still contains the content until a PDF output is generated.</p>
        <p>Save copy and new bundle exports remove text and image pixels under saved redaction marks. The editor’s working revision, imported originals, prior revisions, existing exports and matter history can still contain the content.</p>
        <div class="page-tool-actions"><span>{{redactionRegion ? `Marked ${Math.round(redactionRegion.width*100)}% × ${Math.round(redactionRegion.height*100)}% of page` : 'No area marked yet'}}</span><div><button v-if="redactionRegion" class="btn secondary small" :disabled="redactionBusy" @click="redactionRegion=null;redactionError=''">Clear mark</button><button class="btn primary small" :disabled="!redactionRegion || redactionRegion.width < .01 || redactionRegion.height < .01 || redactionBusy || disabled" @click="applyRedaction">{{redactionBusy ? 'Saving…' : 'Add redaction mark'}}</button></div></div>
        <div v-if="appliedRedactions.some(item => item.page_index === current-1)" class="redaction-restore">
          <strong>Applied areas on this page</strong>
          <p>The amber outline selects a restorable area; the black fill is in the updated PDF. The outline includes text touched beyond the original mark. Newer edits inside it will be replaced; edits outside remain. Original vector objects and links are not reconstructed.</p>
          <p v-if="selectedRedaction?.structure_changed">Page order, size or rotation changed after this mark. Selective restoration is unavailable for this area.</p>
          <button v-if="selectedRedaction && !selectedRedaction.structure_changed" class="btn secondary small" :disabled="redactionBusy || disabled" @click="removeSelectedRedaction">{{redactionBusy ? 'Restoring…' : 'Remove selected redaction'}}</button>
        </div>
        <p v-if="canUndo" class="redaction-undo"><button class="btn secondary small" :disabled="redactionBusy || editing || disabled" :title="undoDescription" @click="emit('undo')"><Undo2 :size="14"/> Undo last matter change</button><span>If redaction was the last change, Undo restores the prior working copy. Earlier exports still need review.</span></p>
        <p v-if="redactionError" class="error-msg" role="alert">{{redactionError}}</p><p v-if="redactionNotice" class="review-tool-notice" role="status">{{redactionNotice}}</p>
      </div></Transition>
      <Transition name="soft"><div v-if="annotationPanelOpen" class="page-annotation-panel" aria-label="Private page marks and image overlays">
        <p v-if="editingAnnotationId && pageAnnotations.find(item=>item.id===editingAnnotationId)" class="annotation-record-time">Saved {{annotationTime(pageAnnotations.find(item=>item.id===editingAnnotationId)!)}} · Author not recorded</p>
        <div class="review-tool-heading"><strong>{{annotationKindLabel(annotationDraft.kind)}}</strong><span>{{annotationDraft.kind==='image'?'Choose a PNG or JPEG, then drag its size and position on the page.':'Drag a region on the page. Each saved mark remains independently selectable, movable and removable.'}}</span></div>
        <p class="annotation-scope-note">{{annotationOutputScope==='private'?'This private review mark is omitted from Save copy and filing bundles.':'This editable mark is included in Save copy and new bundle exports.'}}</p>
        <div class="annotation-form-grid">
          <label v-if="annotationDraft.kind==='image' && !editingAnnotationId">PNG or JPEG<input ref="annotationFileInput" type="file" accept="image/png,image/jpeg,.png,.jpg,.jpeg" :disabled="annotationBusy || disabled" @change="chooseAnnotationImage"/></label>
          <label v-if="!['image','highlight','comment','redaction'].includes(annotationDraft.kind)">Output<select v-model="annotationOutputScope" :disabled="annotationBusy || disabled"><option value="pdf">Include in saved PDF</option><option value="private">Private review only</option></select></label>
          <label v-if="annotationDraft.kind!=='image'">Colour<select v-model="annotationDraft.color" :disabled="annotationBusy || disabled"><option value="yellow">Yellow</option><option value="green">Green</option><option value="blue">Blue</option><option value="pink">Pink</option></select></label>
          <label v-if="annotationDraft.kind==='comment' || annotationDraft.kind==='highlight'" class="annotation-note">{{annotationDraft.kind==='comment'?'Comment text':'Optional highlight note'}}<textarea v-model="annotationDraft.body" maxlength="4000" rows="2" :disabled="annotationBusy || disabled"/></label>
          <label v-if="annotationDraft.kind==='date'" class="annotation-note">Date shown<input v-model="annotationDraft.body" type="date" :disabled="annotationBusy || disabled"/></label>
          <label v-if="annotationDraft.kind==='text'" class="annotation-note">Text<textarea v-model="annotationDraft.body" maxlength="1000" rows="3" :disabled="annotationBusy || disabled"/></label>
        </div>
        <div class="page-tool-actions"><span>{{annotationRegion ? `${Math.round(annotationRegion.width*100)}% × ${Math.round(annotationRegion.height*100)}% region selected` : 'Drag a region on the page'}}</span><div><button v-if="editingAnnotationId" class="btn secondary small" :disabled="annotationBusy" @click="resetAnnotationDraft()">Cancel edit</button><button class="btn secondary small" :disabled="annotationBusy || disabled || !annotationRegion || (annotationDraft.kind==='comment' && !annotationDraft.body.trim()) || (annotationDraft.kind==='image' && !editingAnnotationId && !annotationImage)" @click="savePageAnnotation">{{annotationBusy?'Saving…':editingAnnotationId?'Update mark':'Add mark'}}</button></div></div>
        <p v-if="annotationError" class="error-msg" role="alert">{{annotationError}}</p><p v-if="annotationNotice" class="review-tool-notice" role="status">{{annotationNotice}}</p>
        <div v-if="currentPageAnnotations.length" class="page-annotation-list"><div v-for="item in currentPageAnnotations" :key="item.id" :class="['page-annotation-row',{unresolved:!item.resolved}]"><component :is="item.kind==='image'?Images:item.kind==='highlight'?Highlighter:item.kind==='comment'?MessageSquare:item.kind==='draw'?Pencil:item.kind==='box'?Square:item.kind==='ellipse'?Circle:item.kind==='line'?Minus:item.kind==='arrow'?ArrowUpRight:item.kind==='check'?Check:item.kind==='date'?CalendarDays:item.kind==='text'?Type:item.kind==='redaction'?Eraser:X" :size="15"/><span><strong>{{annotationKindLabel(item.kind)}}</strong><small>{{item.resolved ? (item.body && item.kind!=='draw'?item.body:item.output_scope==='pdf'?'Included in Save copy and new bundles':'Private review mark') : 'Needs review after a revision change'}}</small></span><button class="icon-btn small" :aria-label="`Edit ${item.kind}`" :disabled="annotationBusy || !item.resolved" @click="editPageAnnotation(item)"><Pencil :size="14"/></button><button class="icon-btn small danger" :aria-label="`Remove ${item.kind}`" :disabled="annotationBusy" @click="removePageAnnotation(item)"><Trash2 :size="14"/></button></div></div>
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

      <div v-if="(loading || editing) && !(replacingText && editSnapshot)" class="preview-status" role="status"><LoaderCircle :size="18" class="spin"/> {{editing?'Updating PDF…':'Loading PDF…'}}</div>
      <div v-if="readingPositionError && !engineBlocked" class="preview-status error-msg" role="status">{{readingPositionError}}</div>
      <div v-if="viewerError" class="preview-status error-msg" role="alert">Page {{current}} unavailable. {{engineBlocked ? 'Its data may not be loaded. Reconnect, then retry this PDF.' : viewerError}} <button class="btn secondary small" :disabled="engineBlocked" @click="emit('retryReader')">Retry PDF</button></div>
      <PdfReaderViewport :horizontal-gutter="32" ref="readerViewport" :pdf="readerPdf" v-model:page="current" :zoom="zoomChoice" :mode="activeTool==='select'?readerMode:'single'" :interactive="activeTool==='select' && (!disabled || engineBlocked)" :matches="findOpen?findMatches:[]" :active-match-index="findOpen?findIndex:-1"
        :surface-class="{'selecting-text':activeTool==='select' && (!disabled || engineBlocked),'placing-text':textMode && !disabled,'drawing-link':linkPanelOpen && !disabled,'drawing-annotation':annotationPanelOpen && !disabled,'drawing-redaction':redactionOpen && !disabled,'is-busy':(loading || editing) && !replacingText,'is-edit-refreshing':replacingText,'is-stale':!pageReady,'is-rotating':rotatingPage}"
        @position="rememberPosition" @ready="pageRendered" @error="viewerError=$event" @internal-link="navigateToPage($event)" @external-link="emit('openExternalLink',$event)"
        @surface-click="clickPageSurface" @surface-pointerdown="beginLinkRegion($event);beginAnnotationRegion($event);beginRedactionRegion($event)" @surface-pointermove="updateLinkRegion($event);updateAnnotationRegion($event);updateRedactionRegion($event)" @surface-pointerup="finishLinkRegion($event);finishAnnotationRegion($event);finishRedactionRegion($event)" @surface-pointercancel="finishLinkRegion($event);finishAnnotationRegion($event);finishRedactionRegion($event)">
          <span v-if="textMode && placement && placement.page===current && overlayText" class="pdf-add-text-preview" :style="{left:`${placement.x*100}%`,top:`${placement.y*100}%`,fontSize:`${overlaySize*pageZoom}px`}">{{overlayText}}</span>
          <div v-if="textMode && placement && placement.page===current" class="pdf-inline-add-card" :style="inlineAddStyle" @click.stop @pointerdown.stop>
            <label>Add text on page {{current}}<textarea ref="inlineAddInput" v-model="overlayText" maxlength="1000" rows="3" :disabled="editing || disabled" placeholder="Type text here" @keydown.esc.stop.prevent="placement=null" @keydown.meta.enter.prevent="placeText" @keydown.ctrl.enter.prevent="placeText"></textarea></label>
            <label>Size <input v-model.number="overlaySize" type="number" min="6" max="36" :disabled="editing || disabled"/> pt</label>
            <small class="pdf-add-text-guidance">Printable ASCII · 6–36 pt · Ctrl/⌘+Enter to apply</small>
            <div class="pdf-inline-add-actions"><button class="btn secondary small" :disabled="editing" @click="placement=null">Cancel</button><button class="btn primary small" :disabled="!!overlayValidation || editing || disabled" :title="overlayValidation || undefined" @click="placeText">Apply text</button></div>
          </div>
          <span v-if="placement && placement.page===current && !overlayText" class="pdf-placement-marker" :style="{left:`${placement.x*100}%`,top:`${placement.y*100}%`}" aria-hidden="true"></span>
          <img v-if="editSnapshot" class="pdf-edit-snapshot" :class="{'is-fading':editSnapshotFading}" :src="editSnapshot" alt="" aria-hidden="true" />
          <button v-for="item in currentPageLinks" :key="item.id" type="button" :class="['pdf-reviewed-link',{unresolved:!item.resolved,selected:overlaySelected('link',item.id)}]" :style="{left:`${item.rect_x*100}%`,top:`${item.rect_y*100}%`,width:`${item.rect_width*100}%`,height:`${item.rect_height*100}%`,...overlayStyle('link',item.id)}" :title="item.resolved ? item.title+' · drag to move, Delete to remove, double-click to follow' : item.title+' — needs review'" :aria-label="`Select link ${item.title}; drag to move or press Delete to remove`" :aria-pressed="overlaySelected('link',item.id)" @pointerdown.stop="beginOverlayDrag($event,'link',item)" @pointermove.stop="updateOverlayDrag" @pointerup.stop="finishOverlayDrag" @pointercancel.stop="finishOverlayDrag" @click.stop="clickOverlay('link',item)" @dblclick.stop="followReviewedLink(item)"><span>{{item.title}}</span></button>
          <span v-if="linkPanelOpen && linkRegion" class="pdf-link-region" :style="{left:`${linkRegion.x*100}%`,top:`${linkRegion.y*100}%`,width:`${linkRegion.width*100}%`,height:`${linkRegion.height*100}%`}" aria-hidden="true"></span>
          <template v-for="item in currentPageAnnotations" :key="item.id">
            <img v-if="item.kind==='image' && item.resolved && annotationImageUrls[item.id]" :class="['pdf-image-overlay',{selected:overlaySelected('annotation',item.id)}]" :src="annotationImageUrls[item.id]" :alt="item.body || 'User-added image overlay'" role="button" tabindex="0" :aria-label="item.body || 'Select image overlay; drag to move or press Delete to remove'" :style="{left:`${item.rect_x*100}%`,top:`${item.rect_y*100}%`,width:`${item.rect_width*100}%`,height:`${item.rect_height*100}%`,...overlayStyle('annotation',item.id)}" @pointerdown.stop="beginOverlayDrag($event,'annotation',item)" @pointermove.stop="updateOverlayDrag" @pointerup.stop="finishOverlayDrag" @pointercancel.stop="finishOverlayDrag" @click.stop="clickOverlay('annotation',item)" @keydown.enter.stop.prevent="clickOverlay('annotation',item)" @keydown.space.stop.prevent="clickOverlay('annotation',item)"/>
            <button v-else-if="item.kind==='highlight'" type="button" :class="['pdf-review-highlight',item.color,{unresolved:!item.resolved,selected:overlaySelected('annotation',item.id)}]" :style="{left:`${item.rect_x*100}%`,top:`${item.rect_y*100}%`,width:`${item.rect_width*100}%`,height:`${item.rect_height*100}%`,...overlayStyle('annotation',item.id)}" :title="item.body || 'Private highlight'" :aria-label="'Select highlight; drag to move or press Delete to remove'" :aria-pressed="overlaySelected('annotation',item.id)" @pointerdown.stop="beginOverlayDrag($event,'annotation',item)" @pointermove.stop="updateOverlayDrag" @pointerup.stop="finishOverlayDrag" @pointercancel.stop="finishOverlayDrag" @click.stop="clickOverlay('annotation',item)"></button>
            <button v-else-if="item.kind==='comment'" type="button" :class="['pdf-review-comment',item.color,{unresolved:!item.resolved,selected:overlaySelected('annotation',item.id)}]" :style="{left:`${item.rect_x*100}%`,top:`${item.rect_y*100}%`,width:`${item.rect_width*100}%`,height:`${item.rect_height*100}%`,...overlayStyle('annotation',item.id)}" :title="item.body" :aria-label="'Select comment; drag to move or press Delete to remove'" :aria-pressed="overlaySelected('annotation',item.id)" @pointerdown.stop="beginOverlayDrag($event,'annotation',item)" @pointermove.stop="updateOverlayDrag" @pointerup.stop="finishOverlayDrag" @pointercancel.stop="finishOverlayDrag" @click.stop="clickOverlay('annotation',item)"><MessageSquare :size="Math.max(12,Math.min(22,item.rect_width*pageZoom*300))"/></button>
            <button v-else-if="item.kind!=='image'" type="button" :class="['pdf-review-shape',item.kind,item.color,{unresolved:!item.resolved,selected:overlaySelected('annotation',item.id)}]" :style="{left:`${item.rect_x*100}%`,top:`${item.rect_y*100}%`,width:`${item.rect_width*100}%`,height:`${item.rect_height*100}%`,...overlayStyle('annotation',item.id)}" :aria-label="`Select ${annotationKindLabel(item.kind)}; drag to move or press Delete to remove`" :aria-pressed="overlaySelected('annotation',item.id)" @pointerdown.stop="beginOverlayDrag($event,'annotation',item)" @pointermove.stop="updateOverlayDrag" @pointerup.stop="finishOverlayDrag" @pointercancel.stop="finishOverlayDrag" @click.stop="clickOverlay('annotation',item)">
              <svg v-if="['draw','box','ellipse','line','arrow'].includes(item.kind)" viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true"><polyline v-if="item.kind==='draw'" :points="drawPolyline(item.body)"/><rect v-else-if="item.kind==='box'" x="3" y="3" width="94" height="94" rx="1"/><ellipse v-else-if="item.kind==='ellipse'" cx="50" cy="50" rx="47" ry="47"/><line v-else x1="4" y1="96" x2="96" y2="4"/><polyline v-if="item.kind==='arrow'" points="68,4 96,4 96,32"/></svg>
              <span v-else>{{item.kind==='check'?'✓':item.kind==='cross'?'✕':item.body}}</span>
            </button>
            <button v-else-if="item.kind==='image'" type="button" :class="['pdf-stale-image-overlay',{selected:overlaySelected('annotation',item.id)}]" :style="{left:`${item.rect_x*100}%`,top:`${item.rect_y*100}%`,width:`${item.rect_width*100}%`,height:`${item.rect_height*100}%`}" title="Image overlay needs review after a revision change" @pointerdown.stop @click.stop="clickOverlay('annotation',item)">Image needs review</button>
          </template>
          <button v-if="selectedOverlayItem?.resolved" type="button" class="pdf-overlay-size-handle" :style="{left:`${(selectedOverlayItem.rect_x+(overlayResizeSize?.width ?? selectedOverlayItem.rect_width)+overlayOffset.x)*100}%`,top:`${(selectedOverlayItem.rect_y+(overlayResizeSize?.height ?? selectedOverlayItem.rect_height)+overlayOffset.y)*100}%`}" :aria-label="`Resize selected ${selectedOverlayLabel}`" @pointerdown.stop="beginOverlayResize" @pointermove.stop="updateOverlayResize" @pointerup.stop="finishOverlayResize" @pointercancel.stop="finishOverlayResize" @click.stop></button>
          <span v-if="annotationPanelOpen && annotationRegion && annotationDraft.kind!=='draw'" class="pdf-annotation-region" :style="{left:`${annotationRegion.x*100}%`,top:`${annotationRegion.y*100}%`,width:`${annotationRegion.width*100}%`,height:`${annotationRegion.height*100}%`}" aria-hidden="true"></span>
          <svg v-if="annotationPanelOpen && annotationDraft.kind==='draw' && annotationDrawing && annotationDrawPoints.length" :class="['pdf-draw-live',annotationDraft.color]" viewBox="0 0 1000 1000" preserveAspectRatio="none" aria-hidden="true"><polyline :points="annotationDrawPoints.map(point=>`${point.x*1000},${point.y*1000}`).join(' ')"/></svg>
          <span v-if="annotationPanelOpen && annotationDraft.kind==='draw' && annotationRegion && !annotationDrawing" :class="['pdf-review-shape','draft','draw',annotationDraft.color]" :style="{left:`${annotationRegion.x*100}%`,top:`${annotationRegion.y*100}%`,width:`${annotationRegion.width*100}%`,height:`${annotationRegion.height*100}%`}" aria-hidden="true"><svg viewBox="0 0 100 100" preserveAspectRatio="none"><polyline :points="drawPolyline(annotationDraft.body)"/></svg></span>
          <span v-if="annotationPanelOpen && annotationRegion && ['box','ellipse','line','arrow','check','cross','date'].includes(annotationDraft.kind)" :class="['pdf-review-shape','draft',annotationDraft.kind,annotationDraft.color]" :style="{left:`${annotationRegion.x*100}%`,top:`${annotationRegion.y*100}%`,width:`${annotationRegion.width*100}%`,height:`${annotationRegion.height*100}%`}" aria-hidden="true"><svg v-if="['box','ellipse','line','arrow'].includes(annotationDraft.kind)" viewBox="0 0 100 100" preserveAspectRatio="none"><rect v-if="annotationDraft.kind==='box'" x="3" y="3" width="94" height="94" rx="1"/><ellipse v-else-if="annotationDraft.kind==='ellipse'" cx="50" cy="50" rx="47" ry="47"/><line v-else x1="4" y1="96" x2="96" y2="4"/><polyline v-if="annotationDraft.kind==='arrow'" points="68,4 96,4 96,32"/></svg><span v-else>{{annotationDraft.kind==='check'?'✓':annotationDraft.kind==='cross'?'✕':annotationDraft.body}}</span></span>
          <img v-if="annotationPanelOpen && annotationRegion && annotationDraft.kind==='image' && annotationPreviewUrl" class="pdf-image-overlay pdf-image-draft" :src="annotationPreviewUrl" alt="Image placement preview" :style="{left:`${annotationRegion.x*100}%`,top:`${annotationRegion.y*100}%`,width:`${annotationRegion.width*100}%`,height:`${annotationRegion.height*100}%`}"/>
          <span v-if="redactionOpen && redactionRegion" class="pdf-redaction-region" :style="{left:`${redactionRegion.x*100}%`,top:`${redactionRegion.y*100}%`,width:`${redactionRegion.width*100}%`,height:`${redactionRegion.height*100}%`}" aria-label="Marked redaction area; drag to move, or drag a corner to resize" @pointerdown.stop="beginRedactionMove" @pointermove.stop="updateRedactionMove" @pointerup.stop="finishRedactionMove" @pointercancel.stop="finishRedactionMove">
            <button v-for="corner in redactionCorners" :key="corner" type="button" :class="['pdf-redaction-handle',corner]" :aria-label="`Resize redaction area from ${corner} corner`" :disabled="redactionBusy || disabled" @pointerdown.stop="beginRedactionResize($event,corner)" @pointermove.stop="updateRedactionResize" @pointerup.stop="finishRedactionResize" @pointercancel.stop="finishRedactionResize"></button>
          </span>
          <button v-for="item in (redactionOpen ? appliedRedactions.filter(entry => entry.page_index === current-1) : [])" :key="item.id" type="button" class="pdf-applied-redaction" :class="{selected:selectedRedactionId===item.id}" :style="{left:`${item.restore_x*100}%`,top:`${item.restore_y*100}%`,width:`${item.restore_width*100}%`,height:`${item.restore_height*100}%`}" :aria-label="`Select applied redaction on page ${current}`" :aria-pressed="selectedRedactionId===item.id" @pointerdown.stop @click.stop="selectedRedactionId=item.id"></button>
          <span v-for="item in markPreview" :key="item.kind" :class="['pdf-output-mark-preview',item.kind,{overlap:item.overlaps_content}]" :style="{left:`${item.relative_rect[0]*100}%`,top:`${item.relative_rect[1]*100}%`,width:`${item.relative_rect[2]*100}%`,height:`${item.relative_rect[3]*100}%`,fontSize:`${item.font_size*pageZoom}px`,opacity:item.opacity,textAlign:item.position.endsWith('left')?'left':item.position.endsWith('right')?'right':'center'}"><span>{{item.text}}</span></span>
          <template v-if="editTextMode">
            <button v-for="run in textRuns" :key="run.index" class="pdf-text-run" :class="{selected:selectedRun?.index===run.index, 'move-target':movingRun?.index===run.index}"
              :style="{left:`${(run.relative_bbox[0]+(movingRun?.index===run.index ? horizontalOffset : 0))*100}%`,top:`${(run.relative_bbox[1]+(movingRun?.index===run.index ? verticalOffset : 0))*100}%`,width:`${(run.relative_bbox[2]-run.relative_bbox[0])*100}%`,height:`${(run.relative_bbox[3]-run.relative_bbox[1])*100}%`}"
              :aria-label="`Edit text: ${run.text}`" :disabled="disabled || editing" @click.stop="clickRun(run)" @pointerdown="beginMove($event, run)" @pointermove="updateMove" @pointerup="finishMove" @pointercancel="finishMove"><span v-if="movingRun?.index===run.index" class="pdf-move-label">{{run.text}}</span></button>
          </template>
          <div v-if="editTextMode && selectedRun" class="pdf-inline-editor" :style="inlineEditorStyle" @click.stop>
            <div class="pdf-edit-scope"><span>{{replacementEngine==='pdfium-trial'?'Whole paragraph':'Selected text'}}</span><button v-if="paragraphTrialAvailable" type="button" class="text-tool" :disabled="editing" @click="replacementEngine=replacementEngine==='pdfium-trial'?'pymupdf':'pdfium-trial'">{{replacementEngine==='pdfium-trial'?'Edit selected text only':'Edit whole paragraph'}}</button></div>
            <label :for="`replacement-${selectedRun.index}`">Replacement</label>
            <textarea :id="`replacement-${selectedRun.index}`" ref="inlineReplacementInput" v-model="replacement" maxlength="2000" rows="2" :disabled="editing || disabled || (replacementEngine==='pdfium-trial' && !selectedParagraph)" aria-describedby="inline-edit-hint" @keydown.esc.stop.prevent="cancelReplacement" @keydown.meta.enter.prevent="applyReplacement" @keydown.ctrl.enter.prevent="applyReplacement"></textarea>
            <span id="inline-edit-hint" class="pdf-inline-hint">{{replacementEngine==='pdfium-trial' ? (selectedParagraph ? 'Editing the whole detected paragraph' : 'Loading whole paragraph…') : replacement.includes('\n') ? 'Lines need clear space below; use Edit whole paragraph to include nearby lines' : 'Apply change'}} · Esc to cancel</span>
            <p v-if="runsError" class="error-msg" role="alert">{{runsError}}</p>
            <div class="pdf-inline-actions">
              <button class="btn secondary small" :disabled="editing" @click="cancelReplacement">Cancel</button>
              <button class="btn secondary small" :disabled="!replacement.trim() || editing || disabled || (replacementEngine==='pdfium-trial' && !selectedParagraph)" @click="applyReplacement">Apply replacement</button>
            </div>
          </div>
      </PdfReaderViewport>

      </div>
      </div>
      <PageOrganizer v-if="organizerOpen" :document="document" :disabled="!!disabled || editing" @close="organizerOpen=false" @modified="emit('modified')" @error="emit('error',$event)"/>

    </template>
    <div v-else class="viewer-empty"><FileSearch :size="44"/><strong>Document preview</strong><span>Select a file to review its pages</span><div v-if="expanded" class="empty-reader-history"><button class="btn secondary small" aria-label="Undo last matter change" :title="undoDescription" :disabled="!canUndo || disabled" @click="emit('undo')"><Undo2 :size="16"/> Undo</button><button class="btn secondary small" aria-label="Redo last matter change" :title="redoDescription" :disabled="!canRedo || disabled" @click="emit('redo')"><Redo2 :size="16"/> Redo</button></div></div>
  </div>
</template>
