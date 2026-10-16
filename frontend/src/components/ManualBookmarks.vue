<script setup lang="ts">
import { nextTick, ref, watch } from 'vue'
import { GripVertical } from 'lucide-vue-next'
import { VueDraggable, type SortableEvent } from 'vue-draggable-plus'
import { api } from '../lib/api'
import { friendlyError } from '../lib/format'
import type { ManualBookmark as Bookmark, MatterDocument } from '../lib/types'
const props = defineProps<{ matterId: string, documents: MatterDocument[], refreshKey?: number, disabled: boolean,
  preset?: {documentId:string,sequence:number} }>()
const emit = defineEmits<{ modified: [] }>()
const entries = ref<Bookmark[]>([])
const title = ref('')
const titleInput = ref<HTMLInputElement | null>(null)
const documentId = ref('')
const page = ref(1)
const parentId = ref('')
const error = ref('')
const busy = ref(false)
let bookmarkOrderBeforeDrag: Bookmark[] = []

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
watch(() => props.preset?.sequence, async () => {
  const doc = props.documents.find(item => item.id === props.preset?.documentId)
  if (!doc) return
  documentId.value = doc.id
  page.value = 1
  parentId.value = ''
  title.value = (doc.label ? doc.label + ' · ' : '') + doc.title
  await nextTick()
  titleInput.value?.focus()
})

function serialise(values: Bookmark[]) {
  return values.map(({id, title, document_id, page_index, target_revision, parent_id}) =>
    ({id, title, document_id, page_index, target_revision, parent_id}))
}
async function save(values: Bookmark[]): Promise<boolean> {
  if (busy.value || props.disabled) return false
  busy.value = true
  error.value = ''
  try {
    entries.value = await api<Bookmark[]>(`/matters/${props.matterId}/bookmarks`, 'PUT',
      {entries: serialise(values)})
    emit('modified')
    return true
  } catch (err) { error.value = friendlyError(err); return false }
  finally { busy.value = false }
}
async function add() {
  const doc = props.documents.find(item => item.id === documentId.value)
  if (!doc || !title.value.trim() || !Number.isInteger(page.value) || page.value < 1 || page.value > doc.page_count) {
    error.value = 'Enter a title and a page within the selected document.'
    return
  }
  const entry: Bookmark = {id: crypto.randomUUID().replace(/-/g, ''), title: title.value.trim(),
    document_id: doc.id, page_index: page.value - 1, target_revision: doc.revision,
    parent_id: parentId.value || null, position: entries.value.length, resolved: true}
  if (await save([...entries.value, entry])) title.value = ''
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
function descendantsOf(id: string, candidate: Bookmark, byId: Map<string, Bookmark>): boolean {
  let parent = candidate.parent_id
  while (parent) {
    if (parent === id) return true
    parent = byId.get(parent)?.parent_id || null
  }
  return false
}
function siblingIndex(index: number, step: number): number {
  const entry = entries.value[index]
  if (!entry) return -1
  const siblings = entries.value.filter(item => item.parent_id === entry.parent_id)
  const position = siblings.findIndex(item => item.id === entry.id)
  return entries.value.findIndex(item => item.id === siblings[position + step]?.id)
}
function reorder(sourceId: string, targetId: string, after: boolean) {
  if (sourceId === targetId || busy.value || props.disabled) return
  const source = entries.value.find(item => item.id === sourceId)
  const target = entries.value.find(item => item.id === targetId)
  if (!source || !target) return
  if (source.parent_id !== target.parent_id) {
    error.value = 'Drag bookmarks within the same level. Use Parent bookmark to change nesting.'
    return
  }
  const byId = new Map(entries.value.map(item => [item.id, item]))
  const block = entries.value.filter(item => item.id === sourceId || descendantsOf(sourceId, item, byId))
  const remaining = entries.value.filter(item => !block.includes(item))
  const targetBlock = remaining.filter(item => item.id === targetId || descendantsOf(targetId, item, byId))
  const position = remaining.findIndex(item => item.id === targetId) + (after ? targetBlock.length : 0)
  remaining.splice(position, 0, ...block)
  void save(remaining)
}
function move(index: number, step: number) {
  const targetIndex = siblingIndex(index, step)
  if (targetIndex < 0) return
  reorder(entries.value[index].id, entries.value[targetIndex].id, step > 0)
}
function startBookmarkSort() {
  bookmarkOrderBeforeDrag = entries.value.map(entry => ({...entry}))
}
function endBookmarkSort(event: SortableEvent) {
  const before = bookmarkOrderBeforeDrag
  bookmarkOrderBeforeDrag = []
  if (!before.length) return
  const oldIndex = event.oldIndex
  const newIndex = event.newIndex
  const sourceId = event.item.dataset.bookmarkId ?? (oldIndex === undefined ? undefined : before[oldIndex]?.id)
  const targetId = newIndex === undefined ? undefined : before[newIndex]?.id
  entries.value = before
  if (oldIndex === undefined || newIndex === undefined || !sourceId || !targetId || sourceId === targetId) {
    entries.value = [...before]
    return
  }
  reorder(sourceId, targetId, newIndex > oldIndex)
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
  <section class="card manual-bookmarks" aria-label="Manual bookmarks">
    <div class="manual-bookmark-heading"><div><h3>Manual bookmarks <span class="bookmark-count">{{entries.length}}</span></h3><p>Drag a handle or focus it and press ↑/↓ to reorder within the same level. Choose a parent to nest one.</p></div></div>
    <p v-if="!entries.length" class="bookmark-empty">No manual bookmarks yet. Mark a page in the document preview, or add one below.</p>
    <VueDraggable v-model="entries" class="manual-bookmark-list" item-key="id" handle=".bookmark-drag-handle" :disabled="busy || disabled" :animation="160" :force-fallback="true" :fallback-tolerance="5" :fallback-on-body="true" ghost-class="sortable-ghost" chosen-class="sortable-chosen" drag-class="sortable-drag" fallback-class="sortable-fallback" @start="startBookmarkSort" @end="endBookmarkSort">
    <div v-for="(entry, index) in entries" :key="entry.id" class="manual-bookmark-row" :data-bookmark-id="entry.id">
      <span class="grip bookmark-drag-handle" role="button" :tabindex="busy || disabled ? -1 : 0" :aria-label="'Reorder bookmark '+(index+1)+': '+entry.title+'. Press Up or Down arrow keys.'" title="Drag or press Up/Down to reorder" @keydown.up.prevent="move(index,-1)" @keydown.down.prevent="move(index,1)"><GripVertical :size="17"/></span>
      <div class="bookmark-row-main">
        <div class="bookmark-title-line"><span class="bookmark-number">{{index+1}}</span><input :value="entry.title" :aria-label="'Bookmark title '+(index+1)" maxlength="200"
             :disabled="busy || disabled" @change="change(index,{title:($event.target as HTMLInputElement).value.trim()})"/><span v-if="!entry.resolved" class="bookmark-needs-review" role="status">Needs review</span></div>
        <div class="bookmark-target">{{documents.find(doc => doc.id === entry.document_id)?.title || 'Archived document'}} · source page {{entry.page_index+1}}</div>
      </div>
      <div class="bookmark-row-actions"><label>Parent<select :value="entry.parent_id || ''" :aria-label="'Parent bookmark '+(index+1)" :disabled="busy || disabled"
              @change="change(index,{parent_id:($event.target as HTMLSelectElement).value || null})">
        <option value="">Top level</option>
        <option v-for="parent in entries.slice(0,index)" :key="parent.id" :value="parent.id">Under {{parent.title}}</option>
      </select></label>
      <button v-if="!entry.resolved" class="btn secondary small" :disabled="busy || disabled" @click="resolve(index)">Resolve to current page</button>
      <button class="bookmark-delete" :aria-label="'Delete bookmark '+(index+1)" :disabled="busy || disabled" @click="remove(index)">Delete</button></div>
    </div>
    </VueDraggable>
    <div class="bookmark-create"><h4>Add bookmark</h4><div class="bookmark-create-fields">
      <label>Title<input ref="titleInput" v-model="title" aria-label="New bookmark title" placeholder="What should the bookmark say?" maxlength="200"/></label>
      <label>Document<select v-model="documentId" aria-label="Bookmark document"><option value="">Choose document</option>
        <option v-for="doc in documents" :key="doc.id" :value="doc.id">{{doc.title}}</option></select></label>
      <label>Page<input v-model.number="page" aria-label="Bookmark page" type="number" min="1"
             :max="documents.find(doc => doc.id === documentId)?.page_count || 1"/></label>
      <label>Parent<select v-model="parentId" aria-label="New bookmark parent"><option value="">Top level</option>
        <option v-for="entry in entries" :key="entry.id" :value="entry.id">Under {{entry.title}}</option></select></label>
      <button class="btn primary small" :disabled="busy || disabled" @click="add">Add bookmark</button></div>
    </div>
    <p v-if="error" class="error-msg" role="alert">{{error}}</p>
  </section>
</template>
