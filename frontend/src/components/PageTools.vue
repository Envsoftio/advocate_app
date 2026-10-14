<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { api, downloadPageOutput } from '../lib/api'
import { friendlyError } from '../lib/format'
import type { Job, MatterDocument } from '../lib/types'

const props = defineProps<{ document: MatterDocument, documents: MatterDocument[], disabled: boolean, expanded?: boolean, job?: Job | null }>()
const emit = defineEmits<{ modified: [], jobStarted: [job: Job], cancel: [] }>()
const compressionPreset = ref('lossless')
const compressionJob = computed(() => props.job?.kind === 'compress' && props.job.target_id === props.document.id ? props.job : null)
const compressionResult = computed(() => compressionJob.value?.result as Record<string, unknown> | null)
async function compress() {
  if (props.disabled || working.value) return
  working.value = true
  error.value = ''
  try {
    const job = await api<Job>(`/documents/${props.document.id}/compress`, 'POST', {
      preset: compressionPreset.value, expected_revision: props.document.revision,
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
const working = ref(false)
const error = ref('')
const source = computed(() => props.documents.find(item => item.id === sourceId.value))
watch(() => props.document.id, () => {
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
  working.value = true
  try {
    let payload: unknown
    let operation = action
    if (action === 'crop') {
      payload = { pages: parsePages(cropRange.value, props.document.page_count),
        left: crop.value.left / 100, top: crop.value.top / 100,
        right: crop.value.right / 100, bottom: crop.value.bottom / 100 }
    } else {
      if (!source.value) throw new Error('Choose a source document in this matter.')
      if (action === 'replaceDocument' && source.value.id === props.document.id)
        throw new Error('Choose a different document as the replacement source.')
      const sourcePages = action === 'append'
        ? Array.from({ length: source.value.page_count }, (_, index) => index)
        : action === 'replaceDocument' ? [] : parsePages(sourceRange.value, source.value.page_count)
      if (action === 'replaceDocument') {
        payload = { source_document_id: source.value.id }
      } else if (action === 'replace') {
        payload = { source_document_id: source.value.id, source_pages: sourcePages,
          target_pages: parsePages(targetRange.value, props.document.page_count) }
      } else {
        operation = 'insert'
        if (action === 'insertAfter' && (!Number.isInteger(afterPage.value) || afterPage.value < 1 || afterPage.value > props.document.page_count))
          throw new Error(`Insert after page 1 to ${props.document.page_count}.`)
        const before = action === 'append' ? props.document.page_count + 1
          : action === 'insertAfter' ? afterPage.value + 1 : beforePage.value
        if (!Number.isInteger(before) || before < 1 || before > props.document.page_count + 1)
          throw new Error(`Insert before page 1 to ${props.document.page_count + 1}.`)
        payload = { source_document_id: source.value.id, source_pages: sourcePages, before_index: before - 1 }
      }
    }
    await api(action === 'replaceDocument' ? `/documents/${props.document.id}/replace`
      : `/documents/${props.document.id}/pages/${operation}`, 'POST', payload)
    emit('modified')
  } catch (err) { error.value = friendlyError(err) }
  finally { working.value = false }
}
async function savePages(operation: 'extract' | 'split') {
  if (props.disabled || working.value) return
  error.value = ''
  working.value = true
  try {
    const indexes = operation === 'extract'
      ? parsePages(extractRange.value, props.document.page_count)
      : parsePages(splitAfter.value, props.document.page_count - 1)
    await downloadPageOutput(props.document.id, operation, indexes)
  } catch (err) { error.value = friendlyError(err) }
  finally { working.value = false }
}
</script>

<template>
  <details class="page-tools" :open="expanded">
    <summary>Insert, merge, replace or crop pages</summary>
    <p>Changes create a new working-copy revision. Imported originals stay intact. Page numbers start at 1.</p>
    <label class="field">Source document
      <select v-model="sourceId" :disabled="disabled || working">
        <option value="">Choose document</option>
        <option v-for="item in documents" :key="item.id" :value="item.id">{{ item.title }} ({{ item.page_count }} pages)</option>
      </select>
    </label>
    <label class="field">Source pages <input v-model="sourceRange" placeholder="1,3-5" :disabled="disabled || working"/></label>
    <label class="field">Insert before page <input v-model.number="beforePage" type="number" min="1" :max="document.page_count + 1" :disabled="disabled || working"/></label>
    <label class="field">Or insert after page <input v-model.number="afterPage" type="number" min="1" :max="document.page_count" :disabled="disabled || working"/></label>
    <div class="page-tool-actions">
      <button class="btn secondary small" :disabled="disabled || working" @click="run('insert')">Insert selected pages</button>
      <button class="btn secondary small" :disabled="disabled || working" @click="run('insertAfter')">Insert after page</button>
      <button class="btn secondary small" :disabled="disabled || working" @click="run('append')">Merge: append all</button>
    </div>
    <label class="field">Target pages to replace <input v-model="targetRange" placeholder="1,3-5" :disabled="disabled || working"/></label>
    <button class="btn secondary small" :disabled="disabled || working" @click="run('replace')">Replace one-for-one</button>
    <p>Replace every page with the selected source document's current working pages. The imported original and prior revision remain available through Undo.</p>
    <button class="btn secondary small" :disabled="disabled || working || !sourceId || sourceId === document.id" @click="run('replaceDocument')">Replace entire working document</button>
    <div class="divider-line"></div>
    <label class="field">Pages to crop <input v-model="cropRange" placeholder="1,3-5" :disabled="disabled || working"/></label>
    <div class="crop-grid">
      <label v-for="edge in (['left', 'top', 'right', 'bottom'] as const)" :key="edge">{{edge}} %
        <input v-model.number="crop[edge]" type="number" min="0" max="100" :disabled="disabled || working"/>
      </label>
    </div>
    <button class="btn secondary small" :disabled="disabled || working" @click="run('crop')">Crop selected pages</button>
    <div class="divider-line"></div>
    <p>Extraction and splitting save separate files. They do not change this matter or its undo history. Review links and annotations in those files before sharing; the bundle's omission policy does not apply to them.</p>
    <label class="field">Pages to extract <input v-model="extractRange" placeholder="1,3-5" :disabled="disabled || working"/></label>
    <button class="btn secondary small" :disabled="disabled || working" @click="savePages('extract')">Save extracted PDF</button>
    <label class="field">Split after pages <input v-model="splitAfter" placeholder="2,5" :disabled="disabled || working"/></label>
    <button class="btn secondary small" :disabled="disabled || working" @click="savePages('split')">Save split ZIP</button>
    <div class="divider-line"></div>
    <label class="field">Compression quality
      <select v-model="compressionPreset" :disabled="disabled || working">
        <option value="lossless">Lossless cleanup — retain image quality</option>
        <option value="balanced">Balanced — 240 DPI, JPEG quality 85</option>
        <option value="small">Smaller — 150 DPI, JPEG quality 70</option>
      </select>
    </label>
    <p>Balanced and Smaller can reduce scan quality. Review small text and seals afterward; Undo restores the previous revision. A larger result is discarded.</p>
    <button class="btn secondary small" :disabled="disabled || working" @click="compress">Compress working PDF</button>
    <div v-if="compressionJob" role="status">
      <p>{{compressionJob.message}} · {{compressionJob.progress}}%</p>
      <button v-if="['queued', 'running'].includes(compressionJob.status)" class="text-tool danger" @click="emit('cancel')">Cancel compression</button>
      <p v-if="compressionJob.error" class="error-msg">{{compressionJob.error}}</p>
      <p v-if="compressionJob.status === 'completed' && compressionResult">
        {{compressionResult.before_bytes}} → {{compressionResult.after_bytes}} bytes.
        {{compressionResult.message}}
      </p>
    </div>
    <p v-if="error" class="error-msg" role="alert">{{error}}</p>
  </details>
</template>
