<script setup lang="ts">
import { ref, useId } from 'vue'
import { Eye, EyeOff } from 'lucide-vue-next'

defineProps<{ label: string, autocomplete?: string, minlength?: number }>()
const value = defineModel<string>({ required: true })
const visible = ref(false)
const inputId = useId()
</script>

<template>
  <div class="password-entry">
    <label :for="inputId">{{ label }}</label>
    <div class="password-field">
      <input :id="inputId" v-model="value" :type="visible ? 'text' : 'password'"
             :autocomplete="autocomplete" :minlength="minlength" required/>
      <button type="button" class="password-toggle" :aria-label="(visible ? 'Hide ' : 'Show ') + label"
              :aria-pressed="visible" @click="visible = !visible">
        <EyeOff v-if="visible" :size="16"/><Eye v-else :size="16"/>
        <span>{{ visible ? 'Hide' : 'Show' }}</span>
      </button>
    </div>
  </div>
</template>
