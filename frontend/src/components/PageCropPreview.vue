<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue'
import PdfPageSurface from './PdfPageSurface.vue'
import { pdfDocumentSource } from '../lib/api'
import { usePdfDocument } from '../lib/pdfReader'
import { friendlyError } from '../lib/format'
import { cropToDisplayedPercent, displayedToCropPercent, validPdfCrop, type PdfCropPercent } from '../lib/pdfCrop'
import type { MatterDocument } from '../lib/types'

const props = defineProps<{ document: MatterDocument; pages: number[]; modelValue: PdfCropPercent; disabled?: boolean }>()
const emit = defineEmits<{ 'update:modelValue': [crop: PdfCropPercent]; ready: [ready: boolean] }>()
const reader = usePdfDocument()
const { pdf, loading, error } = reader
const sourceIdentity = computed(() => `${props.document.id}:${props.document.revision}`)
const loadedIdentity = ref('')
const pageNumber = ref(1)
const stage = ref<HTMLDivElement | null>(null)
const stageWidth = ref(480)
const geometry = shallowRef<{ page: number; width: number; height: number; rotation: number } | null>(null)
const pageError = ref('')
const paintedIdentity = ref('')
const surfaceBusy = ref(false)
const drag = shallowRef<{ pointerId: number; start: [number, number]; end: [number, number] } | null>(null)
let metadataGeneration = 0
let sourceGeneration = 0
let resizeObserver: ResizeObserver | null = null

const scale = computed(() => geometry.value
  ? Math.max(.02, Math.min((Math.max(80, stageWidth.value - 32)) / geometry.value.width, 420 / geometry.value.height, 1.5)) : 1)
const previewIdentity = computed(() => `${sourceIdentity.value}:${pageNumber.value}:${scale.value}`)
const canShow = computed(() => !!pdf.value && loadedIdentity.value === sourceIdentity.value
  && props.pages.includes(pageNumber.value) && geometry.value?.page === pageNumber.value)
const ready = computed(() => canShow.value && !loading.value && !surfaceBusy.value && !pageError.value
  && paintedIdentity.value === previewIdentity.value)
const canDraw = computed(() => ready.value && !props.disabled)
const displayedCrop = computed<PdfCropPercent | null>(() => {
  if (drag.value) {
    const { start, end } = drag.value
    return { left: Math.min(start[0], end[0]), top: Math.min(start[1], end[1]),
      right: Math.max(start[0], end[0]), bottom: Math.max(start[1], end[1]) }
  }
  return validPdfCrop(props.modelValue) ? cropToDisplayedPercent(props.modelValue, geometry.value?.rotation || 0) : null
})
const keepStyle = computed(() => {
  const crop = displayedCrop.value
  return crop ? { left: `${crop.left}%`, top: `${crop.top}%`, width: `${crop.right - crop.left}%`, height: `${crop.bottom - crop.top}%` } : {}
})

watch(() => props.pages, pages => {
  if (!pages.includes(pageNumber.value)) pageNumber.value = pages[0] || 1
}, { immediate: true })
watch(sourceIdentity, async identity => {
  const call = ++sourceGeneration
  loadedIdentity.value = ''; paintedIdentity.value = ''; geometry.value = null; pageError.value = ''; drag.value = null
  const id = props.document.id, revision = props.document.revision
  const opened = await reader.open(() => pdfDocumentSource(id, revision))
  if (call === sourceGeneration && opened) loadedIdentity.value = identity
}, { immediate: true, flush: 'sync' })
watch([pdf, pageNumber], async ([document, page]) => {
  const call = ++metadataGeneration
  geometry.value = null; paintedIdentity.value = ''; pageError.value = ''; drag.value = null
  if (!document || page < 1 || page > document.numPages) return
  try {
    const sourcePage = await document.getPage(page)
    if (call !== metadataGeneration || document !== pdf.value) return
    const viewport = sourcePage.getViewport({ scale: 1 })
    geometry.value = { page, width: viewport.width, height: viewport.height, rotation: viewport.rotation }
  } catch (err) { if (call === metadataGeneration) pageError.value = friendlyError(err) }
}, { immediate: true, flush: 'sync' })
watch(previewIdentity, () => { paintedIdentity.value = ''; drag.value = null }, { flush: 'sync' })
watch(() => props.disabled, disabled => { if (disabled) drag.value = null })
watch(ready, value => emit('ready', value), { immediate: true, flush: 'sync' })

function surfaceReady(value: { page: number; scale: number }) {
  if (canShow.value && value.page === pageNumber.value && value.scale === scale.value) paintedIdentity.value = previewIdentity.value
}
function surfaceFailed(message: string) { pageError.value = message; paintedIdentity.value = '' }
function pointerPosition(event: PointerEvent): [number, number] | null {
  const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect()
  if (!bounds.width || !bounds.height) return null
  return [Math.min(100, Math.max(0, (event.clientX - bounds.left) / bounds.width * 100)),
    Math.min(100, Math.max(0, (event.clientY - bounds.top) / bounds.height * 100))]
}
function startDrag(event: PointerEvent) {
  if (!canDraw.value || event.button !== 0) return
  const point = pointerPosition(event)
  if (!point) return
  event.preventDefault()
  const element = event.currentTarget as HTMLElement
  element.focus({ preventScroll: true }); element.setPointerCapture(event.pointerId)
  drag.value = { pointerId: event.pointerId, start: point, end: point }
}
function moveDrag(event: PointerEvent) {
  if (!canDraw.value || !drag.value || event.pointerId !== drag.value.pointerId) return
  const point = pointerPosition(event)
  if (point) drag.value = { ...drag.value, end: point }
}
function endDrag(event: PointerEvent) {
  if (!drag.value || event.pointerId !== drag.value.pointerId) return
  moveDrag(event)
  const crop = displayedCrop.value
  const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect()
  if (canDraw.value && crop && (crop.right - crop.left) * bounds.width / 100 >= 4
      && (crop.bottom - crop.top) * bounds.height / 100 >= 4) {
    const normalized = displayedToCropPercent(crop, geometry.value?.rotation || 0)
    const rounded = Object.fromEntries(Object.entries(normalized).map(([key, value]) => [key, Math.round(value * 100) / 100])) as unknown as PdfCropPercent
    if (validPdfCrop(rounded)) emit('update:modelValue', rounded)
  }
  cancelDrag(event)
}
function cancelDrag(event?: PointerEvent) {
  if (event) {
    const element = event.currentTarget as HTMLElement
    if (element.hasPointerCapture(event.pointerId)) element.releasePointerCapture(event.pointerId)
  }
  drag.value = null
}
onMounted(() => {
  resizeObserver = new ResizeObserver(entries => { stageWidth.value = entries[0].contentRect.width })
  if (stage.value) { stageWidth.value = stage.value.clientWidth; resizeObserver.observe(stage.value) }
})
onBeforeUnmount(() => { sourceGeneration++; metadataGeneration++; resizeObserver?.disconnect(); emit('ready', false) })
</script>

<template>
  <section class="page-crop-preview" aria-label="Crop preview on the working PDF">
    <header class="crop-preview-header">
      <label>Preview source page
        <select v-model.number="pageNumber" :disabled="disabled || !pages.length" aria-label="Preview crop on source page">
          <option v-for="page in pages" :key="page" :value="page">Page {{ page }}</option>
        </select>
      </label>
      <span>{{ pages.length }} selected {{ pages.length === 1 ? 'page' : 'pages' }} · revision {{ document.revision }}</span>
    </header>
    <p class="crop-preview-help">Drag a rectangle to choose the area to keep, or set the percentages below. Shaded content falls outside the visible page.</p>
    <div ref="stage" class="crop-preview-stage" :aria-busy="loading || surfaceBusy">
      <p v-if="!pages.length" role="status">Enter a valid page selection to preview the crop.</p>
      <p v-else-if="error || pageError" class="crop-preview-error" role="alert">{{ error || pageError }}</p>
      <template v-else>
        <p v-if="!ready" class="crop-loading" role="status">Loading source page {{ pageNumber }}…</p>
        <PdfPageSurface v-if="canShow" :key="`${sourceIdentity}:${pageNumber}`" :pdf="pdf" :page-number="pageNumber"
          :scale="scale" :interactive="false" @ready="surfaceReady" @busy="surfaceBusy=$event" @error="surfaceFailed">
          <div class="crop-drawing-layer" :class="{'can-draw':canDraw}" tabindex="0" role="group"
            :aria-label="`Crop area on source page ${pageNumber}. Drag to select; use the edge percentage fields for keyboard adjustment. Escape cancels a drag.`"
            :aria-disabled="!canDraw" @pointerdown="startDrag" @pointermove="moveDrag" @pointerup="endDrag"
            @pointercancel="cancelDrag" @lostpointercapture="drag=null" @keydown.esc.prevent="cancelDrag()">
            <div v-if="displayedCrop" class="crop-keep-area" :style="keepStyle"><span>Keep</span></div>
          </div>
        </PdfPageSurface>
      </template>
    </div>
    <p class="crop-preview-help">The same edge percentages apply to every selected page. Review each page above before applying.</p>
    <p v-if="geometry?.rotation" class="crop-rotation-note">This page is rotated {{ geometry.rotation }}°. The percentage fields use its unrotated edges; this preview shows the resulting visible area.</p>
  </section>
</template>

<style scoped>
.page-crop-preview{border:1px solid var(--border);border-radius:9px;overflow:hidden;background:var(--panel)}
.crop-preview-header{display:flex;align-items:center;justify-content:space-between;gap:12px;padding:12px 14px;background:var(--surface-soft);flex-wrap:wrap}
.crop-preview-header label{display:flex;align-items:center;gap:10px;font-size:12px;font-weight:600;color:var(--text)}
.crop-preview-header select{min-height:36px;padding:5px 9px;border:1px solid var(--border);border-radius:6px;background:var(--panel);color:var(--text)}
.crop-preview-header>span{font-size:11px;color:var(--muted)}
.crop-preview-help,.crop-rotation-note{font-size:11px;line-height:1.6;margin:10px 14px;color:var(--muted)}
.crop-rotation-note{color:var(--text)}
.crop-preview-stage{position:relative;display:flex;align-items:center;justify-content:center;min-height:180px;padding:16px;overflow:hidden;background:var(--surface-soft)}
.crop-preview-stage>p{font-size:12px;color:var(--muted);margin:0;line-height:1.6}
.crop-loading{position:absolute;left:12px;top:10px;z-index:4;background:var(--panel);padding:5px 9px;border-radius:5px}
.crop-preview-stage>.crop-preview-error{color:var(--danger,#c03935)}
.crop-drawing-layer{position:absolute;inset:0;z-index:3;overflow:hidden;pointer-events:auto;user-select:none;touch-action:none;line-height:1.2;cursor:wait}
.crop-drawing-layer.can-draw{cursor:crosshair}
.crop-drawing-layer:focus-visible{outline:3px solid #2876cb;outline-offset:-3px}
.crop-keep-area{position:absolute;border:2px solid #2876cb;box-shadow:0 0 0 10000px #182b4970;pointer-events:none;min-width:0;min-height:0}
.crop-keep-area>span{position:absolute;top:3px;left:3px;padding:3px 6px;color:#fff;background:#2464a9;border-radius:3px;font-size:10px}
</style>
