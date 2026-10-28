<script setup lang="ts">
import { computed, onBeforeUnmount, reactive, ref, watch } from 'vue'
import MetadataSuggestions from './MetadataSuggestions.vue'
import { api } from '../lib/api'
import { friendlyError } from '../lib/format'
import type { DocKind, MatterDocument } from '../lib/types'

type SortMode = 'manual' | 'upload' | 'filename' | 'document_date'
type SortPreview = {
  mode: SortMode
  ids: string[]
  rows: Array<Pick<MatterDocument, 'id'|'title'|'original_name'|'document_date'|'date_status'|'date_source'|'date_note'|'group_key'> & {old_position:number,new_position:number}>
  unknown_dates: 'last'
  stable_ties: string
  group_policy: string
}
type Draft = Pick<MatterDocument, 'title'|'label'|'kind'|'part'|'group_key'|'group_role'|'document_date'> & {date_confirmed:boolean}
type LabelMode = 'sequential' | 'grouped_suffix' | 'filename'
type GroupSuffixStyle = 'lower_alpha'|'upper_alpha'|'numeric'|'lower_alpha_parenthesized'|'upper_alpha_parenthesized'|'numeric_parenthesized'
type LabelPreviewRow = {
  document_id:string, title:string, original_name:string, description:string, group_key:string,
  group_role:'member'|'parent',
  old_label:string, old_status:'unreviewed'|'confirmed', proposed_label:string,
  proposal_status:'suggested'|'needs_manual', reason:string
}
type ReferenceReview = {
  document_id:string, title:string, revision:number, searchable_pages:number, truncated:boolean,
  references:Array<{page:number,raw:string,label:string,status:'old_label'|'proposed_label'|'unmatched',document_id:string|null,snippet:string}>
}
type LabelPreview = {
  rows:LabelPreviewRow[], warnings:string[], blocking:string[], policy:string,
  reference_review:ReferenceReview|null,
  export_history:{has_prior_exports:boolean,prior_uses:Array<{id:string,created_at:string,labels:Array<{document_id:string,label:string}>}>,unknown_exports:Array<{id:string,created_at:string}>}
}

const props = defineProps<{matterId:string, documents:MatterDocument[], busy:boolean, keepGroups:boolean}>()
const emit = defineEmits<{
  modified: []
  openDocument: [id:string]
  error: [error:unknown]
  close: []
  dirtyChanged: [value:boolean]
}>()
const kinds: DocKind[] = ['petition','affidavit','order','annexure','application','evidence','other']
const query = ref('')
const suggestionsOpen=ref(false),suggestionsRef=ref<InstanceType<typeof MetadataSuggestions>|null>(null),suggestionsDirty=ref(false),reviewIds=ref<string[]>([])
const baseline=ref<MatterDocument[]>([]),snapshot=ref(''),loadError=ref(''),advanced=ref(false),compact=ref(true),showColumns=ref(false)
const hiddenColumns=ref<string[]>([])
let loadSequence=0,alive=true
onBeforeUnmount(()=>{alive=false;loadSequence++})
const dirtyRows=computed(()=>baseline.value.filter(rowDirty))
const dirty=computed(()=>!!dirtyRows.value.length||suggestionsDirty.value)
watch(dirty,v=>emit('dirtyChanged',v),{immediate:true})
async function loadReview(force=false){const seq=++loadSequence;const startDraft=JSON.stringify(drafts);try{const value=await api<{input_snapshot:string;documents:MatterDocument[]}>(`/matters/${props.matterId}/metadata-review`);if(!alive||seq!==loadSequence)return;if(!force&&startDraft!==JSON.stringify(drafts)&&dirty.value){loadError.value='A refresh arrived while you were editing. Your draft is retained.';return}baseline.value=value.documents;selectedIds.value=selectedIds.value.filter(id=>baseline.value.some(d=>d.id===id));snapshot.value=value.input_snapshot;resetRows();loadError.value=''}catch(e){loadError.value=friendlyError(e)}}
function resetRows(){for(const doc of baseline.value){drafts[doc.id]={title:doc.title,label:doc.label,kind:doc.kind,part:doc.part,group_key:doc.group_key,group_role:doc.group_role,document_date:doc.document_date,date_confirmed:doc.date_status==='confirmed'};moveTargets[doc.id]=doc.position+1}}
function discard(){suggestionsRef.value?.discard();resetRows()}
function rowChanges(doc:MatterDocument){const d=drafts[doc.id];const fields:Record<string,unknown>={};for(const k of ['title','label','kind','part','group_key','group_role'] as const)if(d[k]!==doc[k])fields[k]=typeof d[k]==='string'?(d[k] as string).trim():d[k];if('label' in fields){fields.label_status=fields.label?'confirmed':'unreviewed';fields.label_source=fields.label?'user':'none'}if(d.document_date!==doc.document_date||d.date_confirmed!==(doc.date_status==='confirmed'))Object.assign(fields,{document_date:d.document_date,date_status:d.document_date?'confirmed':'unknown',date_source:d.document_date?'user':'none',date_note:''});return fields}
async function saveRows(ids?:string[]){if(localBusy.value)return false;const rows=dirtyRows.value.filter(d=>!ids||ids.includes(d.id));const retained=Object.fromEntries(dirtyRows.value.filter(d=>ids&&!ids.includes(d.id)).map(d=>[d.id,{...drafts[d.id]}]));if(!rows.length)return true;localBusy.value=true;loadError.value='';try{await api(`/matters/${props.matterId}/metadata-review`,'PUT',{expected_snapshot:snapshot.value,rows:rows.map(d=>({document_id:d.id,changes:rowChanges(d)}))});await loadReview();Object.assign(drafts,retained);emit('modified');message.value='Changes saved in one undoable action.';return true}catch(e){loadError.value=friendlyError(e);return false}finally{localBusy.value=false}}
async function save(){return suggestionsOpen.value?!!(await suggestionsRef.value?.save()):await saveRows()}
async function openSuggestions(){if(dirtyRows.value.length){loadError.value='Save or discard table changes before reviewing source suggestions.';return}reviewIds.value=selectedIds.value.length?[...selectedIds.value]:baseline.value.map(d=>d.id);suggestionsOpen.value=true}
function closeSuggestions(){if(suggestionsDirty.value){emit('close');return}suggestionsOpen.value=false;void loadReview()}
function escapeCell(doc:MatterDocument,event:KeyboardEvent){if(event.key==='Escape'){const d=baseline.value.find(d=>d.id===doc.id);if(d){drafts[d.id]={title:d.title,label:d.label,kind:d.kind,part:d.part,group_key:d.group_key,group_role:d.group_role,document_date:d.document_date,date_confirmed:d.date_status==='confirmed'}}}}
defineExpose({save,discard,hasPendingDraft:()=>dirty.value,closeSuggestions:()=>{suggestionsOpen.value=false;void loadReview()},isSuggestionsOpen:()=>suggestionsOpen.value})

const selectedIds = ref<string[]>([])
const collapsedParts = ref<number[]>([])
const localBusy = ref(false)
const rowBusy = ref('')
const message = ref('')
const sortMode = ref<SortMode>('manual')
const sortPreview = ref<SortPreview | null>(null)
const drafts = reactive<Record<string,Draft>>({})
const moveTargets = reactive<Record<string,number>>({})
const batch = reactive({kind:'',part:'',group_key:'',applyGroup:false})
const labelOptions = reactive<{mode:LabelMode,prefix:string,start:number,group_separator:string,
  group_suffix_style:GroupSuffixStyle,first_member_base:boolean,
  existing_label_policy:'compact'|'preserve',reference_document_id:string}>({
  mode:'sequential', prefix:'P-', start:1, group_separator:'',
  group_suffix_style:'lower_alpha', first_member_base:false,
  existing_label_policy:'compact', reference_document_id:''
})
const labelPreview = ref<LabelPreview | null>(null)
const labelAcks = reactive({existing:false,exports:false,references:false})

watch(() => props.documents, ()=>{if(!dirty.value&&!localBusy.value&&!suggestionsOpen.value)void loadReview()}, {immediate:true})

const filtered = computed(() => {
  const needle = query.value.trim().toLocaleLowerCase()
  if (!needle) return baseline.value
  return baseline.value.filter(doc => [doc.title, doc.original_name, doc.label, doc.kind,
    doc.group_key, doc.document_date].join(' ').toLocaleLowerCase().includes(needle))
})
const parts = computed(() => [1,2,3,4].map(part => ({part,
  all:props.documents.filter(doc => doc.part === part),
  visible:filtered.value.filter(doc => doc.part === part)})).filter(section => section.all.length))
const selectedSet = computed(() => new Set(selectedIds.value))
const visibleSelected = computed(() => filtered.value.filter(doc => selectedSet.value.has(doc.id)).length)
const allVisibleSelected = computed(() => !!filtered.value.length && visibleSelected.value === filtered.value.length)
const duplicateLabels = computed(() => {
  const counts = new Map<string,number>()
  for (const doc of props.documents) if (doc.label.trim()) {
    const key = doc.label.trim().toLocaleLowerCase()
    counts.set(key, (counts.get(key) || 0) + 1)
  }
  return new Set([...counts].filter(([,count]) => count > 1).map(([label]) => label))
})
const labelGaps = computed(() => {
  const schemes = new Map<string,number[]>()
  for (const doc of props.documents) {
    const match = doc.label.trim().match(/^(.+?)[- ]?(\d+)$/)
    if (!match) continue
    const prefix = match[1].toLocaleUpperCase()
    const values = schemes.get(prefix) || []
    values.push(Number(match[2])); schemes.set(prefix, values)
  }
  const gaps:string[] = []
  for (const [prefix, values] of schemes) {
    const unique = [...new Set(values)].sort((a,b) => a-b)
    if (unique.length < 2) continue
    for (let n=unique[0]; n<unique[unique.length-1]; n++) if (!unique.includes(n)) gaps.push(`${prefix}-${n}`)
  }
  return gaps.slice(0,8)
})
const labelProposalProblems = computed(() => {
  if (!labelPreview.value) return [] as string[]
  const problems = labelPreview.value.blocking.filter(item =>
    !item.startsWith('Every selected document') && !item.includes('would be duplicated'))
  const final = new Map(props.documents.map(doc => [doc.id, doc.label.trim()]))
  for (const row of labelPreview.value.rows) final.set(row.document_id, row.proposed_label.trim())
  if (labelPreview.value.rows.some(row => !row.proposed_label.trim()))
    problems.push('Every selected document needs a label. Enter the missing labels manually.')
  const seen = new Set<string>()
  for (const label of final.values()) if (label) {
    const key = label.toLocaleLowerCase()
    if (seen.has(key)) { problems.push(`Label “${label}” would be duplicated.`); break }
    seen.add(key)
  }
  return [...new Set(problems)]
})
const changesExistingLabels = computed(() => !!labelPreview.value?.rows.some(row =>
  !!row.old_label.trim() && row.old_label.trim() !== row.proposed_label.trim()))
const needsReferenceAck = computed(() => !!labelPreview.value?.reference_review?.references.length)
const labelApplyReady = computed(() => !!labelPreview.value && !labelProposalProblems.value.length &&
  (!changesExistingLabels.value || labelAcks.existing) &&
  (!labelPreview.value.export_history.has_prior_exports || labelAcks.exports) &&
  (!needsReferenceAck.value || labelAcks.references))

function toggleVisible(checked:boolean) {
  const next = new Set(selectedIds.value)
  for (const doc of filtered.value) checked ? next.add(doc.id) : next.delete(doc.id)
  selectedIds.value = [...next]
}
function toggleSelected(id:string, checked:boolean) {
  const next = new Set(selectedIds.value)
  checked ? next.add(id) : next.delete(id)
  selectedIds.value = [...next]
}
function togglePart(part:number) {
  collapsedParts.value = collapsedParts.value.includes(part)
    ? collapsedParts.value.filter(value => value !== part) : [...collapsedParts.value, part]
}
function dateStatus(doc:MatterDocument) {
  if (doc.date_status === 'confirmed') return 'Confirmed by user'
  if (doc.date_status === 'suggested') return `Unreviewed ${doc.date_source} suggestion`
  if (doc.date_status === 'ambiguous') return `Ambiguous ${doc.date_source} text: ${doc.date_note}`
  return 'Unknown document date'
}
function sourceTimestamp(doc:MatterDocument) {
  if (!doc.source_file_mtime) return 'No source file timestamp supplied'
  return `File modified ${new Date(doc.source_file_mtime).toLocaleString()} — not used as the document date`
}
function rowDirty(doc:MatterDocument) {
  const draft = drafts[doc.id]
  if (!draft) return false
  const desiredStatus = draft.document_date ? (draft.date_confirmed ? 'confirmed' : doc.date_status)
    : (doc.date_status === 'ambiguous' ? 'ambiguous' : 'unknown')
  return draft.title !== doc.title || draft.label !== doc.label || draft.kind !== doc.kind ||
    draft.part !== doc.part || draft.group_key !== doc.group_key || draft.group_role !== doc.group_role ||
    draft.document_date !== doc.document_date || desiredStatus !== doc.date_status
}
async function saveRow(doc:MatterDocument) {if(await saveRows([doc.id]))document.querySelectorAll('.batch-row-menu[open]').forEach(el=>el.removeAttribute('open'))}
async function previewLabels() {
  if (!selectedIds.value.length || localBusy.value) return
  localBusy.value = true; message.value = ''; labelPreview.value = null
  labelAcks.existing = false; labelAcks.exports = false; labelAcks.references = false
  try {
    labelPreview.value = await api<LabelPreview>(`/matters/${props.matterId}/labels/preview`, 'POST', {
      ids:[...selectedIds.value], mode:labelOptions.mode, prefix:labelOptions.prefix,
      start:Number(labelOptions.start) || 1,
      group_separator:labelOptions.group_separator,
      group_suffix_style:labelOptions.group_suffix_style,
      first_member_base:labelOptions.first_member_base,
      existing_label_policy:labelOptions.existing_label_policy,
      reference_document_id:labelOptions.reference_document_id || null,
    })
  } catch (error) { emit('error', error) }
  finally { localBusy.value = false }
}
async function applyLabels() {
  if (!labelPreview.value || !labelApplyReady.value || localBusy.value) return
  localBusy.value = true; message.value = ''
  try {
    await api(`/matters/${props.matterId}/labels/apply`, 'POST', {
      mappings:labelPreview.value.rows.map(row => ({document_id:row.document_id,
        old_label:row.old_label,new_label:row.proposed_label.trim()})),
      reference_document_id:labelPreview.value.reference_review?.document_id || null,
      expected_reference_revision:labelPreview.value.reference_review?.revision || null,
      acknowledge_existing_labels:labelAcks.existing,
      acknowledge_prior_exports:labelAcks.exports,
      acknowledge_references:labelAcks.references,
    })
    message.value = `Applied ${labelPreview.value.rows.length} reviewed label mapping(s) in one undoable action.`
    labelPreview.value = null
    await loadReview()
    emit('modified')
  } catch (error) { emit('error', error) }
  finally { localBusy.value = false }
}
async function applyBatch() {
  if (!selectedIds.value.length || localBusy.value) return
  const payload:Record<string,unknown> = {ids:[...selectedIds.value]}
  if (batch.kind) payload.kind = batch.kind
  if (batch.part) payload.part = Number(batch.part)
  if (batch.applyGroup) payload.group_key = batch.group_key
  if (Object.keys(payload).length === 1) { message.value = 'Choose a type, Part or group change.'; return }
  localBusy.value = true; message.value = ''
  try {
    await api(`/matters/${props.matterId}/documents/batch`, 'PATCH', payload)
    message.value = `Updated ${selectedIds.value.length} selected document(s) in one undoable action.`
    await loadReview()
    emit('modified')
  } catch (error) { emit('error', error) }
  finally { localBusy.value = false }
}
async function previewSort() {
  localBusy.value = true; message.value = ''; sortPreview.value = null
  try {
    sortPreview.value = await api<SortPreview>(`/matters/${props.matterId}/documents/sort-preview`,
      'POST', {mode:sortMode.value,unknown_dates:'last'})
  } catch (error) { emit('error', error) }
  finally { localBusy.value = false }
}
async function applySort() {
  if (!sortPreview.value || localBusy.value) return
  localBusy.value = true
  try {
    await api(`/matters/${props.matterId}/documents/reorder`, 'POST', {ids:sortPreview.value.ids})
    message.value = `Applied ${sortPreview.value.mode.replace('_',' ')} order.`
    sortPreview.value = null
    await loadReview()
    emit('modified')
  } catch (error) { emit('error', error) }
  finally { localBusy.value = false }
}
async function moveDocument(doc:MatterDocument) {
  const target = Math.min(props.documents.length, Math.max(1, Number(moveTargets[doc.id]) || 1)) - 1
  const ids = props.documents.map(item => item.id)
  const from = ids.indexOf(doc.id)
  if (from === target || from < 0 || localBusy.value) return
  ids.splice(target, 0, ids.splice(from, 1)[0])
  localBusy.value = true
  try {
    await api(`/matters/${props.matterId}/documents/reorder`, 'POST', {ids})
    message.value = `Moved “${doc.title}” to position ${target + 1}.`
    await loadReview()
    emit('modified')
  } catch (error) { emit('error', error) }
  finally { localBusy.value = false }
}
</script>

<template>
 <div class="metadata-batch-root">
 <MetadataSuggestions v-if="suggestionsOpen" ref="suggestionsRef" :matter-id="matterId" :documents="documents" :ids="reviewIds" :disabled="busy" @dirty-changed="suggestionsDirty=$event" @modified="emit('modified')" @close="closeSuggestions"/>
 <section v-show="!suggestionsOpen" class="document-register batch-review" aria-label="Editable document table">
  <header class="batch-review-heading"><div><h1>Batch review</h1><p>{{documents.length}} documents · Edit metadata, labels and Parts in place.</p></div><button class="membership-button secondary" :disabled="!selectedIds.length" @click="emit('openDocument',selectedIds[0])">Open document</button><button class="text-tool" @click="emit('close')">Close table</button></header>
  <div class="batch-review-toolbar"><input type="checkbox" aria-label="Select all filtered documents" :checked="allVisibleSelected" @change="toggleVisible(($event.target as HTMLInputElement).checked)"/><strong :title="`${visibleSelected} visible in this filter`">{{selectedIds.length}} selected</strong><label><span class="sr-only">Assign selected Part</span><select v-model="batch.part" aria-label="Assign selected Part"><option value="">Assign selected → Part</option><option v-for="part in 4" :value="part" :key="part">Part {{part}}</option></select></label><select v-model="batch.kind" aria-label="Document type"><option value="">Document type</option><option v-for="kind in kinds" :value="kind" :key="kind">{{kind}}</option></select><button class="text-tool" :disabled="busy||localBusy||!selectedIds.length||!!dirtyRows.length" @click="applyBatch">Apply batch change</button><button class="membership-button quiet" :disabled="!documents.length||!!dirtyRows.length" @click="openSuggestions">Review title &amp; dates</button><input v-model="query" type="search" aria-label="Filter rows" placeholder="Filter documents"/><button class="membership-button secondary" :aria-expanded="showColumns" @click="showColumns=!showColumns">Columns</button><button class="membership-button secondary" :aria-pressed="compact" @click="compact=!compact">{{compact?'Compact':'Comfortable'}}</button><button class="text-tool" :aria-expanded="advanced" @click="advanced=!advanced">More tools</button></div>
  <div v-if="showColumns" class="batch-columns"><label v-for="name in ['File name','Group','OCR','Status']" :key="name"><input type="checkbox" :checked="!hiddenColumns.includes(name)" @change="hiddenColumns=hiddenColumns.includes(name)?hiddenColumns.filter(x=>x!==name):[...hiddenColumns,name]"/>{{name}}</label></div>
  <div v-if="loadError" role="alert" class="review-error">{{loadError}} <button class="text-tool" @click="loadReview(true)">Discard table drafts and reload</button></div>
  <div v-if="advanced" class="batch-advanced"><slot name="history"/><button class="text-tool" @click="selectedIds=[]">Clear selection</button><label><input v-model="batch.applyGroup" type="checkbox"/>Change selected group<input v-model="batch.group_key" maxlength="100" :disabled="!batch.applyGroup"/></label><button class="text-tool" :disabled="!!dirtyRows.length" @click="applyBatch">Apply group</button>
    <div class="label-proposal-bar">
      <div><strong>Propose labels for the explicit selection</strong><small>Preview only. Labels stay independent of filenames and page numbers until you confirm the full mapping.</small></div>
      <label>Method<select v-model="labelOptions.mode"><option value="sequential">Sequential</option><option value="grouped_suffix">Grouped suffix</option><option value="filename">Conservative filename scan</option></select></label>
      <label v-if="labelOptions.mode!=='filename'">Prefix<input v-model="labelOptions.prefix" maxlength="20"/></label>
      <label v-if="labelOptions.mode!=='filename'">Start<input v-model.number="labelOptions.start" type="number" min="1" max="9999"/></label>
      <label>Existing labels<select v-model="labelOptions.existing_label_policy"><option value="compact">Compact and renumber selection</option><option value="preserve">Preserve; fill new labels manually</option></select></label>
      <template v-if="labelOptions.mode==='grouped_suffix'">
        <label>Suffix style<select v-model="labelOptions.group_suffix_style"><option value="lower_alpha">a, b, c</option><option value="upper_alpha">A, B, C</option><option value="numeric">1, 2, 3</option><option value="lower_alpha_parenthesized">(a), (b), (c)</option><option value="upper_alpha_parenthesized">(A), (B), (C)</option><option value="numeric_parenthesized">(1), (2), (3)</option></select></label>
        <label>Separator<input v-model="labelOptions.group_separator" maxlength="5" placeholder="None, -, . or /"/></label>
        <label class="group-base-choice"><span><input v-model="labelOptions.first_member_base" type="checkbox"/> First member uses base label</span><small>Uses group order; does not certify a parent document.</small></label>
      </template>
      <label>Citation reference<select v-model="labelOptions.reference_document_id"><option value="">Do not scan</option><option v-for="doc in documents" :key="doc.id" :value="doc.id">{{doc.title}}</option></select></label>
      <button class="btn secondary small" :disabled="busy || localBusy || !selectedIds.length" @click="previewLabels">Preview label changes</button>
    </div>

    <div v-if="labelPreview" class="label-proposal-preview" role="dialog" aria-label="Label proposal preview">
      <div class="sort-preview-head"><div><strong>Old → proposed label mapping</strong><small>{{labelPreview.policy}}</small></div><button class="btn secondary small" @click="labelPreview=null">Cancel without changes</button></div>
      <div v-for="warning in labelPreview.warnings" :key="warning" class="notice amber">{{warning}}</div>
      <div v-for="problem in labelProposalProblems" :key="problem" class="notice error">{{problem}}</div>
      <div class="label-map-wrap"><table class="label-map"><thead><tr><th>Document identity</th><th>Description</th><th>Existing</th><th>Proposed</th><th>Reason</th></tr></thead><tbody>
        <tr v-for="row in labelPreview.rows" :key="row.document_id"><td><strong>{{row.title}}</strong><small>{{row.original_name}}<template v-if="row.group_key"> · Group {{row.group_key}} · {{row.group_role}}</template></small></td><td>{{row.description}}</td><td>{{row.old_label || 'Unlabelled'}}<small>{{row.old_status}}</small></td><td><input v-model="row.proposed_label" maxlength="60" :aria-label="'Proposed label for '+row.title"/></td><td><small>{{row.reason}}</small></td></tr>
      </tbody></table></div>

      <section v-if="labelPreview.reference_review" class="citation-review">
        <h3>Citation review · {{labelPreview.reference_review.title}} revision {{labelPreview.reference_review.revision}}</h3>
        <p>{{labelPreview.reference_review.searchable_pages}} searchable page(s). Matches are text candidates, not verified legal citations; scanned or OCR-imperfect text may be missed.</p>
        <ul v-if="labelPreview.reference_review.references.length"><li v-for="(reference,index) in labelPreview.reference_review.references" :key="index"><strong>Page {{reference.page}} · {{reference.raw}}</strong> <span class="date-badge" :class="reference.status==='unmatched'?'ambiguous':'suggested'">{{reference.status.replace('_',' ')}}</span><small>{{reference.snippet}}</small></li></ul>
        <p v-else>No annexure/exhibit references were found in searchable text. This is not proof that none exist.</p>
      </section>

      <div v-if="labelPreview.export_history.has_prior_exports" class="notice amber">
        <strong>Prior generated builds remain immutable.</strong> {{labelPreview.export_history.prior_uses.length}} build(s) recorded selected labels; {{labelPreview.export_history.unknown_exports.length}} older build(s) lack a label snapshot. Any of them may have circulated.
      </div>
      <div class="label-acknowledgements">
        <label v-if="changesExistingLabels"><input v-model="labelAcks.existing" type="checkbox"/> I reviewed every existing-label → new-label change.</label>
        <label v-if="labelPreview.export_history.has_prior_exports"><input v-model="labelAcks.exports" type="checkbox"/> I understand prior generated builds are unchanged and may have circulated.</label>
        <label v-if="needsReferenceAck"><input v-model="labelAcks.references" type="checkbox"/> I reviewed the citation candidates and will check external references manually.</label>
      </div>
      <div class="label-proposal-actions"><button class="btn secondary small" @click="labelPreview=null">Cancel</button><button class="btn primary small" :disabled="busy || localBusy || !labelApplyReady" @click="applyLabels">Apply reviewed mapping</button></div>
    </div>

    <div class="sort-bar">
      <div><strong>Preview order</strong><small>Unknown dates go last; equal keys retain current manual order.</small></div>
      <select v-model="sortMode" aria-label="Order method"><option value="manual">Current manual order</option><option value="upload">Upload order</option><option value="filename">Original filename</option><option value="document_date">Document date</option></select>
      <button class="btn secondary small" :disabled="busy || localBusy" @click="previewSort">Preview order</button>
    </div>

    <div v-if="sortPreview" class="sort-preview" role="dialog" aria-label="Sort preview">
      <div class="sort-preview-head"><div><strong>Proposed {{sortPreview.mode.replace('_',' ')}} order</strong><small>Unknown dates: last · ties: {{sortPreview.stable_ties}} · {{sortPreview.group_policy}}.</small></div><div><button class="btn secondary small" @click="sortPreview=null">Cancel</button><button class="btn primary small" :disabled="busy || localBusy" @click="applySort">Apply order</button></div></div>
      <ol><li v-for="row in sortPreview.rows" :key="row.id" :class="{'sort-moved':row.old_position!==row.new_position}"><span>{{row.new_position+1}}</span><div><strong>{{row.title}}</strong><small>{{row.original_name}}<template v-if="row.group_key"> · Group {{row.group_key}}</template></small></div><span class="date-badge" :class="row.date_status">{{row.document_date || (row.date_status==='ambiguous' ? row.date_note : 'Unknown date')}} · {{row.date_status}}</span><small>#{{row.old_position+1}} → #{{row.new_position+1}}</small></li></ol>
    </div>


  </div>
  <p v-if="message" role="status" class="review-notice">{{message}}</p>
  <p v-if="duplicateLabels.size||labelGaps.length" class="review-notice">Label review: {{[...duplicateLabels,...labelGaps].join(', ')}}. No automatic correction made.</p>
  <div class="batch-table-scroll" :inert="localBusy"><table :class="['batch-table',{'comfortable':!compact}]"><colgroup><col style="width:3.3%"/><col style="width:3%"/><col style="width:19.3%"/><col v-if="!hiddenColumns.includes('File name')" style="width:12.9%"/><col style="width:10.1%"/><col style="width:8.2%"/><col style="width:5.7%"/><col style="width:7.7%"/><col v-if="!hiddenColumns.includes('Group')" style="width:7.7%"/><col style="width:7.7%"/><col v-if="!hiddenColumns.includes('OCR')" style="width:5.7%"/><col v-if="!hiddenColumns.includes('Status')" style="width:8.8%"/></colgroup>
   <thead><tr><th></th><th>↕</th><th>Title</th><th v-if="!hiddenColumns.includes('File name')">File name</th><th>Document date</th><th>Type</th><th>Part</th><th>Label</th><th v-if="!hiddenColumns.includes('Group')">Group</th><th>Source pages</th><th v-if="!hiddenColumns.includes('OCR')">OCR</th><th v-if="!hiddenColumns.includes('Status')">Status</th></tr></thead>
   <tbody><tr v-for="doc in filtered" :key="doc.id" :class="{'row-selected':selectedSet.has(doc.id),'row-dirty':rowDirty(doc)}" @keydown="escapeCell(doc,$event)"><td><input type="checkbox" :aria-label="'Select '+doc.title" :checked="selectedSet.has(doc.id)" @change="toggleSelected(doc.id,($event.target as HTMLInputElement).checked)"/></td><td><details class="batch-row-menu"><summary :aria-label="'Actions for '+doc.title">⋮</summary><div><button class="text-tool" @click="emit('openDocument',doc.id)">Preview</button><button class="text-tool" :disabled="!rowDirty(doc)||localBusy" @click="saveRow(doc)">Save row</button><label>Position<input v-model.number="moveTargets[doc.id]" type="number" min="1" :max="documents.length"/></label><button class="text-tool" :disabled="!!dirtyRows.length" @click="moveDocument(doc)">Move</button><label>Hierarchy<select v-model="drafts[doc.id].group_role"><option value="member">Member</option><option value="parent">Designated parent</option></select></label></div></details></td><td><input v-model="drafts[doc.id].title" :aria-label="'Title for '+doc.original_name" maxlength="250"/></td><td v-if="!hiddenColumns.includes('File name')" :title="doc.original_name+' · '+sourceTimestamp(doc)">{{doc.original_name}}</td><td><input v-model="drafts[doc.id].document_date" :aria-label="'Date for '+doc.original_name" type="date" @input="drafts[doc.id].date_confirmed=true" :title="dateStatus(doc)"/></td><td><select v-model="drafts[doc.id].kind" :aria-label="'Type for '+doc.original_name"><option v-for="kind in kinds" :value="kind" :key="kind">{{kind}}</option></select></td><td><select v-model.number="drafts[doc.id].part" :aria-label="'Part for '+doc.original_name"><option v-for="part in 4" :value="part" :key="part">{{part}}</option></select></td><td><input v-model="drafts[doc.id].label" :aria-label="'Label for '+doc.original_name" maxlength="60"/></td><td v-if="!hiddenColumns.includes('Group')"><input v-model="drafts[doc.id].group_key" :aria-label="'Group for '+doc.original_name" maxlength="100"/></td><td>{{doc.page_count}}</td><td v-if="!hiddenColumns.includes('OCR')" :title="doc.ocr_status">{{doc.ocr_status==='native_text'?'Text':doc.ocr_status.split('_').join(' ')}}</td><td v-if="!hiddenColumns.includes('Status')">{{rowDirty(doc)?'Unsaved':doc.date_status==='confirmed'?'Reviewed':'Review date'}}</td></tr></tbody>
  </table><p v-if="!filtered.length" class="review-empty">No documents match this filter. Your selection is unchanged.</p></div>
  <footer class="batch-savebar"><span>{{dirtyRows.length}} unsaved changes</span><small>Tab moves cells · Enter edits · Escape cancels row</small><button class="membership-button quiet" :disabled="!dirtyRows.length||localBusy" @click="discard">Discard changes</button><button class="membership-button primary" :disabled="!dirtyRows.length||busy||localBusy" @click="save">{{localBusy?'Saving…':'Save changes'}}</button></footer>
 </section>
 </div>
</template>
