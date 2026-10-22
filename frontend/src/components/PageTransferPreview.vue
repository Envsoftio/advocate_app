<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { ArrowRight, ChevronLeft, ChevronRight, FilePlus2, Files, Replace } from 'lucide-vue-next'
import type { PDFDocumentProxy } from 'pdfjs-dist'
import type { MatterDocument } from '../lib/types'
import { pdfDocumentSource } from '../lib/api'
import { usePdfDocument } from '../lib/pdfReader'
import { friendlyError } from '../lib/format'
import PdfPageSurface from './PdfPageSurface.vue'

const props = defineProps<{
  document: MatterDocument
  source: MatterDocument
  operation: 'insert' | 'append' | 'replace' | 'replaceDocument'
  sourcePages: number[]
  targetPages: number[]
  beforeIndex: number
  disabled?: boolean
}>()
const emit = defineEmits<{ ready: [value: boolean] }>()
const targetOwner = usePdfDocument()
const sourceOwner = usePdfDocument()
const selected = ref(0)
const previewNumber = ref(1)
const previewError = ref('')
const scales = ref<Record<string, number>>({})
const painted = ref(new Set<string>())
const flow = ref<HTMLDivElement | null>(null)
const flowWidth = ref(620)
const sameDocument = computed(() => props.document.id === props.source.id && props.document.revision === props.source.revision)
const sourcePdf = computed(() => sameDocument.value ? targetOwner.pdf.value : sourceOwner.pdf.value)
const inserting = computed(() => props.operation === 'insert' || props.operation === 'append')
const resultCount = computed(() => inserting.value ? props.document.page_count + props.sourcePages.length
  : props.operation === 'replaceDocument' ? props.source.page_count : props.document.page_count)
const sourceSequence = computed(() => describeSequence(props.sourcePages))
const atStart = computed(() => props.beforeIndex === 0)
const atEnd = computed(() => props.beforeIndex === props.document.page_count)
const positionLabel = computed(() => {
  if (!inserting.value) return props.operation === 'replaceDocument' ? 'Replace the entire working PDF' : 'Replace selected working pages'
  if (atStart.value) return 'Insert at the beginning, before current page 1'
  if (atEnd.value) return `Append after current page ${props.document.page_count}`
  return `Insert between current pages ${props.beforeIndex} and ${props.beforeIndex + 1}`
})
type PreviewCard = { key: string, kind: 'target' | 'source' | 'boundary', page: number | null,
  label: string, description: string, resultPage: number | null, pdf: PDFDocumentProxy | null }
function card(kind: 'target' | 'source', page: number, label: string, resultPage: number): PreviewCard {
  const document = kind === 'target' ? props.document : props.source
  return { key: `${kind}:${document.id}:${document.revision}:${page}:${resultPage}`, kind, page, label,
    description: document.title, resultPage, pdf: kind === 'target' ? targetOwner.pdf.value : sourcePdf.value }
}
const cards = computed<PreviewCard[]>(() => {
  const sourceIndex = props.sourcePages[selected.value]
  if (sourceIndex === undefined) return []
  if (inserting.value) {
    const before: PreviewCard = atStart.value
      ? { key: 'start', kind: 'boundary', page: null, label: 'Start of document', description: 'The copied pages come first.', resultPage: null, pdf: null }
      : card('target', props.beforeIndex - 1, 'Immediately before', props.beforeIndex)
    const after: PreviewCard = atEnd.value
      ? { key: 'end', kind: 'boundary', page: null, label: 'End of document', description: 'The copied pages come last.', resultPage: null, pdf: null }
      : card('target', props.beforeIndex, 'Immediately after', props.beforeIndex + props.sourcePages.length + 1)
    return [before, card('source', sourceIndex, 'Pages to add', props.beforeIndex + selected.value + 1), after]
  }
  if (props.operation === 'replace') {
    const targetIndex = props.targetPages[selected.value]
    return [card('target', targetIndex, 'Current page to replace', targetIndex + 1),
      card('source', sourceIndex, 'Replacement page', targetIndex + 1)]
  }
  return [card('target', 0, 'Current working PDF · first page', 1),
    card('source', sourceIndex, 'New working PDF', selected.value + 1)]
})
const ready = computed(() => !!targetOwner.pdf.value && !!sourcePdf.value && cards.value.length > 0 &&
  !previewError.value && cards.value.every(item => item.page === null || painted.value.has(item.key)))
const error = computed(() => previewError.value || targetOwner.error.value || (!sameDocument.value && sourceOwner.error.value))
const replacementRows = computed(() => props.operation === 'replace'
  ? props.targetPages.map((index, position) => ({ target: index + 1, source: props.sourcePages[position] + 1 })) : [])
let sequence = 0
let observer: ResizeObserver | null = null

function describeSequence(indices: number[]) {
  const result: string[] = []
  for (let position = 0; position < indices.length; position++) {
    const first = indices[position]
    let last = first
    while (position + 1 < indices.length && indices[position + 1] === last + 1) last = indices[++position]
    result.push(first === last ? String(first + 1) : `${first + 1}–${last + 1}`)
  }
  return result.join(', ')
}
async function loadTarget() {
  const document = props.document
  await targetOwner.open(async () => ({ ...await pdfDocumentSource(document.id, document.revision),
    disableAutoFetch: true, disableStream: true }))
}
async function loadSource() {
  if (sameDocument.value) { sourceOwner.close(); return }
  const document = props.source
  await sourceOwner.open(async () => ({ ...await pdfDocumentSource(document.id, document.revision),
    disableAutoFetch: true, disableStream: true }))
}
function retry() { void loadTarget(); void loadSource() }
function choose(index: number) {
  selected.value = Math.max(0, Math.min(props.sourcePages.length - 1, index))
  previewNumber.value = selected.value + 1
}
function jump() {
  if (Number.isInteger(previewNumber.value)) choose(previewNumber.value - 1)
  else previewNumber.value = selected.value + 1
}
function markReady(key: string) { painted.value = new Set([...painted.value, key]) }
watch(() => `${props.document.id}:${props.document.revision}`, () => void loadTarget(), { immediate: true })
watch(() => `${props.source.id}:${props.source.revision}:${sameDocument.value}`, () => void loadSource(), { immediate: true })
watch(() => [props.operation, props.sourcePages, props.targetPages, props.beforeIndex], () => choose(0))
watch(() => [cards.value,flowWidth.value] as const, async ([items]) => {
  const call = ++sequence
  scales.value = {}
  painted.value = new Set()
  previewError.value = ''
  if (items.some(item => item.page !== null && !item.pdf)) return
  try {
    const sizes = await Promise.all(items.filter(item => item.page !== null).map(async item => {
      const page = await item.pdf!.getPage(item.page! + 1)
      const viewport = page.getViewport({ scale: 1 })
      const availableWidth = Math.min(168, Math.max(40,
        (flowWidth.value - (inserting.value ? 62 : 31)) / (inserting.value ? 3 : 2) - 16))
      return [item.key, Math.min(availableWidth / viewport.width, 208 / viewport.height)] as const
    }))
    if (call === sequence) scales.value = Object.fromEntries(sizes)
  } catch (cause) { if (call === sequence) previewError.value = friendlyError(cause) }
}, { immediate: true })
watch(ready, value => emit('ready', value), { immediate: true })
watch(flow, element => {
  observer?.disconnect()
  if (!element) return
  const measure = () => { flowWidth.value = element.clientWidth }
  observer = new ResizeObserver(measure)
  observer.observe(element)
  measure()
})
onBeforeUnmount(() => { sequence++; observer?.disconnect(); emit('ready', false) })
</script>

<template>
  <section class="page-transfer-preview" aria-label="Page transfer preview" :aria-busy="!ready && !error">
    <header><div><component :is="operation==='replace' || operation==='replaceDocument' ? Replace : operation==='append' ? Files : FilePlus2" :size="17"/><strong>Review page placement</strong></div><span>{{resultCount}} pages after applying</span></header>
    <p class="transfer-position">{{positionLabel}}</p>
    <div class="transfer-selection"><span>Source order: <strong>{{sourceSequence}}</strong></span><span>{{sourcePages.length}} {{sourcePages.length===1 ? 'page' : 'pages'}} from {{source.title}} · revision {{source.revision}}</span></div>
    <nav v-if="sourcePages.length>1" class="transfer-navigation" aria-label="Preview selected source pages"><button type="button" class="icon-btn small" aria-label="Previous selected source page" :disabled="disabled || selected===0" @click="choose(selected-1)"><ChevronLeft :size="16"/></button><label>Preview selection <input v-model.number="previewNumber" type="number" min="1" :max="sourcePages.length" :disabled="disabled" aria-label="Source selection preview" @change="jump" @keydown.enter.prevent="jump"/> of {{sourcePages.length}}</label><button type="button" class="icon-btn small" aria-label="Next selected source page" :disabled="disabled || selected>=sourcePages.length-1" @click="choose(selected+1)"><ChevronRight :size="16"/></button></nav>
    <div ref="flow" class="transfer-flow" :class="{'two-cards':!inserting}">
      <template v-for="(item,index) in cards" :key="item.key">
        <ArrowRight v-if="index>0" class="transfer-arrow" :size="17" aria-hidden="true"/>
        <article class="transfer-card" :class="[`transfer-${item.kind}`]" :aria-label="item.page===null ? item.label : `${item.label}: page ${item.page+1} of ${item.description}`">
          <strong class="transfer-card-heading">{{item.label}}</strong>
          <div class="transfer-thumbnail">
            <PdfPageSurface v-if="item.page!==null && item.pdf && scales[item.key]" :pdf="item.pdf" :page-number="item.page+1" :scale="scales[item.key]" :interactive="false" @ready="markReady(item.key)" @error="previewError=$event"/>
            <span v-else-if="item.page===null" class="transfer-boundary">{{item.description}}</span>
            <span v-else class="transfer-loading">Loading page…</span>
          </div>
          <template v-if="item.page!==null"><span class="transfer-page-label">{{item.kind==='source' ? 'Source' : 'Current'}} page {{item.page+1}}</span><span class="transfer-document" :title="item.description">{{item.description}}</span><span class="transfer-result">{{item.kind==='target' && !inserting ? 'Before applying' : `Result page ${item.resultPage}`}}</span></template>
        </article>
      </template>
    </div>
    <p v-if="inserting" class="transfer-outcome">Result order: <template v-if="beforeIndex>0">current pages {{beforeIndex===1 ? '1' : `1–${beforeIndex}`}} → </template>source pages {{sourceSequence}}<template v-if="beforeIndex<document.page_count"> → current pages {{beforeIndex+1===document.page_count ? document.page_count : `${beforeIndex+1}–${document.page_count}`}}</template>.</p>
    <details v-else-if="operation==='replace'" class="transfer-mapping"><summary>Review all {{replacementRows.length}} replacements in entered order</summary><ol><li v-for="(row,index) in replacementRows" :key="index">Current page {{row.target}} ← source page {{row.source}}. Result remains page {{row.target}}.</li></ol></details>
    <p v-else class="transfer-outcome">All {{document.page_count}} current working pages will be replaced by the {{source.page_count}} source pages, in source order.</p>
    <p v-if="error" class="transfer-error" role="alert">{{error}} <button type="button" class="btn secondary small" :disabled="disabled" @click="retry">Retry preview</button></p>
    <p class="transfer-footnote">Actual pages from current working revisions. Imported originals and the source document are retained.</p>
  </section>
</template>

<style scoped>
.page-transfer-preview .transfer-mapping>summary{display:list-item}
.page-transfer-preview{grid-column:1/-1;border:1px solid var(--border);border-radius:10px;background:var(--surface);padding:14px;min-width:0;color:var(--text)}.page-transfer-preview>header{display:flex;align-items:center;justify-content:space-between;gap:10px;padding:0!important}.page-transfer-preview>header>div{display:flex;align-items:center;gap:7px}.page-transfer-preview>header strong{font-size:12px}.page-transfer-preview>header>span{font-size:11px;color:var(--primary);white-space:nowrap}.transfer-position{font-size:12px;font-weight:700;margin:12px 0 8px}.transfer-selection{font-size:11px;color:var(--muted);display:grid;gap:5px;line-height:1.5}.transfer-selection>span:first-child{max-height:48px;overflow:auto;overflow-wrap:anywhere}.transfer-navigation{display:flex;gap:8px;align-items:center;justify-content:center;margin:12px 0 6px;font-size:11px}.transfer-navigation label{display:flex;align-items:center;gap:6px}.transfer-navigation input{width:54px;border:1px solid var(--border);border-radius:5px;background:var(--surface);color:var(--text);padding:5px}.transfer-flow{display:grid;grid-template-columns:minmax(0,1fr) 17px minmax(0,1fr) 17px minmax(0,1fr);gap:7px;align-items:center;margin-top:12px}.transfer-flow.two-cards{grid-template-columns:minmax(0,1fr) 17px minmax(0,1fr)}.transfer-card{display:flex;flex-direction:column;align-items:center;min-width:0;border:1px solid var(--border);border-radius:7px;background:var(--surface-soft);padding:9px 6px;gap:5px}.transfer-source{border-color:var(--primary);box-shadow:0 0 0 1px var(--primary)}.transfer-card-heading{font-size:10px;text-align:center;min-height:27px;display:flex;align-items:center;line-height:1.35}.transfer-thumbnail{height:212px;display:flex;align-items:center;justify-content:center;max-width:100%;width:100%;overflow:hidden}.transfer-thumbnail :deep(.pdf-page-surface){max-width:100%;max-height:208px;box-shadow:0 2px 6px #0002;pointer-events:none}.transfer-thumbnail :deep(.pdf-text-layer),.transfer-thumbnail :deep(.pdf-search-highlights){display:none}.transfer-loading,.transfer-boundary{font-size:11px;text-align:center;line-height:1.5;padding:12px;color:var(--muted)}.transfer-page-label{font-size:11px;font-weight:700}.transfer-document{max-width:100%;font-size:10px;color:var(--muted);white-space:nowrap;text-overflow:ellipsis;overflow:hidden}.transfer-result{font-size:10px;color:var(--primary);margin-top:2px}.transfer-arrow{color:var(--primary)}.transfer-outcome,.transfer-mapping,.transfer-footnote{font-size:11px;line-height:1.6;margin:12px 0 0;color:var(--muted);overflow-wrap:anywhere}.transfer-mapping summary{cursor:pointer;font-weight:700}.transfer-mapping ol{padding-left:22px;max-height:140px;overflow:auto}.transfer-footnote{font-size:10px}.transfer-error{color:#b52c37;font-size:12px;line-height:1.5}.transfer-boundary{border:1px dashed var(--border);border-radius:7px;width:100%;height:100%;display:flex;align-items:center;justify-content:center}
@media(max-width:740px){.page-transfer-preview>header{flex-wrap:wrap}.transfer-flow{gap:4px}.transfer-card{padding-inline:3px}.transfer-card-heading{font-size:9px}}
</style>
