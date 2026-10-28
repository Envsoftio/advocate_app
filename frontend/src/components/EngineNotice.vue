<script setup lang="ts">
import { engineState, engineBlocked, engineReason } from '../lib/engineConnection'
defineProps<{reviewLabel?:string}>()
defineEmits<{retry:[],review:[]}>()
</script>
<template>
  <div v-if="engineBlocked" class="engine-notice">
    <details>
      <summary><span role="status">{{engineReason}}</span></summary>
      <p>Reading and search can use PDF data still available in this window. Other pages, text and document information may be unavailable; the whole PDF and search index are not cached. Internet access is separate from the local engine.</p>
      <p>Unsaved drafts and reading positions stay in this window. Retry checks the actual engine and saved inputs. No edits, OCR or builds are replayed. After recovery, review and explicitly save any retained draft.</p>
    </details>
    <button v-if="engineState === 'changed'" class="btn secondary small" @click="$emit('review')">{{reviewLabel || 'Review latest state'}}</button>
    <button v-else class="btn secondary small" :disabled="engineState === 'checking'" @click="$emit('retry')">{{engineState === 'checking' ? 'Checking…' : 'Retry connection'}}</button>
  </div>
</template>
