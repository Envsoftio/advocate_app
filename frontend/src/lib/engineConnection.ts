import { computed, ref } from 'vue'

// A successful health request alone must never unlock writes after a failure.
export const engineState = ref<'ready' | 'disconnected' | 'checking' | 'changed'>('ready')
export const engineBlocked = computed(() => engineState.value !== 'ready')
export const autosaveHeld = ref(false)
export const engineReviewReason = ref('Saved inputs changed. Review the latest state before editing.')
export const engineReason = computed(() => engineState.value === 'changed'
  ? engineReviewReason.value
  : engineState.value === 'checking' ? 'Checking the local engine and saved revisions…'
  : 'Local engine unavailable. Saves, OCR, page edits and builds are paused.')
export function disconnectEngine() {
  engineState.value = 'disconnected'
  autosaveHeld.value = true
}
export function requireEngine() {
  if (engineBlocked.value) throw new Error(engineReason.value)
}
