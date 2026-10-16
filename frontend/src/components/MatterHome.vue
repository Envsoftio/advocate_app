<script setup lang="ts">
import { computed, ref } from 'vue'
import { ArrowRight, BriefcaseBusiness, Plus, Search, WandSparkles } from 'lucide-vue-next'
import type { Matter } from '../lib/types'
import { prettyDate } from '../lib/format'
const props = defineProps<{ matters: Matter[], creating: boolean }>()
const emit = defineEmits<{ open: [id: string], create: [], sample: [] }>()
const query = ref('')
const sort = ref('recent')
const visible = computed(() => props.matters.filter(m =>
  [m.name, m.case_number, m.case_type, m.petitioner, m.respondent].join(' ').toLowerCase().includes(query.value.trim().toLowerCase())
).sort((a,b) => sort.value === 'name' ? a.name.localeCompare(b.name) : b.updated_at.localeCompare(a.updated_at)))
</script>

<template>
  <main id="main-content" tabindex="-1" class="page-content matter-home">
    <header class="home-heading"><div><div class="eyebrow">LOCAL WORKSPACE</div><h1>Your matters</h1><p>Find a case, pick up your review, or prepare a new bundle.</p></div><button class="btn primary" @click="emit('create')"><Plus :size="17"/> New matter</button></header>
    <div class="home-search-row"><label class="home-search"><Search :size="19"/><input v-model="query" aria-label="Search matters" placeholder="Search by matter, case number or party…"/></label><select v-model="sort" aria-label="Sort matters"><option value="recent">Recently updated</option><option value="name">Matter name</option></select></div>
    <div class="home-list-heading"><strong>{{visible.length}} {{visible.length === 1 ? 'matter' : 'matters'}}</strong><span>Stored on this computer</span></div>
    <div v-if="visible.length" class="matter-cards">
      <button v-for="matter in visible" :key="matter.id" class="matter-home-card" @click="emit('open', matter.id)">
        <div class="matter-card-top"><BriefcaseBusiness :size="20"/><span>{{matter.case_number || 'Draft · no case number'}}</span><ArrowRight :size="18"/></div>
        <h2>{{matter.name}}</h2><p>{{matter.case_type || 'Case type not specified'}}</p>
        <div class="matter-card-parties">{{matter.petitioner || 'Petitioner not entered'}}<span>vs.</span>{{matter.respondent || 'Respondent not entered'}}</div>
        <footer><span>Updated {{prettyDate(matter.updated_at)}}</span><strong>Open matter</strong></footer>
      </button>
    </div>
    <div v-else class="home-empty panel"><BriefcaseBusiness :size="30"/><h2>{{query ? 'No matching matters' : 'Start your first matter'}}</h2><p>{{query ? 'Try a different name, case number or party.' : 'Keep source documents, review notes and generated bundles together.'}}</p><button v-if="query" class="btn secondary" @click="query=''">Clear search</button><button v-else class="btn primary" @click="emit('create')">Create your first matter</button></div>
    <div class="home-sample"><div><strong>Explore with a sample</strong><p>Try the preparation workflow using a separate synthetic matter.</p></div><button class="btn secondary" :disabled="creating" @click="emit('sample')"><WandSparkles :size="16"/> Open sample matter</button></div>
  </main>
</template>
