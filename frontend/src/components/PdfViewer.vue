<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { ChevronLeft, ChevronRight, RotateCw, Trash2, FileSearch, FilePenLine, LayoutGrid, LoaderCircle, Type, Pencil } from 'lucide-vue-next'
import * as pdfjs from 'pdfjs-dist'
import workerSrc from 'pdfjs-dist/build/pdf.worker.min.mjs?url'
import type { PDFDocumentProxy, RenderTask } from 'pdfjs-dist'
import type { MatterDocument } from '../lib/types'
import { api, pdfBytes } from '../lib/api'
import { friendlyError } from '../lib/format'

pdfjs.GlobalWorkerOptions.workerSrc = workerSrc
const props = defineProps<{ document: MatterDocument | null, disabled?: boolean, pageTarget?: {documentId:string, page:number, sequence:number} }>()
const emit = defineEmits<{ modified: [], error: [message: string], pageTools: [] }>()
const canvas = ref<HTMLCanvasElement | null>(null)
const canvasWrap = ref<HTMLDivElement | null>(null)
const thumbnailWrap = ref<HTMLDivElement | null>(null)
const current = ref(1)
const loading = ref(false)
const editing = ref(false)
const viewerError = ref('')
const scalePct = ref(100)
const showThumbnails = ref(false)
const loadedPageCount = ref(0)
const textMode = ref(false)
const editTextMode = ref(false)
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
let runsSequence = 0
let pdf: PDFDocumentProxy | null = null
let renderTask: RenderTask | null = null
let thumbnailTasks: RenderTask[] = []
let thumbnailSequence = 0
let epoch = 0
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

async function render() {
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
    if (pdf !== currentPdf) return
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
  const call = ++epoch
  loading.value = true
  viewerError.value = ''
  current.value = 1
  loadedPageCount.value = 0
  placement.value = null
  selectedRun.value = null
  if (renderTask) { renderTask.cancel(); renderTask = null }
  thumbnailSequence++
  thumbnailTasks.forEach(task => task.cancel())
  thumbnailTasks = []
  if (pdf) { const previous = pdf; pdf = null; void previous.destroy() }
  if (!doc) { loading.value = false; return }
  try {
    const bytes = await pdfBytes(doc.id)
    if (call !== epoch) return
    pdf = await pdfjs.getDocument({ data: bytes }).promise
    loadedPageCount.value = pdf.numPages
    if (props.pageTarget?.documentId === doc.id) current.value = Math.min(Math.max(1, props.pageTarget.page), pdf.numPages)
    await nextTick()
    if (call !== epoch) return
    await render()
    await renderThumbnails()
  } catch (err) { viewerError.value = friendlyError(err) }
  finally { if (call === epoch) loading.value = false }
}
watch(() => `${props.document?.id || ''}|${props.document?.updated_at || ''}`, () => void loadPdf(props.document), { immediate: true })
watch(() => props.pageTarget?.sequence, () => {
  const target = props.pageTarget
  if (target && target.documentId === props.document?.id && pdf)
    current.value = Math.min(Math.max(1, target.page), total.value)
})
watch([current, scalePct], () => void render())
watch(current, () => void renderThumbnails())
watch(current, () => { placement.value = null })
watch([current, () => props.document?.id, () => props.document?.revision, editTextMode], async () => {
  const sequence = ++runsSequence
  textRuns.value = []
  selectedRun.value = null
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
onBeforeUnmount(() => { epoch++; thumbnailSequence++; renderTask?.cancel(); thumbnailTasks.forEach(task => task.cancel()); if (pdf) void pdf.destroy() })

async function edit(action: 'rotate' | 'delete' | 'reorder', params: unknown) {
  if (!props.document || editing.value) return
  editing.value = true
  try {
    await api(`/documents/${props.document.id}/pages/${action}`, 'POST', params)
    emit('modified')
  } catch (err) { emit('error', friendlyError(err)) }
  finally { editing.value = false }
}
function move(step: number) {
  if (!props.document || total.value < 2) return
  const from = current.value - 1
  const to = from + step
  if (to < 0 || to >= total.value) return
  const order = Array.from({ length: total.value }, (_, i) => i)
  ;[order[from], order[to]] = [order[to], order[from]]
  void edit('reorder', { order })
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
  editing.value = true
  try {
    await api(`/documents/${props.document.id}/pages/text-overlay`, 'POST', {
      page_index: placement.value.page - 1, x: placement.value.x, y: placement.value.y,
      text: overlayText.value, font_size: overlaySize.value,
    })
    placement.value = null
    overlayText.value = ''
    emit('modified')
  } catch (err) { emit('error', friendlyError(err)) }
  finally { editing.value = false }
}
function selectRun(run: TextRun) {
  if (props.disabled || editing.value) return
  selectedRun.value = run
  replacement.value = run.text
  runsError.value = ''
  editNotice.value = ''
}
async function applyReplacement() {
  if (!props.document || !selectedRun.value || !replacement.value.trim() || props.disabled || editing.value) return
  editing.value = true
  runsError.value = ''
  try {
    const result = await api<{edit_layout:{font_size:number,line_count:number,font_size_reduced:boolean}}>(`/documents/${props.document.id}/pages/replace-text-run`, 'POST', {
      page_index: current.value - 1, span_index: selectedRun.value.index,
      expected_revision: runsRevision.value, expected_text: selectedRun.value.text,
      replacement: replacement.value,
    })
    editNotice.value = `Applied in ${result.edit_layout.line_count} line(s) at ${result.edit_layout.font_size} pt${result.edit_layout.font_size_reduced ? ' (reduced to fit)' : ''}. Review the page.`
    selectedRun.value = null
    emit('modified')
  } catch (err) { runsError.value = friendlyError(err) }
  finally { editing.value = false }
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
        <div class="toolbar-group"><button class="icon-btn small" :aria-label="showThumbnails?'Hide thumbnails':'Show thumbnails'" :aria-pressed="showThumbnails" title="Toggle page thumbnails" @click="showThumbnails=!showThumbnails"><LayoutGrid :size="16"/></button><select v-model.number="scalePct" class="zoom-select" aria-label="Zoom"><option :value="75">75%</option><option :value="100">100%</option><option :value="125">125%</option><option :value="150">150%</option></select></div>
      </div>
      <div class="page-editbar">
        <button class="text-tool" :disabled="editing || loading" title="Open page tools for this document" @click="emit('pageTools')"><FilePenLine :size="15"/> Page tools</button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="textMode" @click="textMode=!textMode; editTextMode=false; placement=null"><Type :size="15"/> Add text</button>
        <button class="text-tool" :disabled="editing || loading || disabled" :aria-pressed="editTextMode" @click="editTextMode=!editTextMode; textMode=false; placement=null"><Pencil :size="15"/> Edit text</button>
        <button class="text-tool page-icon-tool" :disabled="editing || loading" aria-label="Rotate current page" title="Rotate current page" @click="edit('rotate', {page_index:current-1,degrees:90})"><RotateCw :size="16"/></button>
        <button class="text-tool page-icon-tool" :disabled="editing || loading || current<=1" aria-label="Move page left" title="Move page left" @click="move(-1)"><ChevronLeft :size="16"/></button>
        <button class="text-tool page-icon-tool" :disabled="editing || loading || current>=total" aria-label="Move page right" title="Move page right" @click="move(1)"><ChevronRight :size="16"/></button>
        <button class="text-tool page-icon-tool danger" :disabled="editing || loading || total<=1" aria-label="Delete page from working copy" title="Delete page from working copy" @click="deleteCurrent"><Trash2 :size="16"/></button>
      </div>
      <div v-if="textMode" class="text-overlay-panel">
        <strong>Visible text overlay</strong>
        <p>Type printable English text, then click its top-left point on the page. This adds content to a working copy; it does not change existing text. Review overlap and spacing before export.</p>
        <label>Text<textarea v-model="overlayText" maxlength="1000" rows="3" :disabled="editing || disabled"/></label>
        <label>Size (pt)<input v-model.number="overlaySize" type="number" min="6" max="36" :disabled="editing || disabled"/></label>
        <div class="page-tool-actions"><span>{{placement ? `Page ${placement.page} · ${Math.round(placement.x*100)}%, ${Math.round(placement.y*100)}%` : 'Click a position on the page'}}</span><button class="btn secondary small" :disabled="!placement || !overlayText.trim() || editing || disabled" @click="placeText">Apply text</button></div>
      </div>
      <div v-if="editTextMode" class="text-overlay-panel" aria-label="Edit existing text">
        <strong>Edit selectable text</strong>
        <p>Click highlighted text and rewrite it below. Text can use clear space to the right or wrap into clear space below; the font may shrink by up to 25%. Nearby content is not moved. The imported original stays intact. Review the page after applying. This is not a confidential redaction tool.</p>
        <p v-if="runsError" class="error-msg" role="alert">{{runsError}}</p>
        <p v-if="editNotice" role="status">{{editNotice}}</p>
        <p v-else-if="!textRuns.length">No selectable horizontal text found on this page. Scanned text needs OCR and review.</p>
        <template v-if="selectedRun">
          <label>Selected text<input :value="selectedRun.text" readonly/></label>
          <label>Replacement<textarea v-model="replacement" maxlength="2000" rows="3" :disabled="editing || disabled"/></label>
          <button class="btn secondary small" :disabled="!replacement.trim() || editing || disabled" @click="applyReplacement">Apply replacement</button>
        </template>
      </div>
      <div v-if="total > 1 && showThumbnails" ref="thumbnailWrap" class="thumbnail-strip" aria-label="Page thumbnails">
        <button v-for="number in thumbnailPages" :key="number" class="thumbnail-button" :class="{selected:number===current}" :aria-label="'Go to page '+number" @click="current=number"><canvas :data-page="number"></canvas><span>{{number}}</span></button>
      </div>
      <div ref="canvasWrap" class="canvas-scroll">
        <div v-if="loading || editing" class="preview-status"><LoaderCircle :size="18" class="spin"/> {{ editing ? 'Updating PDF…' : 'Loading PDF…' }}</div>
        <div v-if="viewerError" class="preview-status error-msg">{{ viewerError }}</div>
        <div class="pdf-placement-surface" :class="{'placing-text':textMode && !disabled}" @click="choosePlacement">
          <canvas ref="canvas" class="pdf-paper" :style="{visibility: viewerError ? 'hidden' : 'visible'}" />
          <span v-if="placement && placement.page===current" class="pdf-placement-marker" :style="{left:`${placement.x*100}%`,top:`${placement.y*100}%`}" aria-hidden="true"></span>
          <template v-if="editTextMode && !runsError">
            <button v-for="run in textRuns" :key="run.index" class="pdf-text-run" :class="{selected:selectedRun?.index===run.index}"
              :style="{left:`${run.relative_bbox[0]*100}%`,top:`${run.relative_bbox[1]*100}%`,width:`${(run.relative_bbox[2]-run.relative_bbox[0])*100}%`,height:`${(run.relative_bbox[3]-run.relative_bbox[1])*100}%`}"
              :aria-label="`Edit text: ${run.text}`" :disabled="disabled || editing" @click.stop="selectRun(run)"></button>
          </template>
        </div>
      </div>
    </template>
    <div v-else class="viewer-empty"><FileSearch :size="44"/><strong>Document preview</strong><span>Select a file to review its pages</span></div>
  </div>
</template>
