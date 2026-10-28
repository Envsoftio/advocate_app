<script setup lang="ts">
import {computed,nextTick,onBeforeUnmount,onMounted,ref} from 'vue'
import {X,Save,Info} from 'lucide-vue-next'
import {api} from '../lib/api'
import {friendlyError} from '../lib/format'
import {differences,displaySetting,presetMarks,type BundlePreset,type SavedLayout} from '../lib/bundleSettings'
const props=defineProps<{matterId:string;settings:SavedLayout;dirty:boolean;disabled:boolean}>()
const emit=defineEmits<{close:[];applied:[];cover:[]}>()
const dialog=ref<HTMLDialogElement|null>(null),presets=ref<BundlePreset[]>([]),selectedId=ref(''),name=ref(''),error=ref(''),notice=ref(''),busy=ref(false),loading=ref(true),preserve=ref(true)
const selected=computed(()=>presets.value.find(p=>p.id===selectedId.value))
const overrides=computed(()=>differences(props.settings,props.settings.applied))
let returnFocus:HTMLElement|null=null,alive=true
async function load(){loading.value=true;error.value='';try{const items=await api<BundlePreset[]>('/output-presets');if(!alive)return;presets.value=items;selectedId.value ||= props.settings.applied?.id||items[0]?.id||''}catch(e){if(alive)error.value=friendlyError(e)}finally{if(alive)loading.value=false}}
async function save(){if(busy.value||props.dirty||!name.value.trim())return;busy.value=true;error.value='';try{const p=await api<BundlePreset>('/output-presets','POST',{name:name.value.trim(),max_pages_per_volume:props.settings.max_pages_per_volume,keep_groups_intact:props.settings.keep_groups_intact,output_marks:presetMarks(props.settings.output_marks)});if(!alive)return;await load();selectedId.value=p.id;notice.value=`${p.name} v${p.version} saved locally. Select Use selected preset to apply it.`}catch(e){if(alive)error.value=friendlyError(e)}finally{if(alive)busy.value=false}}
async function apply(){if(!selected.value||props.dirty||busy.value)return;busy.value=true;error.value='';try{await api(`/matters/${props.matterId}/output-preset/${selectedId.value}`,'POST',{preserve_overrides:preserve.value,expected_snapshot:props.settings.input_snapshot});if(alive)emit('applied')}catch(e){if(alive)error.value=friendlyError(e)}finally{if(alive)busy.value=false}}
function close(){if(!busy.value)emit('close')}
onMounted(()=>{returnFocus=document.activeElement as HTMLElement;dialog.value?.showModal();void load()})
onBeforeUnmount(()=>{alive=false;dialog.value?.close();void nextTick(()=>returnFocus?.isConnected&&returnFocus.focus())})
</script>
<template>
<dialog ref="dialog" class="bundle-preset-dialog" aria-labelledby="preset-title" @cancel.prevent="close">
 <header><h2 id="preset-title">Bundle presets</h2><button class="icon-btn" aria-label="Close presets" :disabled="busy" @click="close"><X :size="18"/></button><p>Choose reusable settings, or save this bundle’s layout as a new preset.</p></header>
 <div class="preset-dialog-grid">
  <aside class="structure-card preset-list"><h3>Saved presets</h3><p v-if="loading" role="status">Loading local presets…</p><p v-else-if="!presets.length">No saved presets yet. Name the current layout to save your first version.</p><label v-for="preset in presets" :key="preset.id" :class="{selected:selectedId===preset.id}"><input v-model="selectedId" type="radio" name="bundle-preset" :value="preset.id" :disabled="busy"/><span><strong>{{preset.name}}</strong><small>Version {{preset.version}} · {{preset.settings.output_marks.front_matter==='cover_index'?'Cover + linked index':'Linked index'}}<br>{{preset.settings.max_pages_per_volume}} pages per volume</small></span></label><p class="preset-local-count">{{presets.length}} local preset versions<br>No source files or party details are stored.</p><button class="membership-button quiet" disabled title="Audience variants are outside B02A">Manage variants · unavailable</button></aside>
  <section class="structure-card preset-selection"><header><h3>{{selected?.name||'Current matter settings'}}</h3><span v-if="selected" class="structure-badge"><Info :size="14"/>Selected · v{{selected.version}}</span></header>
   <dl v-if="selected"><div><dt>Cover &amp; index</dt><dd><button class="text-tool" @click="emit('cover')">{{selected.settings.output_marks.front_matter==='cover_index'?'Matter cover + linked index':'Linked index'}}</button></dd></div><div><dt>Printed pages</dt><dd>Physical pages; restart per volume</dd></div><div><dt>Front matter</dt><dd>{{selected.settings.output_marks.front_matter==='cover_index'?'Cover + generated index in each volume':'Generated index in each volume'}}</dd></div><div><dt>Navigation</dt><dd>Index links, bookmarks and PDF labels</dd></div><div><dt>Volume limits</dt><dd>{{selected.settings.max_pages_per_volume}} physical pages · {{selected.settings.keep_groups_intact?'Keep groups together':'Groups may split'}}</dd></div></dl>
   <p v-if="selected" class="preset-provenance">Local user preset · {{selected.created_at?new Date(selected.created_at).toLocaleString():'Retained version'}} · Technical preferences; no court approval</p>
   <section class="preset-save"><h3>Save current settings as a preset</h3><label>Preset name<div><input v-model="name" maxlength="80" placeholder="Layout name" :disabled="busy"/><button class="membership-button secondary" :disabled="busy||disabled||dirty||!name.trim()" @click="save"><Save :size="16"/>Save preset</button></div></label><label class="structure-check"><input type="checkbox" checked disabled/>Include structure and style only</label></section>
   <details v-if="overrides.length" class="preset-differences" open><summary>{{overrides.length}} field override(s) from {{settings.applied?.name}} v{{settings.applied?.version}}</summary><ul><li v-for="item in overrides" :key="item.field"><strong>{{item.label}}:</strong> {{displaySetting(item.baseline)}} → {{displaySetting(item.current)}}</li></ul><label class="structure-check"><input v-model="preserve" type="checkbox" :disabled="busy"/>Keep these overrides when applying the selected preset</label></details>
   <p v-if="dirty" role="status">Apply or cancel your unsaved settings before saving or using a preset.</p><p v-if="error" role="alert" class="structure-error">{{error}} <button v-if="!presets.length" class="text-tool" @click="load">Retry loading</button></p><p v-if="notice" role="status">{{notice}}</p>
   <p class="preset-footnote">Applying another preset recalculates the current map. Earlier builds keep their settings. Cover details and document exceptions stay with this matter.</p>
  </section>
 </div>
 <footer><button class="membership-button secondary" :disabled="busy" @click="close">Cancel</button><button class="membership-button primary" :disabled="busy||disabled||dirty||!selected" @click="apply">{{busy?'Saving…':error&&selected?'Retry selected preset':'Use selected preset'}}</button></footer>
</dialog>
</template>
