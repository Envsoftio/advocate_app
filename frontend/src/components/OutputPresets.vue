<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { api } from '../lib/api'
import { friendlyError } from '../lib/format'
import type { MatterDocument, MarkPosition, OutputMarks } from '../lib/types'

interface Settings {
  name: string
  max_pages_per_volume: number
  keep_groups_intact: boolean
  output_marks: OutputMarks
}
interface Preset { id: string; name: string; version: number; settings: Settings; created_at: string }
interface Applied { id: string; name: string; version: number; settings: Settings }

const props = defineProps<{matterId: string; maxPages: number; keepGroups: boolean; settingsDirty: boolean; disabled: boolean; refreshKey: number; documents: MatterDocument[]}>()
const emit = defineEmits<{applyRequested: [id: string]; modified: []; openDocument: [id: string]; dirtyChanged: [dirty: boolean]}>()
const presets = ref<Preset[]>([])
const applied = ref<Applied | null>(null)
const name = ref('')
const selectedId = ref('')
const busy = ref(false)
const error = ref('')
const notice = ref('')
const marks = ref<OutputMarks | null>(null)
let savedMarks = ''
const marksDirty = computed(() => !!marks.value && JSON.stringify(marks.value) !== savedMarks)
watch(marksDirty, dirty => emit('dirtyChanged', dirty), {immediate: true})
const annexures = computed(() => props.documents.filter(document => document.kind === 'annexure'))
const positions: Array<[MarkPosition, string]> = [
  ['top_left','Top left'],['top_center','Top centre'],['top_right','Top right'],
  ['center','Centre'],['bottom_left','Bottom left'],
  ['bottom_center','Bottom centre'],['bottom_right','Bottom right'],
]
const selected = computed(() => presets.value.find(p => p.id === selectedId.value))
const overrides = computed(() => !!applied.value && (
  applied.value.settings.max_pages_per_volume !== props.maxPages ||
  applied.value.settings.keep_groups_intact !== props.keepGroups ||
  (!!marks.value && JSON.stringify({...marks.value,annexure_overrides:{}}) !==
    JSON.stringify({...applied.value.settings.output_marks,annexure_overrides:{}}))))
const exceptionCount = computed(() => Object.keys(marks.value?.annexure_overrides ?? {}).length)
const policySummary = computed(() => {
  if (!marks.value) return ''
  const m = marks.value
  const numbering = m.bates_enabled
    ? `Bates ${m.bates_prefix || ''}${String(m.bates_start).padStart(m.bates_padding, '0')} at ${positions.find(([key]) => key === m.bates_position)?.[1].toLowerCase() || m.bates_position}`
    : 'No Bates identifiers'
  const annexures = m.annexure_mode === 'label_stamp'
    ? `Label on ${m.annexure_stamp_scope === 'first' ? 'first' : 'all'} source page${m.annexure_stamp_scope === 'first' ? '' : 's'}`
    : m.annexure_mode === 'separator' ? 'Separator before each annexure' : 'No added annexure mark'
  return `${numbering} · ${annexures} · ${exceptionCount.value} document exception${exceptionCount.value === 1 ? '' : 's'}`
})

async function load() {
  try {
    const [available, current, outputMarks] = await Promise.all([
      api<Preset[]>('/output-presets'),
      api<Applied | null>(`/matters/${props.matterId}/output-preset`),
      api<OutputMarks>(`/matters/${props.matterId}/output-marks`),
    ])
    presets.value = available
    applied.value = current
    if (!marksDirty.value) { marks.value = outputMarks; savedMarks = JSON.stringify(outputMarks) }
    if (!selectedId.value && available.length) selectedId.value = available[0]!.id
  } catch (err) { error.value = friendlyError(err) }
}
watch(() => [props.matterId, props.refreshKey], () => { void load() }, {immediate: true})

function setOverride(id: string, value: string) {
  if (!marks.value) return
  const next = {...marks.value.annexure_overrides}
  if (value === 'inherit') delete next[id]
  else next[id] = value as 'none' | 'label_stamp' | 'separator'
  marks.value.annexure_overrides = next
}

async function saveFormatting() {
  if (!marks.value || busy.value) return
  busy.value = true; error.value = ''; notice.value = ''
  try {
    const saved = await api<OutputMarks>(`/matters/${props.matterId}/output-marks`,
                                         'PUT', marks.value)
    marks.value = saved
    savedMarks = JSON.stringify(saved)
    notice.value = 'Output policy saved. Existing exports remain unchanged; rebuild to apply it.'
    emit('modified')
  } catch (err) { error.value = friendlyError(err) }
  finally { busy.value = false }
}

async function saveVersion() {
  if (!name.value.trim() || props.settingsDirty || marksDirty.value || busy.value) return
  busy.value = true; error.value = ''; notice.value = ''
  try {
    const currentMarks = await api<OutputMarks>(`/matters/${props.matterId}/output-marks`)
    const preset = await api<Preset>('/output-presets', 'POST', {
      name: name.value.trim(), max_pages_per_volume: props.maxPages,
      keep_groups_intact: props.keepGroups,
      output_marks: {...currentMarks, annexure_overrides:{}},
    })
    await load()
    selectedId.value = preset.id
    notice.value = `${preset.name} v${preset.version} saved as a technical preset. Apply it explicitly to a matter.`
  } catch (err) { error.value = friendlyError(err) }
  finally { busy.value = false }
}
function requestApply() {
  if (selectedId.value) emit('applyRequested', selectedId.value)
}
function discard() {
  if (savedMarks) marks.value = JSON.parse(savedMarks) as OutputMarks
}
defineExpose({discard})
</script>

<template>
  <section class="panel build-panel" aria-label="Reusable output presets">
    <div class="panel-header"><div><h2>Reusable output presets</h2><span>Versioned technical preferences; no court approval</span></div></div>
    <div class="preset-body">
      <p v-if="marks" class="preset-live-summary" role="status">Output policy: {{policySummary}}<span v-if="marksDirty"> · Unsaved</span></p>
      <div v-if="marks" class="preset-formatting">
        <details class="preset-section"><summary>Cover &amp; sections <span>Current technical layout</span></summary>
          <p class="setting-description">The current technical layout creates a linked index for each volume. Add any cover as a normal source document; it keeps its place in the register and page map. Reviewed court-specific cover and index layouts are pending.</p>
        </details>
        <details class="preset-section"><summary>Numbering <span>{{marks.bates_enabled ? 'Bates enabled' : 'Physical pages only'}}</span></summary>
        <p class="setting-description">Physical volume page numbers remain separate from Bates identifiers. Bates runs across selected pages in document order.</p>
        <label class="checkbox-line"><input v-model="marks.bates_enabled" type="checkbox"/><span>Bates identifiers</span></label>
        <div class="preset-fields">
          <label>Prefix<input v-model="marks.bates_prefix" maxlength="24" :disabled="!marks.bates_enabled"/></label>
          <label>Start<input v-model.number="marks.bates_start" type="number" min="1" max="999999999" :disabled="!marks.bates_enabled"/></label>
          <label>Zero padding<input v-model.number="marks.bates_padding" type="number" min="1" max="12" :disabled="!marks.bates_enabled"/></label>
          <label>Placement<select v-model="marks.bates_position" :disabled="!marks.bates_enabled"><option v-for="item in positions" :key="item[0]" :value="item[0]">{{item[1]}}</option></select></label>
          <label>Font size<input v-model.number="marks.bates_font_size" type="number" min="6" max="24" :disabled="!marks.bates_enabled"/></label>
        </div>
        <label class="checkbox-line"><input v-model="marks.bates_include_index" type="checkbox" :disabled="!marks.bates_enabled"/><span>Number index pages</span></label>
        <label class="checkbox-line"><input v-model="marks.bates_include_separator" type="checkbox" :disabled="!marks.bates_enabled"/><span>Number separator pages</span></label>
        <label class="checkbox-line"><input v-model="marks.bates_restart_per_volume" type="checkbox" :disabled="!marks.bates_enabled"/><span>Restart Bates sequence in each volume</span></label>
        </details>
        <details class="preset-section"><summary>Annexure output <span>{{annexures.length}} documents · {{exceptionCount}} exceptions</span></summary>
        <p class="setting-description">Choose no added mark, a label on source pages, or one separate sheet before each annexure. Confirmed register labels are required for marked output.</p>
        <label>Default for annexures<select v-model="marks.annexure_mode"><option value="none">No added mark</option><option value="label_stamp">Label stamp</option><option value="separator">Separator sheet</option></select></label>
        <template v-if="marks.annexure_mode==='label_stamp' || Object.values(marks.annexure_overrides).includes('label_stamp')">
          <label>Stamp pages<select v-model="marks.annexure_stamp_scope"><option value="first">First page only</option><option value="all">Every source page</option></select></label>
          <label>Stamp position<select v-model="marks.annexure_stamp_position"><option v-for="item in positions" :key="item[0]" :value="item[0]">{{item[1]}}</option></select></label>
          <label>Stamp size<input v-model.number="marks.annexure_stamp_font_size" type="number" min="6" max="24"/></label>
        </template>
        <div v-if="annexures.length" class="preset-exceptions">
          <div v-for="document in annexures" :key="document.id"><span>{{document.label || 'Unlabelled'}} · {{document.title}} <small v-if="document.label_status!=='confirmed'">Needs confirmed label for marked output</small></span><select :aria-label="'Output treatment for '+document.title" :value="marks.annexure_overrides[document.id] || 'inherit'" @change="setOverride(document.id, ($event.target as HTMLSelectElement).value)"><option value="inherit">Matter default</option><option value="none">No added mark</option><option value="label_stamp">Label stamp</option><option value="separator">Separator sheet</option></select><button class="text-tool" @click="emit('openDocument',document.id)">Preview page</button></div>
        </div>
        <p class="setting-description">Open a working page to preview stamp placement and detectable overlap. Separator sheets appear in the page-map preview below. No source page is resized or cropped.</p>
        </details>
        <details class="preset-section"><summary>Optional marks <span>{{marks.watermark_enabled ? 'Watermark on' : 'Watermark off'}} · {{marks.timestamp_enabled ? 'Timestamp on' : 'Timestamp off'}}</span></summary>
          <p class="setting-description">Watermark and build timestamp are configured and previewed on an actual working page in the PDF viewer’s Output tools. They are included when you save a technical preset.</p>
        </details>
        <button class="btn secondary" :disabled="disabled || busy || !marksDirty" @click="saveFormatting">Save numbering & annexure policy</button>
        <p v-if="marksDirty" class="hint" role="status">Unsaved output policy. Save it before building or creating a preset.</p>
      </div>
      <p v-if="applied" class="setting-description">Applied: <strong>{{applied.name}} v{{applied.version}}</strong><span v-if="overrides"> · Matter settings differ from this version</span><span v-if="exceptionCount"> · {{exceptionCount}} document exception(s)</span>. Existing exports keep their recorded inputs.</p>
      <p v-else class="setting-description">This matter uses its own settings. Choose a saved preset to copy its settings here.</p>
      <label class="field">Saved version
        <select v-model="selectedId"><option value="" disabled>Choose a preset</option><option v-for="preset in presets" :key="preset.id" :value="preset.id">{{preset.name}} v{{preset.version}}</option></select>
      </label>
      <p v-if="selected" class="setting-description">{{selected.settings.max_pages_per_volume}} pages per volume · {{selected.settings.keep_groups_intact ? 'groups kept together' : 'groups may split' }} · {{selected.settings.output_marks.watermark_enabled ? 'watermark on' : 'watermark off' }} · {{selected.settings.output_marks.timestamp_enabled ? 'timestamp on' : 'timestamp off' }} · {{selected.settings.output_marks.bates_enabled ? 'Bates on' : 'Bates off' }} · {{selected.settings.output_marks.annexure_mode || 'none'}} annexure mode</p>
      <button class="btn secondary" :disabled="disabled || !selectedId || busy" @click="requestApply">Apply selected version to this matter</button>
      <label class="field">Save current matter settings as a new version
        <input v-model="name" maxlength="80" placeholder="Technical layout name"/>
      </label>
      <p class="setting-description">Using an existing name creates the next version. Document exceptions stay with this matter and are not copied to another matter.</p>
      <button class="btn secondary" :disabled="disabled || busy || settingsDirty || marksDirty || !name.trim()" @click="saveVersion">Save new version</button>
      <p v-if="error" role="alert" class="hint">{{error}}</p><p v-if="notice" role="status" class="hint">{{notice}}</p>
    </div>
  </section>
</template>

<style scoped>
.preset-live-summary { margin: 0; padding: 12px 14px; border-radius: 8px; background: var(--surface-soft, #f1f6fc); color: var(--text, #263d55); font-size: 12px; line-height: 1.5; }
.preset-section { border: 1px solid var(--border, #dfe6ee); border-radius: 8px; padding: 0 12px; min-width: 0; }
.preset-section > summary { cursor: pointer; padding: 12px 2px; font-size: 13px; font-weight: 700; color: var(--text, #263d55); }
.preset-section > summary span { margin-left: 8px; font-size: 11px; font-weight: 500; color: var(--muted, #52647a); }
.preset-section[open] { display: grid; gap: 12px; padding-bottom: 12px; }
.preset-section[open] > summary { margin-bottom: -4px; }
.preset-section:focus-within { border-color: #91b8ff; }
</style>
