<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { FilePlus2, Files, Replace, Crop, Download, Scissors, Minimize2, ArrowRight } from 'lucide-vue-next'
import { api, ApiError, downloadPageOutput } from '../lib/api'
import { friendlyError } from '../lib/format'
import { confirmSignatureTransformation } from '../lib/signatures'
import type { Job, MatterDocument } from '../lib/types'
import PageCropPreview from './PageCropPreview.vue'
import PageTransferPreview from './PageTransferPreview.vue'

const props = defineProps<{ document: MatterDocument, documents: MatterDocument[], disabled: boolean, expanded?: boolean, job?: Job | null }>()
const emit = defineEmits<{ modified: [], jobStarted: [job: Job], cancel: [] }>()
const tasks = [
  {id:'insert', title:'Add pages', description:'Copy selected pages into this PDF.', detail:'Choose a document already in this matter, then choose where its pages should go.', icon:FilePlus2},
  {id:'append', title:'Combine PDFs', description:'Add another PDF at the end.', detail:'Copy every page from another document onto the end of this working PDF. The source document stays in the matter.', icon:Files},
  {id:'replace', title:'Replace pages', description:'Swap selected pages for corrected ones.', detail:'Choose the same number of source and target pages. They are replaced in the order you enter them.', icon:Replace},
  {id:'replaceDocument', title:'Replace whole PDF', description:'Use another document as this working copy.', detail:'Every working page in this document will be replaced. Its matter details and imported original stay unchanged.', icon:Replace},
  {id:'crop', title:'Crop margins', description:'Keep a smaller area of the page.', detail:'Preview the area to keep on the actual PDF. Edge percentages start at the unrotated page’s top-left corner. Cropping changes the visible page area; it does not securely remove hidden content.', icon:Crop},
  {id:'extract', title:'Save selected pages', description:'Create a separate PDF from a selection.', detail:'Choose the pages to copy into a new PDF. This document and the matter remain unchanged.', icon:Download},
  {id:'split', title:'Split into PDFs', description:'Divide a PDF into separate files.', detail:'Enter the pages after which to start a new file. The resulting PDFs are saved together in a ZIP; this document stays unchanged.', icon:Scissors},
  {id:'compress', title:'Reduce file size', description:'Clean up or compress this working PDF.', detail:'Choose how much image quality to retain. A smaller result becomes the working copy; a larger result is discarded.', icon:Minimize2},
] as const
const task = ref<(typeof tasks)[number]['id']>('insert')
const selectedTask = computed(() => tasks.find(item => item.id === task.value)!)
const insertionSide = ref('before')
const notice = ref('')
const taskContent = ref<HTMLElement | null>(null)
const needsSource = computed(() => ['insert','append','replace','replaceDocument'].includes(task.value))
const savesCopy = computed(() => task.value === 'extract' || task.value === 'split')
watch(task, async () => { error.value = ''; notice.value = ''; await nextTick(); taskContent.value?.scrollTo({top:0}) })
const compressionPreset = ref('lossless')
const compressionJob = computed(() => props.job?.kind === 'compress' && props.job.target_id === props.document.id ? props.job : null)
const compressionResult = computed(() => compressionJob.value?.result as Record<string, unknown> | null)
async function compress() {
  if (props.disabled || working.value) return
  const signature = confirmSignatureTransformation([props.document], 'Compression')
  if (!signature.proceed) return
  working.value = true
  error.value = ''
  notice.value = ''
  try {
    const job = await api<Job>(`/documents/${props.document.id}/compress`, 'POST', {
      preset: compressionPreset.value, expected_revision: props.document.revision,
      acknowledge_signature_risk: signature.acknowledged,
    })
    emit('jobStarted', job)
  } catch (err) { error.value = friendlyError(err) }
  finally { working.value = false }
}
const sourceId = ref('')
const sourceRange = ref('1')
const targetRange = ref('1')
const beforePage = ref(1)
const afterPage = ref(1)
const cropRange = ref('1')
const extractRange = ref('1')
const splitAfter = ref('1')
const crop = ref({ left: 0, top: 0, right: 100, bottom: 100 })
const cropPreviewReady = ref(false)
const cropPages = computed(() => {
  try { return [...new Set(parsePages(cropRange.value, props.document.page_count))].map(index => index + 1) }
  catch { return [] }
})
const working = ref(false)
const error = ref('')
const source = computed(() => props.documents.find(item => item.id === sourceId.value))
const transferPreviewReady = ref(false)
const transferPreview = computed(() => {
  if (!source.value || !needsSource.value) return null
  try {
    const operation = task.value as 'insert' | 'append' | 'replace' | 'replaceDocument'
    const sourcePages = operation === 'append' || operation === 'replaceDocument'
      ? Array.from({length:source.value.page_count}, (_,index)=>index)
      : parsePages(sourceRange.value, source.value.page_count)
    const targetPages = operation === 'replace' ? parsePages(targetRange.value, props.document.page_count) : []
    if (operation === 'replace' && (sourcePages.length !== targetPages.length || new Set(targetPages).size !== targetPages.length)) return null
    if (operation === 'replaceDocument' && source.value.id === props.document.id) return null
    if (operation === 'insert') {
      const position = insertionSide.value === 'before' ? beforePage.value : afterPage.value
      const maximum = insertionSide.value === 'before' ? props.document.page_count + 1 : props.document.page_count
      if (!Number.isInteger(position) || position < 1 || position > maximum) return null
    }
    const beforeIndex = operation === 'append' ? props.document.page_count
      : insertionSide.value === 'before' ? beforePage.value - 1 : afterPage.value
    if ((operation === 'insert' || operation === 'append') && (!Number.isInteger(beforeIndex) || beforeIndex < 0 || beforeIndex > props.document.page_count)) return null
    return {operation, sourcePages, targetPages, beforeIndex}
  } catch { return null }
})
watch(() => [props.document.id, props.document.revision, source.value?.id, source.value?.revision,
  task.value, sourceRange.value, targetRange.value, insertionSide.value, beforePage.value, afterPage.value],
  () => {transferPreviewReady.value=false})
watch(() => props.document.id, () => {
  task.value = 'insert'
  insertionSide.value = 'before'
  notice.value = ''
  crop.value = {left:0,top:0,right:100,bottom:100}
  sourceId.value = ''
  sourceRange.value = '1'
  targetRange.value = '1'
  beforePage.value = 1
  afterPage.value = 1
  cropRange.value = '1'
  extractRange.value = '1'
  splitAfter.value = '1'
  error.value = ''
}, { immediate: true })

function parsePages(value: string, count: number) {
  const result: number[] = []
  for (const piece of value.split(',')) {
    const match = /^(\d+)(?:\s*-\s*(\d+))?$/.exec(piece.trim())
    if (!match) throw new Error('Use page numbers such as 1,3-5.')
    const first = Number(match[1]), last = Number(match[2] || match[1])
    if (first < 1 || last > count || first > last || result.length + last - first + 1 > 2000)
      throw new Error(`Select pages from 1 to ${count}.`)
    for (let page = first; page <= last; page++) result.push(page - 1)
  }
  return result
}
async function run(action: 'insert' | 'insertAfter' | 'replace' | 'replaceDocument' | 'crop' | 'append') {
  if (props.disabled || working.value) return
  error.value = ''
  notice.value = ''
  working.value = true
  try {
    const involved = action === 'crop' || !source.value
      ? [props.document] : [props.document, source.value]
    const signature = confirmSignatureTransformation(involved,
      action === 'crop' ? 'Cropping' : action === 'replaceDocument' ? 'Replacing the working document' :
        action === 'replace' ? 'Replacing pages' : 'Inserting pages')
    if (!signature.proceed) return
    let payload: unknown
    let operation = action
    if (action === 'crop') {
      payload = { pages: parsePages(cropRange.value, props.document.page_count),
        left: crop.value.left / 100, top: crop.value.top / 100,
        right: crop.value.right / 100, bottom: crop.value.bottom / 100,
        expected_revision: props.document.revision,
        acknowledge_signature_risk: signature.acknowledged }
    } else {
      if (!source.value) throw new Error('Choose a source document in this matter.')
      if (action === 'replaceDocument' && source.value.id === props.document.id)
        throw new Error('Choose a different document as the replacement source.')
      const sourcePages = action === 'append'
        ? Array.from({ length: source.value.page_count }, (_, index) => index)
        : action === 'replaceDocument' ? [] : parsePages(sourceRange.value, source.value.page_count)
      if (action === 'replaceDocument') {
        payload = { source_document_id: source.value.id,
          expected_revision: props.document.revision, expected_source_revision: source.value.revision,
          acknowledge_signature_risk: signature.acknowledged }
      } else if (action === 'replace') {
        payload = { source_document_id: source.value.id, source_pages: sourcePages,
          target_pages: parsePages(targetRange.value, props.document.page_count),
          expected_revision: props.document.revision, expected_source_revision: source.value.revision,
          acknowledge_signature_risk: signature.acknowledged }
      } else {
        operation = 'insert'
        if (action === 'insertAfter' && (!Number.isInteger(afterPage.value) || afterPage.value < 1 || afterPage.value > props.document.page_count))
          throw new Error(`Insert after page 1 to ${props.document.page_count}.`)
        const before = action === 'append' ? props.document.page_count + 1
          : action === 'insertAfter' ? afterPage.value + 1 : beforePage.value
        if (!Number.isInteger(before) || before < 1 || before > props.document.page_count + 1)
          throw new Error(`Insert before page 1 to ${props.document.page_count + 1}.`)
        payload = { source_document_id: source.value.id, source_pages: sourcePages,
          expected_revision: props.document.revision, expected_source_revision: source.value.revision,
          before_index: before - 1, acknowledge_signature_risk: signature.acknowledged }
      }
    }
    await api(action === 'replaceDocument' ? `/documents/${props.document.id}/replace`
      : `/documents/${props.document.id}/pages/${operation}`, 'POST', payload)
    notice.value = 'Working copy updated. Review the PDF; use matter Undo if needed.'
    emit('modified')
  } catch (err) {
    error.value = friendlyError(err)
    if (err instanceof ApiError && err.status === 409 && /revision changed/i.test(err.message)) {
      transferPreviewReady.value = false
      error.value += ' Refreshing the preview; review it before trying again.'
      emit('modified')
    }
  } finally { working.value = false }
}
async function savePages(operation: 'extract' | 'split') {
  if (props.disabled || working.value) return
  error.value = ''
  notice.value = ''
  working.value = true
  try {
    const signature = confirmSignatureTransformation([props.document],
      operation === 'extract' ? 'Creating an extracted PDF' : 'Creating split PDFs')
    if (!signature.proceed) return
    const indexes = operation === 'extract'
      ? parsePages(extractRange.value, props.document.page_count)
      : parsePages(splitAfter.value, props.document.page_count - 1)
    const saved = await downloadPageOutput(props.document.id, operation, indexes, signature.acknowledged)
    if (saved !== false) notice.value = 'Separate file prepared. The document in this matter is unchanged.'
  } catch (err) { error.value = friendlyError(err) }
  finally { working.value = false }
}
const inputSelection = computed(() => {
  try {
    const count = props.document.page_count
    if (needsSource.value && !source.value) return {valid:false, message:'Choose the document to copy pages from.'}
    if (task.value === 'insert' || task.value === 'replace') {
      const pages = parsePages(sourceRange.value, source.value!.page_count)
      if (task.value === 'replace') {
        const targets = parsePages(targetRange.value, count)
        if (new Set(targets).size !== targets.length) throw new Error('Each target page can only be replaced once.')
        if (pages.length !== targets.length) throw new Error('Choose the same number of source and target pages.')
        return {valid:true, message:`Replace ${targets.length} page(s) in “${props.document.title}” with pages ${sourceRange.value} from “${source.value!.title}”.`}
      }
      const position = insertionSide.value === 'before' ? beforePage.value : afterPage.value
      const maximum = insertionSide.value === 'before' ? count + 1 : count
      if (!Number.isInteger(position) || position < 1 || position > maximum) throw new Error(`Choose a position from 1 to ${maximum}.`)
      return {valid:true, message:`Copy ${pages.length} page(s) from “${source.value!.title}” ${insertionSide.value} page ${position}. This PDF will have ${count + pages.length} pages.`}
    }
    if (task.value === 'append') {
      if (source.value!.page_count > 2000) throw new Error('Combine supports up to 2,000 source pages per action. Use Add pages for separate ranges.')
      return {valid:true, message:`Add all ${source.value!.page_count} pages of “${source.value!.title}” at the end. This PDF will have ${count + source.value!.page_count} pages.`}
    }
    if (task.value === 'replaceDocument') {
      if (sourceId.value === props.document.id) throw new Error('Choose a different document as the replacement.')
      return {valid:true, message:`Replace all ${count} working pages with the ${source.value!.page_count} pages of “${source.value!.title}”.`}
    }
    if (task.value === 'crop') {
      const pages = parsePages(cropRange.value, count)
      const {left,top,right,bottom} = crop.value
      if (![left,top,right,bottom].every(value => Number.isFinite(value) && value >= 0 && value <= 100) || left >= right || top >= bottom) throw new Error('The right edge must be after the left edge, and the bottom edge below the top edge (0–100%).')
      if (!cropPreviewReady.value) return {valid:false, message:'Wait for the selected source page to render before applying its crop.'}
      return {valid:true, message:`Crop ${pages.length} page(s), keeping ${right-left}% of the width and ${bottom-top}% of the height.`}
    }
    if (task.value === 'extract') {
      const pages = parsePages(extractRange.value, count)
      return {valid:true, message:`Save a new PDF containing ${pages.length} page(s), in the order entered.`}
    }
    if (task.value === 'split') {
      if (count < 2) throw new Error('This PDF has only one page. Choose a document with at least two pages.')
      const pages = parsePages(splitAfter.value, count-1)
      if (pages.some((page,index) => index > 0 && page <= pages[index-1])) throw new Error('Enter split points in ascending order, without duplicates (for example, 2,5).')
      return {valid:true, message:`Split after page(s) ${[...new Set(pages)].sort((a,b)=>a-b).map(p=>p+1).join(', ')} and save the PDFs in a ZIP.`}
    }
    return {valid:true, message:compressionPreset.value === 'lossless' ? 'Clean up this PDF while retaining image quality. The file may not become smaller.' : 'Compress images in this working PDF. Review small text and seals afterward.'}
  } catch (err) { return {valid:false, message:friendlyError(err)} }
})
const selection = computed(() => {
  if (!inputSelection.value.valid || !needsSource.value || transferPreviewReady.value) return inputSelection.value
  return {valid:false, message:'Wait for the actual source and destination pages to render before applying this change.'}
})
const actionLabel = computed(() => ({insert:'Add selected pages',append:'Combine documents',replace:'Replace selected pages',replaceDocument:'Replace entire working document',crop:'Crop selected pages',extract:'Save extracted PDF',split:'Save split ZIP',compress:'Compress working PDF'})[task.value])
function applyTask() {
  if (!selection.value.valid) return
  if (task.value === 'compress') void compress()
  else if (task.value === 'extract' || task.value === 'split') void savePages(task.value)
  else if (task.value === 'insert') void run(insertionSide.value === 'before' ? 'insert' : 'insertAfter')
  else void run(task.value)
}
</script>

<template>
  <section class="page-tools task-tools">
    <nav class="pdf-task-list" aria-label="Choose a PDF task">
      <p>What would you like to do?</p>
      <button v-for="item in tasks" :key="item.id" :aria-label="item.title" :aria-pressed="task===item.id" :disabled="working" @click="task=item.id">
        <component :is="item.icon" :size="18"/><span><strong>{{item.title}}</strong><small>{{item.description}}</small></span>
      </button>
    </nav>
    <div class="pdf-task-body">
      <div ref="taskContent" class="pdf-task-content">
      <header><span class="task-effect">{{savesCopy ? 'SAVES SEPARATE FILES' : 'UPDATES THE WORKING COPY'}}</span><h3>{{selectedTask.title}}</h3><p>{{selectedTask.detail}}</p></header>
      <div class="task-document"><strong>{{document.title}}</strong><span>{{document.page_count}} pages · Imported original preserved</span></div>
      <p v-if="!savesCopy" class="task-guidance">Changes can be reversed with matter Undo. Review the result before building a bundle.</p>
      <p v-else class="task-guidance">Review links and annotations in saved files before sharing. The bundle’s omission policy does not apply to these files.</p>
      <p v-if="task!=='compress'" class="range-help">Use the document’s page numbers, starting at 1—not printed bundle numbers. For example, <strong>1,3-5</strong> selects pages 1, 3, 4 and 5.</p>
      <div class="task-fields">
        <label v-if="needsSource" class="field">Source document
          <select v-model="sourceId" :disabled="disabled || working"><option value="">Choose a document to copy from</option><option v-for="item in documents" :key="item.id" :value="item.id">{{item.title}} ({{item.page_count}} pages){{item.id===document.id ? ' · current document' : ''}}</option></select>
        </label>
        <label v-if="task==='insert' || task==='replace'" class="field">Source pages<input v-model="sourceRange" placeholder="1,3-5" :disabled="disabled || working"/><small v-if="source">Available: 1–{{source.page_count}} in {{source.title}}</small></label>
        <template v-if="task==='insert'">
          <label class="field">Where to add pages<select v-model="insertionSide" :disabled="disabled || working"><option value="before">Before a page</option><option value="after">After a page</option></select></label>
          <label v-if="insertionSide==='before'" class="field">Insert before page<input v-model.number="beforePage" type="number" min="1" :max="document.page_count+1" :disabled="disabled || working"/></label>
          <label v-else class="field">Insert after page<input v-model.number="afterPage" type="number" min="1" :max="document.page_count" :disabled="disabled || working"/></label>
        </template>
        <label v-if="task==='replace'" class="field">Target pages to replace<input v-model="targetRange" placeholder="1,3-5" :disabled="disabled || working"/><small>Pages in {{document.title}} · Choose as many pages as the source selection.</small></label>
        <PageTransferPreview v-if="needsSource && source && transferPreview" :document="document" :source="source" v-bind="transferPreview" :disabled="disabled || working" @ready="transferPreviewReady=$event"/>
        <template v-if="task==='crop'">
          <label class="field">Pages to crop<input v-model="cropRange" placeholder="1,3-5" :disabled="disabled || working"/></label>
          <PageCropPreview v-model="crop" :document="document" :pages="cropPages" :disabled="disabled || working" @ready="cropPreviewReady=$event"/>
          <div class="crop-grid"><label v-for="edge in (['left','top','right','bottom'] as const)" :key="edge">{{edge}} %<input v-model.number="crop[edge]" type="number" min="0" max="100" :disabled="disabled || working"/></label></div>
        </template>
        <label v-if="task==='extract'" class="field">Pages to extract<input v-model="extractRange" placeholder="1,3-5" :disabled="disabled || working"/></label>
        <label v-if="task==='split'" class="field">Split after pages<input v-model="splitAfter" placeholder="2,5" :disabled="disabled || working"/><small>Example: 2,5 makes files containing pages 1–2, 3–5, and 6 onward.</small></label>
        <template v-if="task==='compress'"><label class="field">Compression quality<select v-model="compressionPreset" :disabled="disabled || working"><option value="lossless">Keep image quality — lossless cleanup</option><option value="balanced">Balanced — smaller scans, some quality loss</option><option value="small">Smallest — more quality loss</option></select></label><p class="task-guidance">Balanced uses 240 DPI / JPEG quality 85; Smallest uses 150 DPI / quality 70. Small text and seals may become less clear. A smaller size is not guaranteed.</p></template>
      </div>
      <div v-if="task==='compress' && compressionJob" role="status"><p>{{compressionJob.message}} · {{compressionJob.progress}}%</p><button v-if="['queued','running'].includes(compressionJob.status)" class="text-tool danger" @click="emit('cancel')">Cancel compression</button><p v-if="compressionJob.error" class="error-msg">{{compressionJob.error}}</p><p v-if="compressionJob.status==='completed' && compressionResult">{{compressionResult.before_bytes}} → {{compressionResult.after_bytes}} bytes. {{compressionResult.message}}</p></div>
      </div>
      <footer class="task-footer">      <div class="task-preview" :class="{'invalid':!selection.valid}" aria-live="polite"><strong>{{selection.valid ? 'What will happen' : 'Before you continue'}}</strong><p>{{selection.message}}</p></div>
      <p v-if="error" class="error-msg" role="alert">{{error}}</p><p v-if="notice" class="task-success" role="status">{{notice}}</p>
<div class="task-submit-row"><span>{{savesCopy ? 'Current document stays unchanged' : 'Original file stays unchanged'}}</span><button class="btn primary" :disabled="disabled || working || !selection.valid" @click="applyTask">{{working ? 'Working…' : actionLabel}}<ArrowRight :size="16"/></button></div></footer>
    </div>
  </section>
</template>
