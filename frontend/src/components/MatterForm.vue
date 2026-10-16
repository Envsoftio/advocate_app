<script setup lang="ts">
import { reactive, ref } from 'vue'
import { X, Plus, ShieldAlert } from 'lucide-vue-next'
import type { CourtProfile, MatterNew } from '../lib/types'

const props = defineProps<{ profiles: CourtProfile[], working: boolean }>()
const emit = defineEmits<{ close: [], create: [value: MatterNew] }>()
const form = reactive<MatterNew>({
  name: '', court_profile: 'general-demo', case_type: 'Demonstration filing',
  petitioner: '', respondent: '', advocate: '', case_number: '',
  max_pages_per_volume: 300, keep_groups_intact: true,
})
const hasVisited = ref(false)
function submit() {
  hasVisited.value = true
  if (form.name.trim().length < 2) return
  emit('create', { ...form, name: form.name.trim() })
}
</script>

<template>
  <div class="modal-backdrop" @click.self="emit('close')">
    <div class="modal-card" role="dialog" aria-modal="true" aria-label="Create matter">
      <div class="modal-head">
        <div><div class="eyebrow">NEW WORKSPACE</div><h2>Create a legal matter</h2></div>
        <button class="icon-btn" aria-label="Close" @click="emit('close')"><X :size="19" /></button>
      </div>
      <form @submit.prevent="submit" class="form-grid">
        <label class="field full">Matter name <span>*</span><input v-model="form.name" autofocus placeholder="Example: ABC Pvt Ltd v. XYZ" maxlength="200" /></label>
        <div v-if="hasVisited && form.name.trim().length < 2" class="field-error full">Enter a matter name (at least two characters).</div>
        <label class="field full">Court profile
          <select v-model="form.court_profile">
            <option v-for="profile in props.profiles" :key="profile.id" :value="profile.id">{{ profile.name }}</option>
          </select>
        </label>
        <label class="field full">Case type<input v-model="form.case_type" placeholder="For example: Writ petition - demonstration" /></label>
        <label class="field">Petitioner / Applicant<input v-model="form.petitioner" placeholder="Applicant name" /></label>
        <label class="field">Respondent<input v-model="form.respondent" placeholder="Respondent name" /></label>
        <label class="field">Advocate<input v-model="form.advocate" placeholder="Advocate name" /></label>
        <label class="field">Case no. (optional)<input v-model="form.case_number" placeholder="If already assigned" /></label>
        <div class="notice subtle full"><ShieldAlert :size="16" /> Court profiles are illustrative. This POC does not certify court compliance or submit filings.</div>
        <div class="modal-actions full">
          <button class="btn secondary" type="button" @click="emit('close')">Cancel</button>
          <button class="btn primary" type="submit" :disabled="props.working"><Plus :size="16" /> {{ props.working ? 'Creating…' : 'Create matter' }}</button>
        </div>
      </form>
    </div>
  </div>
</template>
