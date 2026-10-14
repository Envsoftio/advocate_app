<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from 'vue'
import { ArrowRight, CircleAlert, FileSpreadsheet, Plus, Trash2, Undo2, Redo2 } from 'lucide-vue-next'
import type { CourtProfile } from '../lib/types'
import { buildMemoPartiesCsv } from '../lib/delhi'
import type { MemoPartyRow } from '../lib/delhi'
import { api } from '../lib/api'

type FilingGuidance = NonNullable<CourtProfile['filing_guidance']>

const props = defineProps<{ guidance: FilingGuidance, matterId: string | null, canUndo: boolean, canRedo: boolean }>()
const emit = defineEmits<{ (event: 'open-url', url: string): void; (event: 'modified'): void; (event: 'undo'): void; (event: 'redo'): void }>()
const blankParty = (): MemoPartyRow => ({ role: 'Petitioner', name: '', fatherOrHusbandName: '', address: '', email: '', mobile: '' })
const parties = ref<MemoPartyRow[]>([blankParty()])
const saveState = ref<'loading' | 'saved' | 'unsaved' | 'saving' | 'failed'>('loading')
let loading = false
let timer: ReturnType<typeof setTimeout> | null = null
let generation = 0
let editVersion = 0
let pendingSave: Promise<void> = Promise.resolve()

function saveNow(matterId = props.matterId, rows = parties.value.map(row => ({...row})), version = editVersion) {
  if (!matterId) return
  saveState.value = 'saving'
  pendingSave = pendingSave.then(async () => {
    try {
      await api(`/matters/${matterId}/memo-parties`, 'PUT', {parties: rows})
      if (matterId === props.matterId) {
        emit('modified')
        if (version === editVersion) saveState.value = 'saved'
      }
    } catch {
      if (matterId === props.matterId && version === editVersion) saveState.value = 'failed'
    }
  })
  return pendingSave
}

async function flushPendingSave(): Promise<boolean> {
  if (timer) { clearTimeout(timer); timer = null }
  if (saveState.value === 'unsaved') await saveNow()
  else if (saveState.value === 'saving') await pendingSave
  return saveState.value === 'saved'
}

defineExpose({flushPendingSave})

watch(() => props.matterId, async (matterId, oldId) => {
  const current = ++generation
  if (timer) { clearTimeout(timer); timer = null }
  if (oldId && saveState.value === 'unsaved') await saveNow(oldId)
  loading = true
  saveState.value = 'loading'
  try {
    const saved = matterId ? await api<{parties: MemoPartyRow[]}>(`/matters/${matterId}/memo-parties`) : null
    if (current !== generation) return
    parties.value = saved?.parties.length ? saved.parties : [blankParty()]
    saveState.value = 'saved'
  } catch {
    if (current === generation) saveState.value = 'failed'
  } finally { if (current === generation) loading = false }
}, {immediate: true})

watch(parties, () => {
  if (loading || !props.matterId) return
  editVersion += 1
  saveState.value = 'unsaved'
  if (timer) clearTimeout(timer)
  timer = setTimeout(() => { timer = null; void saveNow() }, 600)
}, {deep: true})

onBeforeUnmount(() => {
  if (timer) { clearTimeout(timer); timer = null; void saveNow() }
})

function addParty() { parties.value.push(blankParty()) }
function removeParty(index: number) { if (parties.value.length > 1) parties.value.splice(index, 1) }
function downloadReviewCsv() {
  const csv = buildMemoPartiesCsv(parties.value)
  const url = URL.createObjectURL(new Blob([csv], {type: 'text/csv;charset=utf-8'}))
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = 'memo-of-parties-review.csv'
  anchor.click()
  URL.revokeObjectURL(url)
}
</script>

<template>
  <section class="filing-guide">
    <div class="guide-heading"><div><div class="guide-eyebrow">DELHI HIGH COURT</div><h2>{{ guidance.title }}</h2><p>{{ guidance.scope }}</p></div><div class="review-chip">Reviewed through {{ guidance.reviewed_through }}</div></div>
    <div class="requirements-grid">
      <article v-for="requirement in guidance.requirements" :key="requirement.title" class="guide-card"><h3>{{ requirement.title }}</h3><p>{{ requirement.detail }}</p><button class="source-link" @click="emit('open-url', requirement.source_url)">{{ requirement.source_label }} <ArrowRight :size="13"/></button></article>
    </div>
    <div class="conflict-box"><CircleAlert :size="20"/><div><strong>Source precedence and review warnings</strong><p v-for="warning in guidance.source_conflicts" :key="warning">{{ warning }}</p></div></div>
    <div class="template-section"><h3>Official forms and templates</h3><div class="template-list"><button v-for="template in guidance.templates" :key="template.url" class="template-row" @click="emit('open-url', template.url)"><FileSpreadsheet :size="20"/><div><strong>{{ template.title }} <span>{{ template.format }}</span></strong><p>{{ template.purpose }}</p></div><ArrowRight :size="16"/></button></div></div>
    <div class="memo-helper">
      <div class="memo-heading"><div><h3>Memo of Parties data helper</h3><p>Enter party data once, then download a review CSV using the court template's field labels.</p><p v-if="!matterId">Select or create a matter to store party entries.</p><p v-else role="status">{{saveState === 'failed' ? 'Save failed. Retry before leaving this page.' : saveState === 'saved' ? 'Saved to this matter' : saveState === 'saving' ? 'Saving…' : saveState === 'unsaved' ? 'Unsaved changes' : 'Loading…'}}</p></div><div class="memo-history-actions"><button class="btn secondary small" :disabled="!canUndo || saveState === 'loading'" @click="emit('undo')"><Undo2 :size="15"/> Undo matter edit</button><button class="btn secondary small" :disabled="!canRedo || saveState === 'loading'" @click="emit('redo')"><Redo2 :size="15"/> Redo matter edit</button><button class="btn secondary small" :disabled="!matterId || saveState === 'loading'" @click="addParty"><Plus :size="15"/> Add party</button></div></div>
      <div v-for="(party,index) in parties" :key="index" class="party-form"><div class="party-number">Party {{ index + 1 }}</div><label>Role<select v-model="party.role" :disabled="!matterId || loading"><option>Petitioner</option><option>Respondent</option></select></label><label>Party name<input v-model="party.name" :disabled="!matterId || loading"/></label><label>Father/Husband name<input v-model="party.fatherOrHusbandName" :disabled="!matterId || loading"/></label><label class="wide">Address<textarea v-model="party.address" rows="2" :disabled="!matterId || loading"></textarea></label><label>Email<input v-model="party.email" type="email" :disabled="!matterId || loading"/></label><label>Mobile<input v-model="party.mobile" inputmode="tel" :disabled="!matterId || loading"/></label><button class="remove-party" :disabled="!matterId || loading || parties.length === 1" title="Remove party" @click="removeParty(index)"><Trash2 :size="16"/></button></div>
      <div class="memo-actions"><button class="btn primary" :disabled="!matterId || saveState !== 'saved'" @click="downloadReviewCsv"><FileSpreadsheet :size="16"/> Download review CSV</button><button v-if="saveState === 'failed'" class="btn secondary" @click="saveNow()">Retry save</button><p>This CSV is a transfer/review aid, not the official XLS upload file. Compare it with the current official template before filing.</p></div>
    </div>
  </section>
</template>

<style scoped>
.filing-guide{margin-top:22px;display:grid;gap:16px}.guide-heading{display:flex;justify-content:space-between;gap:24px;align-items:flex-start}.guide-heading h2{margin:3px 0 6px}.guide-heading p{margin:0;max-width:780px;color:var(--muted)}.guide-eyebrow{font-size:11px;font-weight:800;letter-spacing:.12em;color:var(--primary)}.review-chip{white-space:nowrap;background:#eef4ff;color:#294e8c;border-radius:20px;padding:7px 11px;font-size:12px;font-weight:700}.requirements-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:12px}.guide-card,.template-section,.memo-helper{background:var(--panel);border:1px solid var(--border);border-radius:12px;padding:16px}.guide-card h3,.template-section h3,.memo-helper h3{margin:0 0 7px}.guide-card p,.template-row p,.memo-heading p,.memo-actions p{margin:0;color:var(--muted);font-size:13px;line-height:1.45}.source-link{margin-top:12px;padding:0;border:0;background:none;color:var(--primary);font:inherit;font-size:12px;font-weight:700;display:flex;gap:5px;align-items:center;text-align:left;cursor:pointer}.conflict-box{display:flex;gap:12px;padding:15px;border:1px solid #e9c66d;background:#fff8df;border-radius:12px;color:#654d12}.conflict-box strong{font-size:13px}.conflict-box p{margin:5px 0 0;font-size:12px;line-height:1.45}.template-list{display:grid;gap:8px;margin-top:10px}.template-row{width:100%;display:grid;grid-template-columns:auto 1fr auto;align-items:center;gap:11px;text-align:left;background:#f8fafc;border:1px solid var(--border);border-radius:9px;padding:11px;cursor:pointer;color:inherit}.template-row strong{display:flex;gap:8px;align-items:center}.template-row strong span{font-size:10px;color:var(--muted);background:#e9eef5;border-radius:4px;padding:2px 5px}.memo-heading{display:flex;justify-content:space-between;gap:20px;align-items:start}.memo-history-actions{display:flex;flex-wrap:wrap;gap:8px;justify-content:flex-end}.party-form{position:relative;margin-top:13px;padding:14px;background:#f8fafc;border:1px solid var(--border);border-radius:9px;display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:10px}.party-form label{font-size:11px;font-weight:700;color:var(--muted);display:grid;gap:5px}.party-form input,.party-form select,.party-form textarea{font:inherit;color:var(--text);border:1px solid var(--border);border-radius:7px;padding:8px;background:white}.party-form .wide{grid-column:span 3}.party-number{grid-column:span 3;font-size:12px;font-weight:800}.remove-party{position:absolute;right:10px;top:9px;border:0;background:none;color:#a33;cursor:pointer}.remove-party:disabled{opacity:.3;cursor:not-allowed}.memo-actions{display:flex;gap:14px;align-items:center;margin-top:13px}.memo-actions p{max-width:620px}@media(max-width:900px){.requirements-grid{grid-template-columns:1fr}.guide-heading,.memo-heading,.memo-actions{flex-direction:column}.memo-history-actions{justify-content:flex-start}.party-form{grid-template-columns:1fr}.party-form .wide,.party-number{grid-column:span 1}}
</style>
