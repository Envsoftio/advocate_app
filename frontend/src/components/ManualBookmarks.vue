<script setup lang="ts">
import { ref, watch } from 'vue'
import { api } from '../lib/api'
import { friendlyError } from '../lib/format'
import type { MatterDocument } from '../lib/types'

interface Bookmark {
  id: string
  title: string
  document_id: string
  page_index: number
  target_revision: number
  parent_id: string | null
  position: number
  resolved: boolean
}
const props = defineProps<{ matterId: string, documents: MatterDocument[], refreshKey?: number, disabled: boolean }>()
const emit = defineEmits<{ modified: [] }>()
const entries = ref<Bookmark[]>([])
const title = ref('')
const documentId = ref('')
const page = ref(1)
const parentId = ref('')
const error = ref('')
const busy = ref(false)
const dragged = ref<string | null>(null)

watch(() => props.matterId, async id => {
  error.value = ''
  entries.value = await api<Bookmark[]>(`/matters/${id}/bookmarks`).catch(err => {
    error.value = friendlyError(err)
    return []
  })
}, { immediate: true })
watch(() => props.documents.map(doc => `${doc.id}:${doc.revision}:${doc.page_count}`).join('|'),
  async () => { entries.value = await api<Bookmark[]>(`/matters/${props.matterId}/bookmarks`).catch(() => entries.value) })
watch(() => props.refreshKey, async () => {
  if (!busy.value) entries.value = await api<Bookmark[]>(`/matters/${props.matterId}/bookmarks`).catch(() => entries.value)
})

function serialise(values: Bookmark[]) {
  return values.map(({id, title, document_id, page_index, target_revision, parent_id}) =>
    ({id, title, document_id, page_index, target_revision, parent_id}))
}
async function save(values: Bookmark[]) {
  if (busy.value || props.disabled) return
  busy.value = true
  error.value = ''
  try {
    entries.value = await api<Bookmark[]>(`/matters/${props.matterId}/bookmarks`, 'PUT',
      {entries: serialise(values)})
    emit('modified')
  } catch (err) { error.value = friendlyError(err) }
  finally { busy.value = false }
}
function add() {
  const doc = props.documents.find(item => item.id === documentId.value)
  if (!doc || !title.value.trim() || !Number.isInteger(page.value) || page.value < 1 || page.value > doc.page_count) {
    error.value = 'Enter a title and a page within the selected document.'
    return
  }
  const entry: Bookmark = {id: crypto.randomUUID().replace(/-/g, ''), title: title.value.trim(),
    document_id: doc.id, page_index: page.value - 1, target_revision: doc.revision,
    parent_id: parentId.value || null, position: entries.value.length, resolved: true}
  void save([...entries.value, entry])
  title.value = ''
}
function change(index: number, changes: Partial<Bookmark>) {
  const updated = entries.value.map((entry, i) => i === index ? {...entry, ...changes} : entry)
  void save(updated)
}
function remove(index: number) {
  const removed = entries.value[index]
  void save(entries.value.filter(entry => entry.id !== removed.id).map(entry =>
    entry.parent_id === removed.id ? {...entry, parent_id: removed.parent_id} : entry))
}
function move(from: number, to: number) {
  if (from < 0 || to < 0 || from >= entries.value.length || to >= entries.value.length || from === to) return
  const updated = [...entries.value]
  updated.splice(to, 0, updated.splice(from, 1)[0])
  void save(updated)
}
function drop(targetId: string) {
  if (dragged.value) move(entries.value.findIndex(x => x.id === dragged.value),
                          entries.value.findIndex(x => x.id === targetId))
  dragged.value = null
}
function resolve(index: number) {
  const entry = entries.value[index]
  const doc = props.documents.find(item => item.id === entry.document_id)
  if (!doc) { error.value = 'Restore the document before resolving this bookmark.'; return }
  change(index, {target_revision: doc.revision,
                 page_index: Math.min(entry.page_index, doc.page_count - 1)})
}
</script>

<template>
  <section class="card" aria-label="Manual bookmarks">
    <h3>Manual bookmarks</h3>
    <p>Targets use a document page and revision. A page edit marks older targets unresolved until you review them. Generated index bookmarks remain separate.</p>
    <div v-for="(entry, index) in entries" :key="entry.id" class="manual-bookmark-row"
         draggable="true" @dragstart="dragged=entry.id" @dragover.prevent @drop.prevent="drop(entry.id)">
      <span aria-hidden="true">⋮⋮</span>
      <input :value="entry.title" :aria-label="'Bookmark title '+(index+1)"
             :disabled="busy || disabled" @change="change(index,{title:($event.target as HTMLInputElement).value.trim()})"/>
      <span>{{documents.find(doc => doc.id === entry.document_id)?.title || 'Archived document'}} · p{{entry.page_index+1}}</span>
      <span v-if="!entry.resolved" role="status">Needs review</span>
      <button v-if="!entry.resolved" class="btn secondary small" :disabled="busy || disabled" @click="resolve(index)">Resolve to current page</button>
      <select :value="entry.parent_id || ''" :aria-label="'Parent bookmark '+(index+1)" :disabled="busy || disabled"
              @change="change(index,{parent_id:($event.target as HTMLSelectElement).value || null})">
        <option value="">Top level</option>
        <option v-for="parent in entries.slice(0,index)" :key="parent.id" :value="parent.id">Under {{parent.title}}</option>
      </select>
      <button class="btn secondary small" :aria-label="'Move bookmark '+(index+1)+' up'" :disabled="busy || disabled || index===0" @click="move(index,index-1)">↑</button>
      <button class="btn secondary small" :aria-label="'Move bookmark '+(index+1)+' down'" :disabled="busy || disabled || index===entries.length-1" @click="move(index,index+1)">↓</button>
      <button class="btn secondary small" :aria-label="'Delete bookmark '+(index+1)" :disabled="busy || disabled" @click="remove(index)">Delete</button>
    </div>
    <div class="manual-bookmark-row">
      <input v-model="title" aria-label="New bookmark title" placeholder="Bookmark title" maxlength="200"/>
      <select v-model="documentId" aria-label="Bookmark document"><option value="">Choose document</option>
        <option v-for="doc in documents" :key="doc.id" :value="doc.id">{{doc.title}}</option></select>
      <input v-model.number="page" aria-label="Bookmark page" type="number" min="1"
             :max="documents.find(doc => doc.id === documentId)?.page_count || 1"/>
      <select v-model="parentId" aria-label="New bookmark parent"><option value="">Top level</option>
        <option v-for="entry in entries" :key="entry.id" :value="entry.id">Under {{entry.title}}</option></select>
      <button class="btn secondary small" :disabled="busy || disabled" @click="add">Add bookmark</button>
    </div>
    <p v-if="error" class="error-msg" role="alert">{{error}}</p>
  </section>
</template>
