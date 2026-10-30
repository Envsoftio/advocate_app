<script setup lang="ts">
import { reactive, ref, onMounted, onUnmounted } from 'vue'
import { X, Folder, HardDrive } from 'lucide-vue-next'
import type { CourtProfile, MatterNew } from '../lib/types'

const props = defineProps<{ profiles: CourtProfile[], working: boolean }>()
const emit = defineEmits<{ close: [], create: [value: MatterNew] }>()
const form = reactive<MatterNew>({
  name: '', court_forum: '', client_reference: '', internal_reference: '', notes: '',
  court_profile: 'general-demo', case_type: '',
  petitioner: '', respondent: '', advocate: '', case_number: '',
  max_pages_per_volume: 300, keep_groups_intact: true,
})
const visited = ref(false)
const storageInfo = ref(false)
const dialog = ref<HTMLElement | null>(null)
const opener = document.activeElement as HTMLElement | null
function close() { if (!props.working) emit('close') }
function submit() {
  visited.value = true
  if (form.name.trim().length < 2 || props.working) return
  emit('create', { ...form, name: form.name.trim() })
}
function keydown(event: KeyboardEvent) {
  if (event.key === 'Escape') { event.preventDefault(); close() }
  if (event.key !== 'Tab' || !dialog.value) return
  const focusable = [...dialog.value.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), summary')]
  if (!focusable.length) return
  const first = focusable[0], last = focusable[focusable.length - 1]
  if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus() }
  else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus() }
}
onMounted(() => dialog.value?.querySelector<HTMLInputElement>('input[name="matter-name"]')?.focus())
onUnmounted(() => opener?.focus())
</script>

<template>
  <div class="modal-backdrop new-matter-backdrop" @click.self="close">
    <section ref="dialog" class="new-matter-dialog" role="dialog" aria-modal="true" aria-label="Create matter" aria-describedby="new-matter-description" @keydown="keydown">
      <header class="new-matter-head">
        <h2 id="new-matter-title">New matter</h2>
        <button class="icon-btn" type="button" aria-label="Close new matter" @click="close"><X :size="18" /></button>
      </header>
      <p id="new-matter-description">Create a local workspace for the documents and packages in this matter.</p>
      <form class="new-matter-form" @submit.prevent="submit">
        <label class="new-matter-field" for="matter-name">Matter name <span aria-hidden="true">*</span></label>
        <input id="matter-name" name="matter-name" v-model="form.name" maxlength="200" :aria-invalid="visited && form.name.trim().length < 2" :aria-describedby="visited && form.name.trim().length < 2 ? 'matter-name-error' : undefined" placeholder="e.g. Sharma v. State of Maharashtra" />
        <p v-if="visited && form.name.trim().length < 2" id="matter-name-error" class="field-error" role="alert">Enter a matter name of at least two characters.</p>
        <div class="new-matter-pair">
          <div><label class="new-matter-field" for="matter-forum">Court / forum</label><input id="matter-forum" v-model="form.court_forum" maxlength="300" placeholder="Enter court, tribunal or forum" /><small>Enter any court, tribunal or forum.</small></div>
          <div><label class="new-matter-field" for="matter-case-number">Case number</label><input id="matter-case-number" v-model="form.case_number" maxlength="100" placeholder="If already assigned" /></div>
        </div>
        <div class="new-matter-pair references">
          <div><label class="new-matter-field" for="matter-client">Client / party reference</label><input id="matter-client" v-model="form.client_reference" maxlength="300" placeholder="Optional reference" /></div>
          <div><label class="new-matter-field" for="matter-internal">Internal reference</label><input id="matter-internal" v-model="form.internal_reference" maxlength="200" placeholder="Optional reference" /></div>
        </div>
        <label class="new-matter-field" for="matter-notes">Notes</label>
        <textarea id="matter-notes" v-model="form.notes" maxlength="2000" rows="2" placeholder="Optional notes for this matter" />
        <div class="new-matter-storage-label">Storage location</div>
        <div class="new-matter-storage"><span><Folder :size="16" /> App-managed local workspace</span><button class="btn secondary" type="button" :aria-expanded="storageInfo" @click="storageInfo=!storageInfo">Details…</button></div>
        <p v-if="storageInfo" class="new-matter-storage-info">MatterDraft stores each matter in its configured local app workspace. Use Storage after creation to back up or export it.</p>
        <p class="new-matter-local"><HardDrive :size="16" /> Files and processing stay on this computer.</p>
        <details class="new-matter-advanced"><summary>Preparation settings</summary><label for="matter-profile">Court profile</label><select id="matter-profile" v-model="form.court_profile"><option v-for="profile in profiles" :key="profile.id" :value="profile.id">{{ profile.name }}</option></select><p>Profiles are illustrative and do not certify compliance.</p><label for="matter-case-type">Case type</label><input id="matter-case-type" v-model="form.case_type" maxlength="150" /></details>
        <footer class="new-matter-actions"><span>* Required</span><button class="btn secondary" type="button" @click="close">Cancel</button><button class="btn primary" type="submit" :disabled="working">{{ working ? 'Creating…' : 'Create matter' }}</button></footer>
      </form>
    </section>
  </div>
</template>
