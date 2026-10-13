<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { ChevronLeft, ChevronRight, RotateCw, Trash2, FileSearch, LoaderCircle } from 'lucide-vue-next'
import * as pdfjs from 'pdfjs-dist'
import workerSrc from 'pdfjs-dist/build/pdf.worker.min.mjs?url'
import type { PDFDocumentProxy, RenderTask } from 'pdfjs-dist'
import type { MatterDocument } from '../lib/types'
import { api, pdfBytes } from '../lib/api'
import { friendlyError } from '../lib/format'

pdfjs.GlobalWorkerOptions.workerSrc = workerSrc
const props = defineProps<{ document: MatterDocument | null }>()
const emit = defineEmits<{ modified: [], error: [message: string] }>()
const canvas = ref<HTMLCanvasElement | null>(null)
const canvasWrap = ref<HTMLDivElement | null>(null)
const current = ref(1)
const loading = ref(false)
const editing = ref(false)
const viewerError = ref('')
const scalePct = ref(100)
const loadedPageCount = ref(0)
let pdf: PDFDocumentProxy | null = null
let renderTask: RenderTask | null = null
let epoch = 0
const total = computed(() => loadedPageCount.value || props.document?.page_count || 0)

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
  if (renderTask) { renderTask.cancel(); renderTask = null }
  if (pdf) { const previous = pdf; pdf = null; void previous.destroy() }
  if (!doc) { loading.value = false; return }
  try {
    const bytes = await pdfBytes(doc.id)
    if (call !== epoch) return
    pdf = await pdfjs.getDocument({ data: bytes }).promise
    loadedPageCount.value = pdf.numPages
    await nextTick()
    if (call !== epoch) return
    await render()
  } catch (err) { viewerError.value = friendlyError(err) }
  finally { if (call === epoch) loading.value = false }
}
watch(() => `${props.document?.id || ''}|${props.document?.updated_at || ''}`, () => void loadPdf(props.document), { immediate: true })
watch([current, scalePct], () => void render())
onBeforeUnmount(() => { epoch++; renderTask?.cancel(); if (pdf) void pdf.destroy() })

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
        <select v-model.number="scalePct" class="zoom-select" aria-label="Zoom"><option :value="75">75%</option><option :value="100">100%</option><option :value="125">125%</option><option :value="150">150%</option></select>
      </div>
      <div class="page-editbar">
        <button class="text-tool" :disabled="editing || loading" title="Rotate current page" @click="edit('rotate', {page_index:current-1,degrees:90})"><RotateCw :size="15"/> Rotate</button>
        <button class="text-tool" :disabled="editing || loading || current<=1" title="Move page left" @click="move(-1)"><ChevronLeft :size="15"/> Move</button>
        <button class="text-tool" :disabled="editing || loading || current>=total" title="Move page right" @click="move(1)">Move <ChevronRight :size="15"/></button>
        <button class="text-tool danger" :disabled="editing || loading || total<=1" title="Delete page from working copy" @click="deleteCurrent"><Trash2 :size="15"/> Delete</button>
      </div>
      <div ref="canvasWrap" class="canvas-scroll">
        <div v-if="loading || editing" class="preview-status"><LoaderCircle :size="18" class="spin"/> {{ editing ? 'Updating PDF…' : 'Loading PDF…' }}</div>
        <div v-if="viewerError" class="preview-status error-msg">{{ viewerError }}</div>
        <canvas ref="canvas" class="pdf-paper" :style="{visibility: viewerError ? 'hidden' : 'visible'}" />
      </div>
    </template>
    <div v-else class="viewer-empty"><FileSearch :size="44"/><strong>Document preview</strong><span>Select a file to review its pages</span></div>
  </div>
</template>
