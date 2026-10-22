<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue'
import { ArrowDown, ArrowUp, CheckSquare2, ChevronLeft, ChevronRight, GripVertical, RotateCcw, RotateCw, Trash2, X } from 'lucide-vue-next'
import * as pdfjs from 'pdfjs-dist'
import workerSrc from '../lib/pdfWorker?worker&url'
import type { PDFDocumentLoadingTask, PDFDocumentProxy, RenderTask } from 'pdfjs-dist'
import { api, ApiError, pdfDocumentSource } from '../lib/api'
import { friendlyError } from '../lib/format'
import { signatureRiskDocuments, signatureStatusLabel } from '../lib/signatures'
import type { MatterDocument } from '../lib/types'

pdfjs.GlobalWorkerOptions.workerSrc = workerSrc
const props = defineProps<{ document: MatterDocument, disabled?: boolean }>()
const emit = defineEmits<{ close: [], modified: [], error: [message: string] }>()
const dialog = ref<HTMLDialogElement | null>(null)
const grid = ref<HTMLDivElement | null>(null)
const confirmButton = ref<HTMLButtonElement | null>(null)
const currentDocument = shallowRef(props.document)
const selected = ref(new Set<number>())
const selectionAnchor = ref<number | null>(null)
const rangeText = ref('')
const selectionError = ref('')
const error = ref('')
const notice = ref('')
const conflict = ref(false)
const loading = ref(false)
const working = ref(false)
const order = ref<number[]>([])
const chunk = ref(0)
const chunkSize = 24
const dragging = ref<number | null>(null)
const dropBefore = ref<number | null>(null)
const droppingAtEnd = ref(false)
const thumbnailState = ref<Record<number, 'ready' | 'error'>>({})
const signatureAcknowledged = ref(false)
type Confirmation = { operation: 'rotate' | 'delete' | 'reorder', pages: number[], revision: number, degrees?: number, order?: number[] }
const confirmation = ref<Confirmation | null>(null)
const pageCount = computed(() => currentDocument.value.page_count)
const chunkCount = computed(() => Math.max(1, Math.ceil(pageCount.value / chunkSize)))
const visiblePages = computed(() => order.value.slice(chunk.value * chunkSize, (chunk.value + 1) * chunkSize))
const orderChanged = computed(() => order.value.some((page, position) => page !== position))
const unavailable = computed(() => !!props.disabled || loading.value || working.value || conflict.value)
const controlsLocked = computed(() => unavailable.value || !!confirmation.value)
const riskySignature = computed(() => signatureRiskDocuments([currentDocument.value]).length > 0)
const selectionLabel = computed(() => describePages([...selected.value]))
const confirmationLabel = computed(() => confirmation.value ? describePages(confirmation.value.pages) : '')
const actionLabel = computed(() => {
  const pending = confirmation.value
  if (!pending) return ''
  if (pending.operation === 'reorder') return 'Apply page order'
  return `${pending.operation === 'delete' ? 'Delete' : 'Rotate'} ${pending.pages.length} ${pending.pages.length === 1 ? 'page' : 'pages'}`
})
let pdf: PDFDocumentProxy | null = null
let loadingTask: PDFDocumentLoadingTask | null = null
let loadSequence = 0
let renderSequence = 0
let disposed = false
const canvases = new Map<number, HTMLCanvasElement>()
const renders = new Set<RenderTask>()

function describePages(indices: number[]): string {
  const sorted = [...indices].sort((a, b) => a - b)
  const ranges: string[] = []
  for (let position = 0; position < sorted.length; position++) {
    const start = sorted[position]
    let end = start
    while (position + 1 < sorted.length && sorted[position + 1] === end + 1) end = sorted[++position]
    ranges.push(start === end ? String(start + 1) : `${start + 1}–${end + 1}`)
  }
  return ranges.join(', ')
}

function setCanvas(page: number, element: unknown) {
  if (element instanceof HTMLCanvasElement) canvases.set(page, element)
  else {
    const previous = canvases.get(page)
    if (previous) { previous.width = 0; previous.height = 0 }
    canvases.delete(page)
  }
}

function cancelRenders() {
  renderSequence++
  for (const task of renders) task.cancel()
  renders.clear()
}

async function renderThumbnails() {
  cancelRenders()
  const sequence = renderSequence
  const source = pdf
  thumbnailState.value = {}
  if (!source) return
  await nextTick()
  // Pagination keeps both mounted canvases and PDF.js rendering work bounded.
  for (const index of visiblePages.value) {
    if (sequence !== renderSequence || disposed || source !== pdf) return
    const canvas = canvases.get(index)
    if (!canvas) continue
    let page: Awaited<ReturnType<PDFDocumentProxy['getPage']>> | undefined
    let task: RenderTask | undefined
    try {
      page = await source.getPage(index + 1)
      if (sequence !== renderSequence || disposed || source !== pdf) return
      const initial = page.getViewport({ scale: 1 })
      const scale = Math.min(176 / initial.width, 214 / initial.height)
      const density = Math.min(window.devicePixelRatio || 1, 2)
      const viewport = page.getViewport({ scale: scale * density })
      canvas.width = Math.ceil(viewport.width)
      canvas.height = Math.ceil(viewport.height)
      canvas.style.width = `${viewport.width / density}px`
      canvas.style.height = `${viewport.height / density}px`
      const context = canvas.getContext('2d')
      if (!context) throw new Error('Canvas is unavailable')
      task = page.render({ canvasContext: context, viewport })
      renders.add(task)
      await task.promise
      if (sequence === renderSequence) thumbnailState.value[index] = 'ready'
    } catch (cause) {
      if (sequence === renderSequence && !disposed && (cause as Error)?.name !== 'RenderingCancelledException')
        thumbnailState.value[index] = 'error'
    } finally {
      if (task) renders.delete(task)
      page?.cleanup()
    }
  }
}

async function loadDocument(document: MatterDocument) {
  const sequence = ++loadSequence
  cancelRenders()
  const previousLoading = loadingTask
  const previousPdf = pdf
  loadingTask = null
  pdf = null
  if (previousLoading) await previousLoading.destroy().catch(() => undefined)
  else if (previousPdf) await previousPdf.destroy().catch(() => undefined)
  if (sequence !== loadSequence || disposed) return
  currentDocument.value = document
  order.value = Array.from({ length: document.page_count }, (_, index) => index)
  chunk.value = Math.min(chunk.value, Math.max(0, Math.ceil(document.page_count / chunkSize) - 1))
  selected.value = new Set()
  selectionAnchor.value = null
  confirmation.value = null
  selectionError.value = ''
  rangeText.value = ''
  error.value = ''
  conflict.value = false
  loading.value = true
  try {
    const source = await pdfDocumentSource(document.id, document.revision)
    if (sequence !== loadSequence || disposed) return
    const task = pdfjs.getDocument({ ...source, disableAutoFetch: true, disableStream: true })
    loadingTask = task
    const loaded = await task.promise
    if (sequence !== loadSequence || disposed) { await loaded.destroy(); return }
    pdf = loaded
    if (loaded.numPages !== document.page_count) throw new Error('Page count changed. Reload the document before organizing pages.')
    await renderThumbnails()
  } catch (cause) {
    if (sequence === loadSequence && !disposed) error.value = friendlyError(cause)
  } finally {
    if (sequence === loadSequence) loading.value = false
  }
}

function toggleSelection(index: number, event: MouseEvent) {
  if (controlsLocked.value) return
  const next = new Set(selected.value)
  const checked = (event.currentTarget as HTMLInputElement).checked
  if (event.shiftKey && selectionAnchor.value !== null) {
    const first = order.value.indexOf(selectionAnchor.value)
    const last = order.value.indexOf(index)
    for (let position = Math.min(first, last); position <= Math.max(first, last); position++) {
      if (checked) next.add(order.value[position])
      else next.delete(order.value[position])
    }
  } else if (checked) next.add(index)
  else next.delete(index)
  selected.value = next
  selectionAnchor.value = index
  selectionError.value = ''
}

function selectRanges() {
  if (controlsLocked.value) return
  try {
    const next = new Set<number>()
    for (const part of rangeText.value.split(',')) {
      const match = part.trim().match(/^(\d+)(?:\s*[-–]\s*(\d+))?$/)
      if (!match) throw new Error('Enter page numbers or ranges, for example 1, 3–5.')
      const first = Number(match[1])
      const last = Number(match[2] || match[1])
      if (!Number.isSafeInteger(first) || !Number.isSafeInteger(last) || first < 1 || last < first || last > pageCount.value)
        throw new Error(`Choose pages from 1 to ${pageCount.value}, with each range in ascending order.`)
      for (let page = first; page <= last; page++) next.add(page - 1)
    }
    selected.value = next
    selectionAnchor.value = null
    selectionError.value = ''
  } catch (cause) { selectionError.value = (cause as Error).message }
}

function selectAll() { selected.value = new Set(order.value); selectionAnchor.value = null }
function clearSelection() { selected.value = new Set(); selectionAnchor.value = null }

async function movePage(index: number, offset: number) {
  if (controlsLocked.value) return
  const position = order.value.indexOf(index)
  const destination = position + offset
  if (destination < 0 || destination >= order.value.length) return
  const next = [...order.value]
  next.splice(position, 1)
  next.splice(destination, 0, index)
  order.value = next
  chunk.value = Math.floor(destination / chunkSize)
  notice.value = `Page ${index + 1} moved to position ${destination + 1}. Apply page order to save.`
  await nextTick()
  const card = grid.value?.querySelector<HTMLElement>(`[data-page="${index}"]`)
  card?.querySelector<HTMLButtonElement>(`button[data-direction="${offset}"]`)?.focus()
}

function startDrag(index: number, event: DragEvent) {
  if (controlsLocked.value) { event.preventDefault(); return }
  dragging.value = index
  event.dataTransfer?.setData('text/plain', String(index))
  if (event.dataTransfer) event.dataTransfer.effectAllowed = 'move'
}
function endDrag() { dragging.value = null; dropBefore.value = null; droppingAtEnd.value = false }
function dropPage(before: number | null) {
  if (controlsLocked.value || dragging.value === null || dragging.value === before) { endDrag(); return }
  const index = dragging.value
  const next = order.value.filter(page => page !== index)
  const destination = before === null ? next.length : next.indexOf(before)
  if (destination < 0) { endDrag(); return }
  next.splice(destination, 0, index)
  order.value = next
  notice.value = `Page ${index + 1} moved to position ${destination + 1}. Apply page order to save.`
  endDrag()
}

async function reviewOperation(operation: Confirmation['operation'], degrees?: number) {
  if (controlsLocked.value) return
  const pages = operation === 'reorder' ? [...order.value] : [...selected.value].sort((a, b) => a - b)
  if (!pages.length || (operation === 'delete' && pages.length === pageCount.value)) return
  confirmation.value = { operation, degrees, pages, revision: currentDocument.value.revision,
    ...(operation === 'reorder' ? { order: [...order.value] } : {}) }
  signatureAcknowledged.value = false
  error.value = ''
  await nextTick()
  confirmButton.value?.focus()
}

async function applyOperation() {
  const pending = confirmation.value
  if (!pending || unavailable.value || (riskySignature.value && !signatureAcknowledged.value)) return
  working.value = true
  error.value = ''
  try {
    const body = { expected_revision: pending.revision, acknowledge_signature_risk: signatureAcknowledged.value,
      ...(pending.operation === 'reorder' ? { order: pending.order } :
        { operation: pending.operation, pages: pending.pages, ...(pending.degrees ? { degrees: pending.degrees } : {}) }) }
    const updated = await api<MatterDocument>(`/documents/${currentDocument.value.id}/pages/${pending.operation === 'reorder' ? 'reorder' : 'batch'}`, 'POST', body, 120_000)
    notice.value = `${pending.operation === 'reorder' ? 'Page order applied' : `${pending.pages.length} ${pending.pages.length === 1 ? 'page' : 'pages'} ${pending.operation === 'delete' ? 'deleted' : 'rotated'}`}. Undo restores this entire action.`
    await loadDocument(updated)
    emit('modified')
  } catch (cause) {
    error.value = friendlyError(cause)
    conflict.value = cause instanceof ApiError && cause.status === 409
    confirmation.value = null
    emit('error', error.value)
  } finally { working.value = false }
}

async function reloadDocument() {
  if (working.value) return
  working.value = true
  try {
    const documents = await api<MatterDocument[]>(`/matters/${currentDocument.value.matter_id}/documents`)
    const document = documents.find(item => item.id === currentDocument.value.id)
    if (!document) throw new Error('This document is no longer available. Close the organizer and refresh the matter.')
    await loadDocument(document)
    emit('modified')
  } catch (cause) { error.value = friendlyError(cause) }
  finally { working.value = false }
}

function requestClose() {
  if (working.value) return
  if (orderChanged.value && !window.confirm('Discard the unapplied page order?')) return
  dialog.value?.close()
  emit('close')
}

watch([chunk, order], () => { if (!loading.value) void renderThumbnails() })
watch(() => [props.document.id, props.document.revision], () => {
  if (props.document.id !== currentDocument.value.id || props.document.revision !== currentDocument.value.revision)
    void loadDocument(props.document)
})
onMounted(() => { dialog.value?.showModal(); void loadDocument(props.document) })
onBeforeUnmount(() => {
  disposed = true
  loadSequence++
  cancelRenders()
  if (loadingTask) void loadingTask.destroy().catch(() => undefined)
  else if (pdf) void pdf.destroy().catch(() => undefined)
  for (const canvas of canvases.values()) { canvas.width = 0; canvas.height = 0 }
  canvases.clear()
})
</script>

<template>
  <Teleport to="body">
    <dialog ref="dialog" class="page-organizer" aria-labelledby="organizer-title" aria-describedby="organizer-description" @keydown.stop @cancel.prevent="requestClose">
      <header class="organizer-heading">
        <div><span class="organizer-eyebrow">WORKING COPY · REVISION {{currentDocument.revision}}</span><h2 id="organizer-title">Organize pages</h2><p class="organizer-document">{{currentDocument.title}}</p></div>
        <button class="icon-btn" aria-label="Close page organizer" :disabled="working" @click="requestClose"><X :size="20"/></button>
      </header>
      <p id="organizer-description" class="organizer-description">Select pages to rotate or delete. Drag a page to a marked position, or use its arrow buttons to preview a new order. Page numbers refer to this working PDF.</p>
      <div class="organizer-selection">
        <div class="organizer-selection-actions"><strong aria-live="polite">{{selected.size}} of {{pageCount}} selected</strong><button class="btn secondary small" :disabled="controlsLocked" @click="selectAll"><CheckSquare2 :size="15"/>Select all</button><button class="btn secondary small" :disabled="controlsLocked || !selected.size" @click="clearSelection">Clear</button><span class="organizer-hint">Shift-click selects a range.</span></div>
        <form class="organizer-ranges" @submit.prevent="selectRanges"><label for="organizer-ranges">Select page ranges</label><input id="organizer-ranges" v-model="rangeText" placeholder="1, 3–5" :disabled="controlsLocked" aria-describedby="organizer-range-help"/><button class="btn secondary small" :disabled="controlsLocked || !rangeText.trim()">Select ranges</button></form>
        <span id="organizer-range-help" class="organizer-hint">Ranges replace the current selection. Selections remain when you browse the grid.</span>
        <p v-if="selectionError" class="organizer-error" role="alert">{{selectionError}}</p>
        <p v-if="selected.size" class="organizer-selected-pages">Selected pages: {{selectionLabel}}</p>
      </div>
      <div class="organizer-toolbar">
        <button class="btn secondary small" :disabled="controlsLocked || !selected.size || orderChanged" @click="reviewOperation('rotate',270)"><RotateCcw :size="16"/>Rotate left</button>
        <button class="btn secondary small" :disabled="controlsLocked || !selected.size || orderChanged" @click="reviewOperation('rotate',90)"><RotateCw :size="16"/>Rotate right</button>
        <button class="btn secondary small organizer-delete" :disabled="controlsLocked || !selected.size || selected.size===pageCount || orderChanged" @click="reviewOperation('delete')"><Trash2 :size="16"/>Delete selected</button>
        <span v-if="selected.size===pageCount && !orderChanged" class="organizer-hint">Keep at least one page.</span>
        <span v-if="orderChanged" class="organizer-hint">Apply or reset the order before rotating or deleting.</span>
      </div>
      <section v-if="confirmation" class="organizer-confirmation" aria-label="Review page action">
        <strong>{{actionLabel}} · {{currentDocument.title}}</strong>
        <p v-if="confirmation.operation==='reorder'">New order: {{confirmation.order!.map(index=>index+1).join(', ')}}. This creates one undoable working-copy revision.</p>
        <p v-else>Pages {{confirmationLabel}} will be {{confirmation.operation==='delete' ? 'removed from this working copy' : `rotated ${confirmation.degrees===270 ? '90° left' : '90° right'}`}}. This is one undoable action.</p>
        <p>Imported originals remain unchanged. Revision-linked bookmarks, links and notes may need review after page changes.</p>
        <label v-if="riskySignature" class="organizer-signature"><input v-model="signatureAcknowledged" type="checkbox" :disabled="working"/><span>{{signatureStatusLabel(currentDocument.signature_status)}}. I reviewed this selection and understand that editing may invalidate or omit signature protections; certificate validity and visible signatures are unverified.</span></label>
        <div><button ref="confirmButton" class="btn primary small" :disabled="unavailable || (riskySignature && !signatureAcknowledged)" @click="applyOperation">{{working ? 'Applying…' : actionLabel}}</button><button class="btn secondary small" :disabled="working" @click="confirmation=null">Cancel</button></div>
      </section>
      <div v-if="error" class="organizer-error organizer-load-error" role="alert">{{error}} <button class="btn secondary small" :disabled="working" @click="reloadDocument">Reload document</button></div>
      <div ref="grid" class="organizer-grid" :aria-busy="loading || working">
        <article v-for="(index, position) in visiblePages" :key="`${currentDocument.id}:${currentDocument.revision}:${index}`" class="organizer-page" :class="{'is-selected':selected.has(index),'is-dragging':dragging===index,'drop-before':dropBefore===index && dragging!==index}" :data-page="index" @dragover.prevent="dropBefore=index;droppingAtEnd=false" @drop.prevent="dropPage(index)">
          <span v-if="dropBefore===index && dragging!==null && dragging!==index" class="organizer-drop-label">Place before page {{index+1}}</span>
          <label class="organizer-page-label"><input type="checkbox" :checked="selected.has(index)" :disabled="controlsLocked" :aria-label="`Select page ${index+1}`" @click="toggleSelection(index,$event)"/><strong>Page {{index+1}}</strong><small v-if="orderChanged">Position {{chunk*chunkSize+position+1}}</small></label>
          <div class="organizer-thumbnail"><canvas :ref="element=>setCanvas(index,element)" aria-hidden="true" :class="{'thumbnail-ready':thumbnailState[index]==='ready'}"/><span v-if="thumbnailState[index]!=='ready'">{{thumbnailState[index]==='error' ? 'Preview unavailable' : 'Loading preview…'}}</span></div>
          <div class="organizer-page-actions"><button class="icon-btn small" :draggable="!controlsLocked" :disabled="controlsLocked" :aria-label="`Drag page ${index+1} to reorder`" title="Drag to reorder" @dragstart="startDrag(index,$event)" @dragend="endDrag"><GripVertical :size="16"/></button><span class="organizer-hint">{{chunk*chunkSize+position+1}} / {{pageCount}}</span><button class="icon-btn small" data-direction="-1" :disabled="controlsLocked || chunk*chunkSize+position===0" :aria-label="`Move page ${index+1} earlier`" title="Move one position earlier" @click="movePage(index,-1)"><ArrowUp :size="16"/></button><button class="icon-btn small" data-direction="1" :disabled="controlsLocked || chunk*chunkSize+position===pageCount-1" :aria-label="`Move page ${index+1} later`" title="Move one position later" @click="movePage(index,1)"><ArrowDown :size="16"/></button></div>
        </article>
        <div v-if="dragging!==null" class="organizer-end-target" :class="{'drop-active':droppingAtEnd}" @dragover.prevent="droppingAtEnd=true;dropBefore=null" @drop.prevent="dropPage(null)">Drop here to move page {{dragging+1}} to the end of this document</div>
      </div>
      <footer class="organizer-footer">
        <div class="organizer-pagination"><button class="icon-btn" :disabled="controlsLocked || chunk===0" aria-label="Previous page grid" @click="chunk--"><ChevronLeft :size="18"/></button><span>Showing {{chunk*chunkSize+1}}–{{Math.min((chunk+1)*chunkSize,pageCount)}} of {{pageCount}}</span><button class="icon-btn" :disabled="controlsLocked || chunk>=chunkCount-1" aria-label="Next page grid" @click="chunk++"><ChevronRight :size="18"/></button></div>
        <p class="organizer-notice" role="status">{{loading ? 'Loading page previews…' : notice || 'Changes affect the working copy. Each applied action can be undone.'}}</p>
        <div v-if="orderChanged" class="organizer-order-actions"><button class="btn secondary small" :disabled="controlsLocked" @click="order=Array.from({length:pageCount},(_,index)=>index);notice='Page order reset.'">Reset order</button><button class="btn primary small" :disabled="controlsLocked" @click="reviewOperation('reorder')">Apply page order</button></div>
        <button v-else class="btn secondary small" :disabled="working" @click="requestClose">Done</button>
      </footer>
    </dialog>
  </Teleport>
</template>

<style scoped>
.page-organizer{width:min(1160px,calc(100vw - 40px));max-width:none;height:min(900px,calc(100dvh - 40px));max-height:none;padding:0;border:1px solid var(--border);border-radius:16px;background:var(--panel);color:var(--text);box-shadow:0 24px 90px #0005;overflow:hidden}
.page-organizer[open]{display:flex;flex-direction:column}.page-organizer::backdrop{background:#08182a99;backdrop-filter:blur(3px)}
.organizer-heading{display:flex;align-items:flex-start;justify-content:space-between;padding:22px 26px 10px;gap:20px}.organizer-eyebrow{font-size:10px;font-weight:800;letter-spacing:1.4px;color:var(--muted)}.organizer-heading h2{font-size:24px;letter-spacing:-.6px;margin:5px 0}.organizer-document{margin:0;font-size:13px;max-width:900px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.organizer-description{margin:0;padding:0 26px 14px;color:var(--muted);font-size:12px;line-height:1.6}
.organizer-selection{padding:12px 26px;border-block:1px solid var(--border);display:flex;flex-wrap:wrap;align-items:center;gap:10px 18px}.organizer-selection-actions{display:flex;align-items:center;gap:10px;flex-wrap:wrap}.organizer-selection-actions strong{font-size:13px}.organizer-hint{font-size:11px;color:var(--muted);line-height:1.5}.organizer-ranges{display:flex;align-items:center;gap:8px;margin-left:auto;font-size:11px}.organizer-ranges input{width:145px;padding:7px 9px;border:1px solid var(--border);border-radius:6px;background:var(--surface);color:var(--text)}.organizer-selected-pages{width:100%;margin:0;max-height:44px;overflow:auto;color:var(--primary);font-size:11px;overflow-wrap:anywhere}.organizer-toolbar{display:flex;flex-wrap:wrap;align-items:center;gap:8px;padding:12px 26px}.organizer-delete{color:#be3c45}.organizer-grid{flex:1;min-height:140px;overflow:auto;display:grid;grid-template-columns:repeat(auto-fill,minmax(180px,1fr));align-content:start;gap:18px;padding:18px 26px 24px;background:var(--surface-soft)}
.organizer-page{border:2px solid transparent;border-radius:10px;background:var(--panel);box-shadow:0 2px 8px #0d22300a;position:relative;padding:10px;min-width:0}.organizer-page.is-selected{border-color:var(--primary)}.organizer-page.is-dragging{opacity:.45}.organizer-page.drop-before{border-top-color:var(--primary);box-shadow:0 -4px 0 var(--primary)}.organizer-page-label{display:flex;align-items:center;gap:8px;font-size:12px;cursor:pointer;min-height:24px}.organizer-page-label input{accent-color:#1d69c3;width:16px;height:16px}.organizer-page-label small{font-size:10px;color:var(--muted);margin-left:auto}.organizer-thumbnail{height:228px;display:flex;align-items:center;justify-content:center;position:relative;margin:8px 0;background:var(--surface-soft);border-radius:4px}.organizer-thumbnail canvas{display:block;max-width:100%;max-height:214px;opacity:0;background:#fff;box-shadow:0 2px 7px #0002}.organizer-thumbnail canvas.thumbnail-ready{opacity:1}.organizer-thumbnail>span{position:absolute;font-size:11px;color:var(--muted)}.organizer-page-actions{display:flex;align-items:center;gap:4px}.organizer-page-actions>span{margin-right:auto}.organizer-page-actions button:first-child{cursor:grab}.organizer-drop-label{position:absolute;z-index:1;top:-13px;left:8px;background:var(--primary);color:var(--panel);font-size:10px;border-radius:3px;padding:3px 6px}.organizer-end-target{grid-column:1/-1;text-align:center;border:2px dashed var(--border);padding:18px;font-size:12px;border-radius:8px}.organizer-end-target.drop-active{border-color:var(--primary);color:var(--primary)}
.organizer-confirmation{padding:15px 26px;border-block:1px solid var(--border);background:var(--surface-soft);font-size:12px;flex-shrink:0;max-height:260px;overflow:auto}.organizer-confirmation p{line-height:1.5;margin:7px 0;overflow-wrap:anywhere}.organizer-confirmation>div{display:flex;gap:8px;margin-top:12px}.organizer-signature{display:flex;align-items:flex-start;gap:9px;line-height:1.5;padding:10px;border:1px solid #c89143;border-radius:7px}.organizer-signature input{margin-top:3px;flex-shrink:0}.organizer-error{color:#b52c37;font-size:12px;line-height:1.5;margin:0;width:100%}.organizer-load-error{padding:8px 26px}.organizer-footer{display:flex;align-items:center;gap:18px;flex-wrap:wrap;padding:12px 26px;border-top:1px solid var(--border);font-size:11px}.organizer-pagination{display:flex;align-items:center;gap:5px;white-space:nowrap}.organizer-notice{flex:1;min-width:140px;margin:0;color:var(--muted);line-height:1.5}.organizer-order-actions{display:flex;gap:8px}.page-organizer .icon-btn{color:var(--muted)}
@media(max-width:700px){.page-organizer{width:calc(100vw - 12px);height:calc(100dvh - 12px);border-radius:10px}.organizer-heading,.organizer-selection,.organizer-toolbar,.organizer-grid,.organizer-footer,.organizer-confirmation{padding-left:14px;padding-right:14px}.organizer-description{padding-inline:14px}.organizer-ranges{margin-left:0;flex-wrap:wrap}.organizer-grid{grid-template-columns:repeat(auto-fill,minmax(155px,1fr));gap:10px}.organizer-hint{font-size:10px}.organizer-heading h2{font-size:20px}.organizer-document{max-width:65vw}.organizer-selection-actions{gap:7px}}
</style>
