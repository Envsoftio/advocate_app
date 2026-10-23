<script setup lang="ts">
import { ref, watch } from 'vue'
import { api } from '../lib/api'
import { friendlyError } from '../lib/format'

type Item = {title:string; part:number|null; status:'unreviewed'|'present'|'not_applicable'}
type Review = {court:string; case_type:string; stage:string; source_note:string; items:Item[]}
const empty = ():Review => ({court:'', case_type:'', stage:'', source_note:'', items:[]})
const props = defineProps<{matterId:string; refreshKey:number; disabled:boolean}>()
const emit = defineEmits<{modified:[]}>()
const draft = ref<Review>(empty())
const busy = ref(false)
const dirty = ref(false)
const error = ref('')
const message = ref('')
let sequence = 0
async function load() {
  const current = ++sequence
  try {
    const review = await api<Review>(`/matters/${props.matterId}/filing-review`)
    if (current === sequence) { draft.value = review; dirty.value = false; error.value = '' }
  } catch (err) { if (current === sequence) error.value = friendlyError(err) }
}
watch(() => props.matterId, () => {draft.value = empty(); void load()}, {immediate:true})
watch(() => props.refreshKey, () => {if (!dirty.value && !busy.value) void load()})
function changed() {dirty.value = true; message.value = ''}
function add() {
  if (draft.value.items.length >= 12) return
  draft.value.items.push({title:'', part:null, status:'unreviewed'})
  changed()
}
async function save() {
  if (busy.value || props.disabled) return
  busy.value = true
  error.value = ''
  message.value = ''
  try {
    draft.value = await api<Review>(`/matters/${props.matterId}/filing-review`, 'PUT', draft.value)
    dirty.value = false
    message.value = 'Review checklist saved. It is a user-authored aid, not a court rule.'
    emit('modified')
  } catch (err) {error.value = friendlyError(err)}
  finally {busy.value = false}
}
</script>

<template>
  <section class="panel filing-review" aria-label="User-authored filing review checklist">
    <div class="panel-header"><div><h2>Filing review checklist</h2><span>Optional items for this matter and filing stage</span></div></div>
    <div class="filing-review-body">
      <p class="hint">Enter the scope and items supplied by your reviewer or court instructions. This app does not supply a universal required-document list.</p>
      <div class="filing-review-scope">
        <label class="field">Court or forum<input v-model="draft.court" maxlength="120" placeholder="Enter the intended court" @input="changed"/></label>
        <label class="field">Case type<input v-model="draft.case_type" maxlength="120" placeholder="Enter the case type" @input="changed"/></label>
        <label class="field">Filing stage<input v-model="draft.stage" maxlength="120" placeholder="e.g. initial filing" @input="changed"/></label>
      </div>
      <label class="field">Source or reviewer note<input v-model="draft.source_note" maxlength="500" placeholder="Where did this checklist come from?" @input="changed"/></label>
      <div class="filing-review-items"><div v-for="(item,index) in draft.items" :key="index" class="filing-review-item">
        <label class="field">Item {{index+1}}<input v-model="item.title" maxlength="80" placeholder="Document or review item" @input="changed"/></label>
        <label class="field">Part<select v-model="item.part" @change="changed"><option :value="null">Any Part</option><option v-for="part in 4" :key="part" :value="part">Part {{part}}</option></select></label>
        <label class="field">Review<select v-model="item.status" @change="changed"><option value="unreviewed">To review</option><option value="present">Present, reviewer marked</option><option value="not_applicable">Not applicable, reviewer marked</option></select></label>
        <button class="btn secondary small" :disabled="busy || disabled" :aria-label="'Remove checklist item '+(index+1)" @click="draft.items.splice(index,1); changed()">Remove</button>
      </div></div>
      <div class="filing-review-actions"><button class="btn secondary small" :disabled="busy || disabled || draft.items.length>=12" @click="add">Add item</button>
        <button class="btn primary small" :disabled="busy || disabled || !dirty" @click="save">Save checklist</button></div>
      <p v-if="error" class="error-msg" role="alert">{{error}}</p><p v-if="message" role="status">{{message}}</p>
    </div>
  </section>
</template>
