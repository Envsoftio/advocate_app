<script setup lang="ts">
import { nextTick, onBeforeUnmount, ref, watch } from 'vue'
import * as pdfjs from 'pdfjs-dist'
import workerSrc from 'pdfjs-dist/build/pdf.worker.min.mjs?url'
import type { PDFDocumentProxy, RenderTask } from 'pdfjs-dist'
import { exportPdfBytes } from '../lib/api'
import { friendlyError } from '../lib/format'

pdfjs.GlobalWorkerOptions.workerSrc = workerSrc
const props = defineProps<{matterId:string,exportId:string,filename:string}>()
const emit = defineEmits<{close:[]}>()
const canvas = ref<HTMLCanvasElement | null>(null)
const canvasWrap = ref<HTMLDivElement | null>(null)
const loading = ref(true)
const error = ref('')
const pageNumber = ref(1)
const pageCount = ref(0)
const outline = ref<Array<{title:string,page:number,level:number}>>([])
let pdf: PDFDocumentProxy | null = null
let renderTask: RenderTask | null = null
let generation = 0

async function renderPage() {
  const currentPdf = pdf
  const element = canvas.value
  const wrap = canvasWrap.value
  if (!currentPdf || !element || !wrap) return
  if (renderTask) {
    renderTask.cancel()
    try { await renderTask.promise } catch { /* previous page cancelled */ }
    renderTask = null
  }
  const requestedPage = Math.min(Math.max(1, pageNumber.value), currentPdf.numPages)
  pageNumber.value = requestedPage
  try {
    const page = await currentPdf.getPage(requestedPage)
    if (pdf !== currentPdf) return
    const base = page.getViewport({scale:1})
    const viewport = page.getViewport({scale:Math.min(1.5, Math.max(.45, (wrap.clientWidth - 24) / base.width))})
    const pixelRatio = Math.min(window.devicePixelRatio || 1, 2)
    element.width = Math.round(viewport.width * pixelRatio)
    element.height = Math.round(viewport.height * pixelRatio)
    element.style.width = `${viewport.width}px`
    element.style.height = `${viewport.height}px`
    const context = element.getContext('2d')
    if (!context) return
    renderTask = page.render({canvasContext:context,viewport,
      transform:pixelRatio === 1 ? undefined : [pixelRatio,0,0,pixelRatio,0,0]})
    await renderTask.promise
  } catch (err) {
    if (!(err instanceof Error && err.name === 'RenderingCancelledException')) error.value = friendlyError(err)
  }
}

async function load() {
  const call = ++generation
  loading.value = true
  error.value = ''
  outline.value = []
  pageCount.value = 0
  pageNumber.value = 1
  if (renderTask) renderTask.cancel()
  if (pdf) { void pdf.destroy(); pdf = null }
  try {
    const bytes = await exportPdfBytes(props.matterId, props.exportId, props.filename)
    if (call !== generation) return
    const opened = await pdfjs.getDocument({data:bytes}).promise
    if (call !== generation) { void opened.destroy(); return }
    pdf = opened
    pageCount.value = opened.numPages
    const items = await opened.getOutline() || []
    const entries: Array<{title:string,page:number,level:number}> = []
    async function collect(nodes: typeof items, level: number) {
      for (const item of nodes) {
        try {
          const destination = typeof item.dest === 'string'
            ? await opened.getDestination(item.dest) : item.dest
          if (destination?.[0]) {
            const index = await opened.getPageIndex(destination[0])
            entries.push({title:item.title,page:index+1,level})
          }
        } catch { /* keep rendering even if one source outline target is invalid */ }
        if (item.items?.length) await collect(item.items, level+1)
      }
    }
    await collect(items, 0)
    if (call !== generation) return
    outline.value = entries
    loading.value = false
    await nextTick()
    await renderPage()
  } catch (err) {
    if (call === generation) {error.value = friendlyError(err);loading.value = false}
  }
}
function goToPage(page: number) {
  if (!Number.isInteger(page) || page < 1 || page > pageCount.value) return
  pageNumber.value = page
  void renderPage()
}
watch(() => [props.matterId,props.exportId,props.filename], () => {void load()}, {immediate:true})
onBeforeUnmount(() => {
  generation++
  if (renderTask) renderTask.cancel()
  if (pdf) void pdf.destroy()
})
</script>

<template>
  <div class="export-review-backdrop" @click.self="emit('close')">
    <section class="export-review" role="dialog" aria-modal="true" aria-label="Review generated PDF">
      <header><div><h2>Review {{filename}}</h2><p>Check the printed index, page numbers and PDF bookmarks. Test index hyperlinks in a saved PDF viewer.</p></div><button class="btn secondary small" @click="emit('close')">Close review</button></header>
      <p v-if="loading" role="status">Opening generated PDF…</p>
      <p v-if="error" class="error-msg" role="alert">{{error}}</p>
      <div v-if="!loading && !error" class="export-review-content">
        <aside><h3>PDF bookmarks</h3><p v-if="!outline.length">No bookmarks in this PDF.</p><button v-for="(item,index) in outline" :key="index" :style="{paddingLeft:`${8 + item.level * 13}px`}" @click="goToPage(item.page)">{{item.title}} <small>p{{item.page}}</small></button></aside>
        <div class="export-review-document"><div class="export-review-controls"><button class="btn secondary small" :disabled="pageNumber<=1" @click="goToPage(pageNumber-1)">Previous</button><label>PDF page <input :value="pageNumber" aria-label="PDF page number" type="number" min="1" :max="pageCount" @change="goToPage(Number(($event.target as HTMLInputElement).value))"/> / {{pageCount}}</label><button class="btn secondary small" :disabled="pageNumber>=pageCount" @click="goToPage(pageNumber+1)">Next</button></div><div ref="canvasWrap" class="export-review-canvas"><canvas ref="canvas" aria-label="Generated PDF page"></canvas></div></div>
      </div>
    </section>
  </div>
</template>
