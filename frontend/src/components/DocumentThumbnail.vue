<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from 'vue'
import { FileText } from 'lucide-vue-next'
import { documentThumbnailBlob } from '../lib/api'

const props = defineProps<{ documentId: string, revision: number }>()
const frame = ref<HTMLElement | null>(null)
const imageUrl = ref('')
const failed = ref(false)
let observer: IntersectionObserver | null = null
let controller: AbortController | null = null
let sequence = 0

function clearImage() {
  if (imageUrl.value) URL.revokeObjectURL(imageUrl.value)
  imageUrl.value = ''
}
async function load() {
  if (imageUrl.value || controller || failed.value) return
  const current = ++sequence
  const pending = new AbortController()
  controller = pending
  try {
    const blob = await documentThumbnailBlob(props.documentId, props.revision, pending.signal)
    if (current === sequence) imageUrl.value = URL.createObjectURL(blob)
  } catch (error) {
    if (current === sequence && !pending.signal.aborted) failed.value = true
  } finally {
    if (current === sequence) controller = null
  }
}
function observe() {
  observer?.disconnect()
  if (!frame.value) return
  observer = new IntersectionObserver(entries => {
    if (entries.some(entry => entry.isIntersecting)) {
      observer?.disconnect()
      void load()
    }
  }, { rootMargin: '180px' })
  observer.observe(frame.value)
}
watch(frame, observe)
watch(() => `${props.documentId}:${props.revision}`, () => {
  sequence++
  controller?.abort()
  controller = null
  clearImage()
  failed.value = false
  observe()
})
onBeforeUnmount(() => {
  sequence++
  observer?.disconnect()
  controller?.abort()
  clearImage()
})
</script>

<template>
  <div ref="frame" class="document-thumbnail" :data-loaded="!!imageUrl">
    <img v-if="imageUrl" :src="imageUrl" alt="" />
    <FileText v-else :size="25" aria-hidden="true" />
  </div>
</template>
