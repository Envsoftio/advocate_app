<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from 'vue'
import { api } from '../lib/api'
import { friendlyError } from '../lib/format'
import type { Matter, MatterDocument } from '../lib/types'

const props = defineProps<{matter:Matter; documents:MatterDocument[]; archivedDocuments:MatterDocument[]}>()
const emit = defineEmits<{close:[]; modified:[]}>()
const busy = ref(false)
const error = ref('')
const addButton = ref<HTMLButtonElement|null>(null)
const counts = computed(() => new Map(props.matter.parts.map(part => [part,
  props.documents.filter(doc => doc.part === part).length])))
const archivedCounts = computed(() => new Map(props.matter.parts.map(part => [part,
  props.archivedDocuments.filter(doc => doc.part === part).length])))
onMounted(async () => {await nextTick();addButton.value?.focus()})

async function add() {
  if (busy.value) return
  busy.value = true; error.value = ''
  try {await api(`/matters/${props.matter.id}/parts`, 'POST'); emit('modified')}
  catch (cause) {error.value = friendlyError(cause)}
  finally {busy.value = false}
}
async function remove(part:number) {
  if (busy.value || props.matter.parts.length === 1 || counts.value.get(part) || archivedCounts.value.get(part)) return
  busy.value = true; error.value = ''
  try {await api(`/matters/${props.matter.id}/parts/${part}`, 'DELETE'); emit('modified')}
  catch (cause) {error.value = friendlyError(cause)}
  finally {busy.value = false}
}
</script>

<template>
  <div class="part-manager-overlay" role="presentation" @click.self="emit('close')" @keydown.esc.stop.prevent="emit('close')">
    <section class="part-manager" role="dialog" aria-modal="true" aria-labelledby="part-manager-title">
      <header><div><h2 id="part-manager-title">Manage Parts</h2><p>Organize documents in this matter. Part numbers stay stable when a Part is removed.</p></div><button class="part-manager-close" aria-label="Close Manage Parts" @click="emit('close')">×</button></header>
      <div class="part-manager-list">
        <div v-for="part in matter.parts" :key="part" class="part-manager-row">
          <div><strong>Part {{part}}</strong><span>{{counts.get(part)}} {{counts.get(part)===1?'document':'documents'}}<template v-if="archivedCounts.get(part)"> · {{archivedCounts.get(part)}} archived</template></span></div>
          <button :aria-label="`Remove Part ${part}`" :disabled="busy || matter.parts.length===1 || !!counts.get(part) || !!archivedCounts.get(part)" :title="counts.get(part)||archivedCounts.get(part)?'Move or restore documents using this Part before removal':matter.parts.length===1?'A matter needs at least one Part':'Remove this empty Part'" @click="remove(part)">Remove</button>
        </div>
      </div>
      <p class="part-manager-note">To remove a Part with documents, first change their Part in Batch review. Archived documents and checklist items also keep a Part in use.</p>
      <p v-if="error" class="part-manager-error" role="alert">{{error}}</p>
      <footer><span>{{matter.parts.length}} {{matter.parts.length===1?'Part':'Parts'}} in this matter</span><button ref="addButton" class="btn primary small" :disabled="busy || matter.parts.length>=50 || matter.next_part>99" @click="add">{{busy?'Working…':`Add Part ${matter.next_part}`}}</button></footer>
    </section>
  </div>
</template>

<style scoped>
.part-manager-overlay{position:fixed;inset:0;z-index:110;display:grid;place-items:center;background:#0e1a2c99;padding:20px}
.part-manager{width:min(480px,100%);max-height:min(680px,calc(100vh - 40px));display:flex;flex-direction:column;overflow:hidden;border:1px solid var(--cb-border);border-radius:12px;background:var(--cb-surface);color:var(--cb-text);box-shadow:0 24px 70px #0004}
.part-manager>header{display:flex;align-items:flex-start;gap:12px;padding:20px 22px 14px;border-bottom:1px solid var(--cb-border)}
.part-manager h2{margin:0;font-size:19px;line-height:26px}.part-manager header p{margin:5px 0 0;color:var(--cb-muted);font-size:12px;line-height:1.5}
.part-manager-close{margin-left:auto;border:0;background:transparent;color:var(--cb-muted);font-size:24px;line-height:24px;cursor:pointer}
.part-manager-list{overflow:auto;padding:8px 22px;min-height:70px}
.part-manager-row{display:flex;align-items:center;justify-content:space-between;gap:12px;min-height:54px;border-bottom:1px solid var(--cb-border)}
.part-manager-row:last-child{border-bottom:0}.part-manager-row>div{display:flex;align-items:baseline;gap:10px}.part-manager-row strong{font-size:13px}.part-manager-row span{font-size:12px;color:var(--cb-muted)}
.part-manager-row button{border:0;background:transparent;color:var(--cb-link);font:inherit;font-size:12px;cursor:pointer}.part-manager-row button:disabled{color:var(--cb-muted);opacity:.65;cursor:not-allowed}
.part-manager-note{margin:0;padding:10px 22px 15px;color:var(--cb-muted);font-size:12px;line-height:1.55}
.part-manager-error{margin:0 22px 12px;padding:9px 11px;border-radius:7px;background:var(--cb-dangerBg);color:var(--cb-danger);font-size:12px}
.part-manager footer{display:flex;align-items:center;justify-content:space-between;gap:12px;padding:14px 22px;border-top:1px solid var(--cb-border);font-size:12px;color:var(--cb-muted)}
</style>
