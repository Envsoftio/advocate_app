<script setup lang="ts">
import RevisionHistoryDialog from './RevisionHistoryDialog.vue';
import AppSelect from './ui/AppSelect.vue';
import AppConfirmDialog from './ui/AppConfirmDialog.vue';
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue';
import {
  Check,
  CircleCheck,
  Minus,
  Package,
  FileText,
  AlertCircle,
  X,
  ArrowRight,
  Circle,
  CircleX,
  LoaderCircle,
} from 'lucide-vue-next';
import { api, inTauri, revealSavedExport } from '../lib/api';
import { bytesLabel, prettyDate, friendlyError } from '../lib/format';
import type { ExportManifest, Job, MatterDocument, CourtProfile, BuildFailure } from '../lib/types';
import OutputChecksDialog from './OutputChecksDialog.vue';
import { useOutputValidation } from '../lib/outputValidation';
import { differences, type SavedLayout } from '../lib/bundleSettings';
import SupplementWorkspace from './SupplementWorkspace.vue';
import BuildComparison from './BuildComparison.vue';
const props = defineProps<{
  matterId: string;
  documents: MatterDocument[];
  artifacts: ExportManifest[];
  job: Job | null;
  busy: boolean;
  blocked: boolean;
  cancelPending: boolean;
  maxPages: number;
  keepGroups: boolean;
  refreshKey: number;
  dirty: boolean;
  profile?: CourtProfile;
  checksText: string;
  checksState?: 'loading' | 'error' | 'pending' | 'danger' | 'warning' | 'success';
  storage?: boolean;
  archivedDocuments?: MatterDocument[];
}>();
const emit = defineEmits<{
  dirtyChanged: [boolean];
  supplementContext: [string];
  supplementBuild: [string];
  build: [];
  cancel: [];
  retry: [job: Job];
  sourceFailure: [failure: BuildFailure];
  review: [id: string, name: string];
  export: [artifact: ExportManifest];
  documents: [];
  checks: [];
  membership: [];
  storage: [];
  backup: [];
  artifactsChanged: [];
  updatePolicy: [artifact: ExportManifest];
  storedDocument: [id: string, original: boolean];
  source: [id: string, page: number, revision: number];
  restoreDocument: [id: string];
  finding: [finding: any, buildId: string];
  configure: [name: string];
}>();
const supplementOpen = ref(false),
  supplementPlanId = ref(''),
  supplementInitialMode = ref<'plan' | 'policy'>('plan');
const supplementPanel = ref<InstanceType<typeof SupplementWorkspace> | null>(null);
const savedSupplements = ref<Array<{ id: string; title: string; baseline_build_id: string }>>([]);
const supplementBaselineId = ref('');
const supplementBaseline = computed(() =>
  supplementBaselineId.value
    ? props.artifacts.find((a) => a.id === supplementBaselineId.value) || null
    : artifact.value,
);
let supplementTrigger: HTMLElement | null = null;
let supplementRestored = false;
const activeSupplementKey = () => `matterdraft.activeSupplement.${props.matterId}`;
function supplementalSaved(id: string) {
  supplementPlanId.value = id;
  localStorage.setItem(activeSupplementKey(), id);
  void loadSupplements();
}
async function loadSupplements() {
  try {
    const data = await api<{ plans: typeof savedSupplements.value }>(
      `/matters/${props.matterId}/supplements`,
    );
    savedSupplements.value = data.plans;
    if (!supplementRestored) {
      supplementRestored = true;
      const id = localStorage.getItem(activeSupplementKey());
      if (id && data.plans.some((p) => p.id === id)) openSupplement(id);
    }
  } catch {
    /* Opening the planner exposes a retryable service error. */
  }
}
function openSupplement(id = '', initial: 'plan' | 'policy' = 'plan') {
  supplementTrigger = document.activeElement as HTMLElement;
  supplementPlanId.value = id;
  supplementInitialMode.value = initial;
  supplementBaselineId.value =
    savedSupplements.value.find((p) => p.id === id)?.baseline_build_id || artifact.value?.id || '';
  supplementOpen.value = true;
}
async function closeSupplement() {
  supplementOpen.value = false;
  localStorage.removeItem(activeSupplementKey());
  emit('supplementContext', '');
  emit('dirtyChanged', false);
  await loadSupplements();
  await nextTick();
  supplementTrigger?.isConnected && supplementTrigger.focus();
}
watch(
  () => [props.matterId, props.refreshKey],
  () => void loadSupplements(),
  { immediate: true },
);
const selectedId = ref(''),
  screen = ref<'settings' | 'result' | 'stale' | 'changes' | 'job'>('settings');
const storageCategory = ref<'generated' | 'originals' | 'working' | 'archive' | 'archived-builds'>(
  'generated',
);
const explicitBuildSelection = ref(false);
const visibleArtifacts = computed(() =>
  props.artifacts.filter((a) =>
    props.storage && storageCategory.value === 'archived-builds'
      ? !!a.archived_at
      : !a.archived_at ||
        (!props.storage && explicitBuildSelection.value && a.id === selectedId.value),
  ),
);
const artifact = computed(
  () =>
    visibleArtifacts.value.find((a) => a.id === selectedId.value) ||
    visibleArtifacts.value[0] ||
    null,
);
const historyDocument = ref<MatterDocument | null>(null);
const archivePending = ref(false),
  archiveError = ref('');
async function archiveBuild() {
  if (!artifact.value || archivePending.value) return;
  archivePending.value = true;
  archiveError.value = '';
  try {
    await api(`/matters/${props.matterId}/exports/${artifact.value.id}/archive`, 'PUT', {
      archived: !artifact.value.archived_at,
    });
    emit('artifactsChanged');
  } catch (error) {
    archiveError.value = friendlyError(error);
  } finally {
    archivePending.value = false;
  }
}
const baselinePlans = computed(() =>
  savedSupplements.value.filter((plan) => plan.baseline_build_id === artifact.value?.id),
);
const revealError = ref('');
async function revealOutput() {
  if (!artifact.value) return;
  revealError.value = '';
  try {
    await revealSavedExport(props.matterId, artifact.value.id);
  } catch (error) {
    revealError.value = friendlyError(error);
  }
}
watch(
  () => [props.matterId, artifact.value?.id],
  () => {
    revealError.value = '';
  },
);
const checksOpen = ref(false);
const {
  state: qa,
  pending: qaPending,
  error: qaError,
  review: saveReview,
  reload: reloadQa,
} = useOutputValidation(
  () => [props.matterId, artifact.value?.id || ''],
  () => props.refreshKey,
);
const humanReviewed = computed(
  () =>
    !!qa.value &&
    Object.keys(qa.value.review_labels).length > 0 &&
    Object.keys(qa.value.review_labels).every((key) => qa.value!.review.items[key]),
);
const included = computed(() => props.documents.filter((d) => d.bundle_included));
const sourcePages = computed(() => included.value.reduce((n, d) => n + d.page_count, 0));
const job = computed(() => (props.job?.kind === 'build' ? props.job : null));
const hasSizeException = computed(() =>
  Array.isArray((job.value?.result as Record<string, unknown> | null)?.size_exceptions),
);
const running = computed(() => job.value && ['running', 'queued'].includes(job.value.status));
const failures = computed(() => {
  const values = (job.value?.result as { failures?: BuildFailure[] } | null)?.failures;
  return Array.isArray(values) ? values : [];
});
const cancelOpen = ref(false);
watch(
  () => [job.value?.id, job.value?.status],
  () => {
    cancelOpen.value = false;
  },
);
function confirmCancel() {
  if (running.value && !props.cancelPending && !props.blocked) emit('cancel');
}
// Boundaries are actual progress events emitted by engine.build_bundle, not timers.
// Covers, numbering and navigation are assembled per volume, not separate job stages.
const buildStages = computed(() =>
  [
    { label: 'Check source documents', start: 5, end: 10 },
    { label: 'Assemble volumes, index and navigation', start: 10, end: 70 },
    { label: 'Create combined PDF and index CSV', start: 70, end: 85 },
    { label: 'Process outputs and package ZIP', start: 85, end: 97 },
    { label: 'Check saved output files', start: 97, end: 100 },
  ].map((stage) => ({
    ...stage,
    state:
      (job.value?.progress ?? 0) >= stage.end
        ? 'Complete'
        : (job.value?.progress ?? 0) >= stage.start
          ? 'In progress'
          : 'Waiting',
  })),
);

const storedDocuments = computed(() =>
  storageCategory.value === 'archive'
    ? props.archivedDocuments || []
    : storageCategory.value === 'originals'
      ? [...props.documents, ...(props.archivedDocuments || [])]
      : props.documents,
);
const view = computed(() =>
  props.storage && !['changes', 'stale'].includes(screen.value)
    ? 'storage'
    : running.value
      ? 'job'
      : screen.value,
);
const title = computed(
  () =>
    ({
      settings: 'Build bundle',
      result: 'Bundle ready for review',
      stale: 'Bundle is out of date',
      changes: `Changes since build ${artifact.value?.id.slice(0, 8)}`,
      job: running.value ? 'Building bundle' : 'Build stopped',
      storage: 'Storage',
    })[view.value],
);
type Plan = {
  combined_pages: number;
  input_snapshot: string;
  generated_pages: number;
  volumes: Array<{ page_count: number; index_pages: number; cover_pages: number }>;
  numbering: string;
};
const plan = ref<Plan | null>(null),
  planError = ref('');
const savedLayout = ref<SavedLayout | null>(null);
const savedOptions = computed(() => savedLayout.value?.output_marks || null);
const loading = ref(false);
const overrides = computed(() =>
  savedLayout.value ? differences(savedLayout.value, savedLayout.value.applied) : [],
);
const presetName = computed(() =>
  savedLayout.value?.applied
    ? `${savedLayout.value.applied.name} · v${savedLayout.value.applied.version}`
    : savedLayout.value
      ? 'Custom bundle settings'
      : 'Loading saved settings…',
);
const qualityLabel = computed(() =>
  savedOptions.value
    ? {
        original: 'Original page quality',
        lossless: 'Lossless optimization',
        balanced: 'Balanced images · 240 dpi',
        small: 'Smaller images · 150 dpi',
      }[savedOptions.value.processing_quality || 'original']
    : 'Output settings unavailable',
);
const officeLabel = computed(() => {
  const o = savedOptions.value;
  if (!o) return 'Office selection unavailable';
  const selected = [
    o.office_docx && 'DOCX',
    o.office_xlsx && 'XLSX',
    o.office_pptx && 'PPTX',
  ].filter(Boolean);
  return selected.length
    ? `${selected.join(', ')} review drafts selected`
    : 'No Office review drafts selected';
});
const outputFiles = computed(() => [
  'Combined bookmarked PDF',
  'Individual volume PDFs',
  'CSV index',
  'ZIP archive',
]);
const structureFeatures = computed(() => {
  const o = savedOptions.value;
  return [
    {
      label: o?.front_matter === 'cover_index' ? 'Cover and index pages' : 'Index pages · no cover',
      on: true,
    },
    { label: 'Linked index entries', on: o?.numbering_linked_index !== false },
    { label: 'Document bookmarks', on: true },
    { label: 'Matching viewer page labels', on: o?.numbering_pdf_labels !== false },
  ];
});
const numberingLabel = computed(() => {
  const o = savedOptions.value;
  if (!o) return 'Saved numbering unavailable';
  const style = { arabic: '1, 2, 3', roman_lower: 'i, ii, iii', roman_upper: 'I, II, III' }[
    o.numbering_style || 'arabic'
  ];
  const restart = {
    volume: 'restart per volume',
    continuous: 'continuous across volumes',
    section: 'restart per Part',
  }[o.numbering_restart || 'volume'];
  return `${o.numbering_prefix || ''}${style} · ${restart} · start ${o.numbering_start ?? 1}`;
});
const frontMatter = computed(() =>
  plan.value?.volumes.reduce((n, v) => n + v.index_pages + v.cover_pages, 0),
);
const statusKind = computed(() =>
  props.blocked ? 'pending' : props.dirty ? 'pending' : props.checksState || 'pending',
);
const statusText = computed(() =>
  props.blocked
    ? 'Build controls unavailable while the engine is busy or disconnected'
    : props.dirty
      ? 'Unsaved settings · preflight and page plan need refreshing'
      : props.checksText,
);
let sequence = 0;
async function refresh() {
  const call = ++sequence;
  plan.value = null;
  planError.value = '';
  savedLayout.value = null;
  loading.value = false;
  if (props.busy || props.blocked || props.dirty) return;
  loading.value = true;
  try {
    const [p, options] = await Promise.all([
      api<Plan>(`/matters/${props.matterId}/bundle-preview`, 'POST', {
        max_pages_per_volume: props.maxPages,
        keep_groups_intact: props.keepGroups,
      }),
      api<SavedLayout>(`/matters/${props.matterId}/bundle-settings`),
    ]);
    if (call === sequence) {
      if (p.input_snapshot !== options.input_snapshot)
        throw new Error('Matter changed while loading. Refresh the build summary.');
      plan.value = p;
      savedLayout.value = options;
    }
  } catch (e) {
    if (call === sequence) planError.value = friendlyError(e);
  } finally {
    if (call === sequence) loading.value = false;
  }
}
watch(
  () => [
    props.matterId,
    props.refreshKey,
    props.maxPages,
    props.keepGroups,
    props.busy,
    props.blocked,
    props.dirty,
  ],
  () => void refresh(),
  { immediate: true },
);
watch(
  () => `${props.job?.id}:${props.job?.status}`,
  () => {
    const j = props.job;
    if (j?.kind === 'build') {
      if (['failed', 'cancelled', 'interrupted', 'queued', 'running'].includes(j.status))
        screen.value = 'job';
      else if (j.status === 'completed') {
        selectedId.value = j.id;
        screen.value =
          !props.storage && props.artifacts.find((a) => a.id === j.id)?.stale ? 'stale' : 'result';
      }
    }
  },
  { immediate: true },
);
watch(
  () => props.artifacts,
  (list) => {
    if (!selectedId.value && list.length) {
      selectedId.value = list[0].id;
      if (screen.value !== 'job')
        screen.value = !props.storage && list[0].stale ? 'stale' : 'result';
    }
  },
  { immediate: true },
);
const optionsOpen = ref(false),
  volumesOpen = ref(false),
  sizeRecovery = ref(false);
const volumeTrigger = ref<HTMLButtonElement | null>(null);
function closeVolumes() {
  volumesOpen.value = false;
  void nextTick(() => volumeTrigger.value?.focus());
}
const tool = ref(''),
  toolDialog = ref<HTMLDialogElement | null>(null);
let returnFocus: HTMLElement | null = null;
async function openTool(name: string) {
  if (['Presets', 'Preview', 'Cover & sections', 'Edit numbering', 'Numbering'].includes(name)) {
    emit('configure', name);
    return;
  }
  returnFocus = document.activeElement as HTMLElement;
  if (['Plan volumes', 'Size recovery'].includes(name)) {
    sizeRecovery.value = name === 'Size recovery';
    volumesOpen.value = true;
    return;
  }
  tool.value = name;
  if (name === 'Output options') {
    optionsOpen.value = true;
    return;
  }
  await nextTick();
  toolDialog.value?.showModal();
}
function closeOptions() {
  optionsOpen.value = false;
  void nextTick(() => returnFocus?.isConnected && returnFocus.focus());
}
function closeTool() {
  toolDialog.value?.close();
  void nextTick(() => returnFocus?.isConnected && returnFocus.focus());
}
async function showChanges() {
  screen.value = 'changes';
  await nextTick();
  document.querySelector<HTMLElement>('.build-heading h1')?.focus();
}
async function showChoices() {
  screen.value = 'stale';
  await nextTick();
  document.querySelector<HTMLButtonElement>('.build-heading .review-changes')?.focus();
}
function selectBuild() {
  screen.value = artifact.value?.stale ? 'stale' : 'result';
}
function showResult() {
  if (artifact.value) {
    screen.value = artifact.value.stale ? 'stale' : 'result';
  }
}
const extraTools = ref<HTMLDetailsElement | null>(null);
async function showOutputTools() {
  showResult();
  await nextTick();
  if (extraTools.value) {
    extraTools.value.open = true;
    extraTools.value.scrollIntoView({ block: 'center' });
  }
}
defineExpose({
  showChoicesFor: (id: string) => {
    explicitBuildSelection.value = true;
    selectedId.value = id;
    void showChoices();
  },
  openSupplementMembership: () => {
    if (!supplementOpen.value) return false;
    supplementPanel.value?.openPicker();
    return true;
  },
  openSupplementTool: (name: string) => {
    if (!supplementOpen.value) return false;
    if (name === 'Preview') void supplementPanel.value?.openMap();
    else supplementPanel.value?.openNumbering();
    return true;
  },
  saveDraft: () => supplementPanel.value?.saveDraft(),
  discard: () => supplementPanel.value?.discard(),
  openTool,
  focusSettings: async (name: string) => {
    screen.value = 'settings';
    await nextTick();
    const labels: Record<string, string> = {
      Presets: 'Presets',
      Preview: 'Preview',
      'Edit numbering': 'Edit numbering',
    };
    const buttons = Array.from(
      document.querySelectorAll<HTMLButtonElement>('.build-settings-card button'),
    );
    buttons
      .find((button) =>
        button.textContent
          ?.trim()
          .replace(/\s+/g, ' ')
          .startsWith(labels[name] || name),
      )
      ?.focus();
  },
  showResult,
  showOutputTools,
  showSettings: () => {
    screen.value = 'settings';
  },
});
onBeforeUnmount(() => {
  sequence++;
  emit('supplementContext', '');
  toolDialog.value?.close();
});
</script>
<template>
  <div class="build-route">
    <section v-if="supplementOpen && !supplementBaseline" class="supplement-workspace"
      ><h1>Baseline unavailable</h1
      ><p role="alert"
        >Saved baseline {{ supplementBaselineId }} is unavailable. This supplement cannot be rebound
        to another build.</p
      ><button class="membership-button secondary" @click="closeSupplement"
        >Return to builds</button
      ></section
    >
    <SupplementWorkspace
      v-else-if="supplementOpen && supplementBaseline"
      ref="supplementPanel"
      :matter-id="matterId"
      :baseline="supplementBaseline"
      :initial-plan-id="supplementPlanId"
      :initial-mode="supplementInitialMode"
      :blocked="blocked || busy"
      :refresh-key="refreshKey"
      :job="props.job"
      :artifacts="artifacts"
      @close="closeSupplement"
      @dirty-changed="emit('dirtyChanged', $event)"
      @saved="supplementalSaved"
      @context="emit('supplementContext', $event)"
      @build="emit('supplementBuild', $event)"
      @cancel="emit('cancel')"
      @retry="emit('retry', $event)"
      @review="(id, name) => emit('review', id, name)"
      @export="emit('export', $event)"
      @finding="(finding, id) => emit('finding', finding, id)"
    />
    <slot v-else-if="volumesOpen" name="volumes" :recovery="sizeRecovery" :close="closeVolumes" />
    <section v-else class="build-workspace" :data-screen="view" aria-label="Bundle build workflow">
      <header class="build-heading">
        <div>
          <h1 tabindex="-1">{{ title }}</h1>
          <p v-if="view === 'settings'">
            {{ included.length }} included documents ·
            {{ documents.length - included.length }} reference-only documents
          </p>
          <p v-else-if="view === 'job'">
            {{ job?.id ? `Build ${job.id.slice(0, 8)} · ` : '' }}Your preparation work and previous
            bundles remain saved.
          </p>
          <p v-else-if="artifact">
            Build {{ artifact.id.slice(0, 8) }} · {{ prettyDate(artifact.created_at) }} · Saved on
            this computer
          </p>
          <p v-else>Local matter files · No generated bundles yet</p>
        </div>
        <button
          v-if="view === 'stale'"
          class="membership-button secondary review-changes"
          @click="showChanges"
          >Review changes</button
        >
        <button v-if="view === 'changes'" class="membership-button primary" @click="emit('checks')"
          >Run checks</button
        >
        <button
          v-if="artifact && view === 'result'"
          class="membership-button primary"
          @click="emit('review', artifact.id, 'combined.pdf')"
        >
          Review bundle</button
        ><button
          v-if="view === 'storage'"
          class="membership-button secondary"
          @click="emit('backup')"
        >
          Create backup
        </button>
      </header>
      <template v-if="view === 'settings'">
        <div class="build-strip build-preflight" :class="statusKind" role="status">
          <CircleCheck v-if="statusKind === 'success'" :size="16" /><AlertCircle
            v-else
            :size="16"
          />
          <span>{{ statusText }}</span>
          <button class="text-tool" @click="emit('checks')">Preflight</button>
        </div>
        <div v-if="planError" class="build-load-error" role="alert"
          >Build summary unavailable: {{ planError }}
          <button class="membership-button secondary" :disabled="busy || blocked" @click="refresh"
            >Refresh summary</button
          >
        </div>
        <div class="build-settings-grid" :aria-busy="loading">
          <section class="build-card build-settings-card">
            <header>
              <div
                ><small>BUNDLE PRESET</small><h2>{{ presetName }}</h2></div
              >
              <button class="membership-button quiet" @click="openTool('Presets')">Presets</button>
              <button class="membership-button secondary" @click="openTool('Preview')"
                >Preview</button
              >
            </header>
            <section class="build-output-section">
              <h3>Output files</h3>
              <div class="build-features">
                <span v-for="name in outputFiles" :key="name"
                  ><Check :size="16" aria-hidden="true" />{{ name }}</span
                >
              </div>
            </section>
            <section class="build-structure-section">
              <div class="build-desktop-structure">
                <h3>Structure &amp; navigation</h3>
                <div v-if="savedOptions" class="build-features">
                  <span
                    v-for="feature in structureFeatures"
                    :key="feature.label"
                    :class="{ 'feature-off': !feature.on }"
                  >
                    <component :is="feature.on ? Check : Minus" :size="16" aria-hidden="true" />{{
                      feature.label
                    }}<small v-if="!feature.on">Off</small>
                  </span>
                </div>
                <p v-else>{{
                  loading ? 'Loading saved navigation…' : 'Saved navigation unavailable'
                }}</p>
              </div>
              <div class="build-compact-structure">
                <h3>{{
                  savedOptions?.front_matter === 'cover_index'
                    ? 'Cover, index & bookmarks'
                    : 'Index & bookmarks · no cover'
                }}</h3>
                <p>{{
                  savedOptions
                    ? `${savedOptions.numbering_linked_index === false ? 'Index links off' : 'Linked index'} · ${savedOptions.numbering_pdf_labels === false ? 'Viewer labels off' : 'Viewer labels match printed references'}`
                    : 'Saved navigation unavailable'
                }}</p>
              </div>
            </section>
            <section class="build-setting-row build-numbering-row">
              <div
                ><h3>Numbering</h3><p :title="numberingLabel">{{ numberingLabel }}</p></div
              >
              <button class="membership-button secondary" @click="openTool('Edit numbering')"
                ><span class="build-desktop-label">Edit numbering</span
                ><span class="build-compact-label">Numbering</span></button
              >
            </section>
            <section class="build-setting-row build-processing-row">
              <div
                ><h3>{{ qualityLabel }} · screen output</h3
                ><p
                  >{{
                    savedOptions?.cleanup_metadata ? 'Metadata cleanup on' : 'Metadata retained'
                  }}
                  · {{ officeLabel }}</p
                ></div
              >
              <button class="membership-button secondary" @click="openTool('Output options')"
                >Output options</button
              >
            </section>
            <p class="build-compact-note"
              >{{ qualityLabel }} · screen output<br />{{ officeLabel }} · Output options</p
            >
          </section>
          <aside class="build-card build-summary">
            <h2>Bundle summary</h2>
            <dl>
              <div
                ><dt>Included documents</dt><dd>{{ included.length }}</dd></div
              >
              <div
                ><dt>Source pages</dt><dd>{{ sourcePages }}</dd></div
              >
              <div
                ><dt>Front matter</dt><dd>{{ frontMatter ?? '—' }}</dd></div
              >
              <div
                ><dt>Physical pages</dt><dd>{{ plan?.combined_pages ?? '—' }}</dd></div
              >
              <div
                ><dt>Volumes</dt><dd>{{ plan?.volumes.length ?? '—' }}</dd></div
              >
              <div
                ><dt>Generated pages</dt><dd>{{ plan?.generated_pages ?? '—' }}</dd></div
              >
            </dl>
            <div class="build-limit">
              <small>VOLUME SIZE POLICY</small>
              <strong>{{
                savedOptions
                  ? savedOptions.volume_max_bytes
                    ? `${bytesLabel(savedOptions.volume_max_bytes)} per volume PDF`
                    : 'No optional volume byte limit'
                  : 'Saved byte policy unavailable'
              }}</strong>
              <p class="build-policy-desktop"
                >{{ maxPages }} physical pages per volume.
                {{ keepGroups ? 'Keep groups together.' : 'Split between documents.' }}<br />{{
                  profile?.max_file_mb
                    ? `Profile file limit: ${profile.max_file_mb} MB · separately checked.`
                    : 'No profile file-size limit.'
                }}</p
              >
              <p class="build-policy-compact"
                >{{ maxPages }} pages/volume<br />Profile:
                {{
                  profile?.max_file_mb ? `${profile.max_file_mb} MB/file` : 'no file-size limit'
                }}</p
              >
            </div>
            <button
              ref="volumeTrigger"
              class="membership-button quiet"
              @click="openTool('Plan volumes')"
              >Plan volumes</button
            >
          </aside>
        </div>
        <footer class="build-footer">
          <span>This build keeps a fixed copy of the selected revisions and settings.</span>
          <button
            class="membership-button primary"
            :disabled="blocked || busy || dirty || !included.length"
            :aria-busy="busy"
            @click="emit('build')"
            ><Package :size="16" />Build bundle</button
          >
        </footer>
        <div class="build-secondary">
          <button class="text-tool" @click="emit('membership')">Bundle membership</button>
          <button class="text-tool" @click="openTool('Technical output tools')"
            >Technical marks &amp; Part extracts</button
          >
          <button v-if="artifact" class="text-tool" @click="showResult"
            >Previous generated output</button
          >
          <span v-if="loading" role="status">Loading current page plan…</span>
          <span v-else-if="dirty">Save or discard settings before building.</span>
          <span v-else-if="!included.length">Select documents in Bundle membership to build.</span>
          <details v-if="overrides.length" class="build-overrides"
            ><summary>{{ overrides.length }} preset override(s)</summary
            ><ul
              ><li v-for="item in overrides" :key="item.field"
                >{{ item.label }}: {{ item.current }}</li
              ></ul
            ></details
          >
          <span v-if="savedOptions?.part_extracts_enabled">Part PDFs selected</span>
        </div>
      </template>
      <template v-else-if="view === 'job'">
        <section
          v-if="running"
          class="build-card build-job-card"
          aria-label="Build progress details"
        >
          <span class="build-state" role="status">{{
            cancelPending
              ? 'Cancellation requested'
              : job?.status === 'queued'
                ? 'Queued'
                : 'In progress'
          }}</span>
          <div class="build-progress-title">
            <h2>{{ job?.message }}</h2><strong>{{ job?.progress }}%</strong>
          </div>
          <progress :value="job?.progress" max="100" aria-label="Build progress" />
          <div class="build-job-grid">
            <ol class="build-stage-list" aria-label="Build stages">
              <li v-for="stage in buildStages" :key="stage.label" :data-state="stage.state">
                <CircleCheck v-if="stage.state === 'Complete'" :size="16" aria-hidden="true" />
                <LoaderCircle
                  v-else-if="stage.state === 'In progress'"
                  :size="16"
                  aria-hidden="true"
                />
                <Circle v-else :size="16" aria-hidden="true" />
                <span>{{ stage.label }}</span><small>{{ stage.state }}</small>
              </li>
            </ol>
            <aside class="build-card">
              <h3>{{ job?.input_context ? 'Frozen input snapshot' : 'Submitted build' }}</h3>
              <p class="build-job-id">{{ job?.id }}</p>
              <template v-if="job?.input_context">
                <p>{{ job.input_context.included_documents }} included documents</p>
                <p
                  >{{ job.input_context.source_pages }} source pages +
                  {{ job.input_context.generated_pages ?? '—' }} generated</p
                >
                <p
                  >{{ job.input_context.volumes ?? '—' }} volumes ·
                  {{ job.input_context.physical_pages ?? '—' }} physical pages</p
                >
                <small v-if="job.input_context.plan_error"
                  >Page plan unavailable; see the build error.</small
                >
              </template>
              <p v-else>Legacy job: frozen counts are unavailable.</p>
              <p v-if="job?.created_at">Submitted {{ prettyDate(job.created_at) }}</p>
              <button
                v-if="artifact"
                class="text-tool"
                @click="emit('review', artifact.id, 'combined.pdf')"
              >
                Earlier build {{ artifact.id.slice(0, 8) }} stays available
              </button>
            </aside>
          </div>
          <p class="build-progress-note" role="status">{{
            cancelPending
              ? 'Waiting for the backend to confirm cancellation. A completed publication cannot be undone.'
              : 'The checks report will describe the files actually written.'
          }}</p>
        </section>
        <div v-else class="build-recovery-grid">
          <section class="build-card build-recovery-card" aria-label="Build recovery">
            <div class="build-recovery-status">
              <span class="build-failure-icon"><CircleX :size="20" aria-hidden="true" /></span>
              <span class="build-state" :class="{ danger: job?.status === 'failed' }">{{
                job?.status === 'interrupted'
                  ? 'Build interrupted'
                  : job?.status === 'cancelled'
                    ? 'Build cancelled'
                    : 'Build failed'
              }}</span>
            </div>
            <h2>{{
              job?.status === 'cancelled'
                ? 'The build was cancelled'
                : job?.status === 'interrupted'
                  ? 'The local engine stopped before completion'
                  : 'The bundle could not be generated'
            }}</h2>
            <p class="build-error-detail" role="alert">{{
              job?.error ||
              job?.message ||
              'Inspect the current inputs before starting a new build.'
            }}</p>
            <p class="build-recovery-help"
              >Inspect the working documents and saved settings, confirm the page map and bookmarks,
              then run checks again. Retry saved inputs reuses the submitted revisions and settings.
              Build current inputs uses your latest saved preparation.</p
            >
            <div class="build-actions">
              <button class="membership-button primary" @click="emit('documents')"
                >Inspect working documents</button
              >
              <button class="membership-button secondary" @click="emit('checks')"
                >Run checks</button
              >
              <button
                v-if="job?.input_context && !job.retry_job_id"
                class="membership-button quiet"
                :disabled="busy || blocked"
                @click="emit('retry', job)"
                >Retry saved inputs</button
              >
              <button
                class="membership-button secondary"
                :disabled="busy || blocked || dirty || !included.length"
                @click="emit('build')"
                >Build current inputs</button
              >
            </div>
            <details class="build-recovery-details">
              <summary>Technical details</summary>
              <p class="build-job-id"
                >Job {{ job?.id }} · {{ job?.status }} · {{ job?.progress }}%</p
              >
              <p>Imported originals and earlier generated files remain preserved.</p>
              <p v-if="job?.input_context">Snapshot {{ job.input_context.snapshot_id }}</p>
              <ul v-if="job?.events?.length"
                ><li v-for="event in job.events" :key="event.at"
                  >{{ event.at }} · {{ event.progress }}% · {{ event.message }}</li
                ></ul
              >
            </details>
          </section>
          <aside class="build-card build-recovery-context">
            <h2>Build context</h2>
            <FileText :size="20" aria-hidden="true" />
            <ul v-if="failures.length" class="build-failure-targets">
              <li v-for="(failure, index) in failures" :key="index">
                <strong>{{ failure.code.replace(/_/g, ' ').toLowerCase() }}</strong>
                <p>{{ failure.message }}</p>
                <button
                  v-if="failure.document_id && failure.target_revision !== undefined"
                  class="text-tool"
                  @click="emit('sourceFailure', failure)"
                  >{{
                    failure.page_index === undefined
                      ? 'Inspect affected document'
                      : `Open source page ${failure.page_index + 1}`
                  }}
                  · Revision {{ failure.target_revision }}</button
                >
              </li>
            </ul>
            <p v-else
              >This failure concerns the build as a whole. Inspect the error and current
              settings.</p
            >
            <span class="build-state">Working copies</span>
            <p v-if="artifact"
              ><button class="text-tool" @click="emit('review', artifact.id, 'combined.pdf')"
                >Open earlier build {{ artifact.id.slice(0, 8) }}</button
              ></p
            >
            <h3>Recovery path</h3>
            <ol
              ><li>Inspect inputs and build settings</li
              ><li>Confirm page map and bookmarks</li
              ><li>Run checks and rebuild</li></ol
            >
          </aside>
        </div>
        <footer class="build-footer">
          <button v-if="storage" class="membership-button secondary" @click="screen = 'result'"
            >Back to storage</button
          >
          <button class="membership-button secondary" @click="emit('documents')">
            Continue working</button
          ><AppConfirmDialog
            v-if="running"
            v-model:open="cancelOpen"
            title="Cancel this build?"
            description="Request cancellation of this attempt. Your preparation and previous builds remain saved. If publication has already finished, the completed build stays available."
            confirm-label="Cancel build"
            cancel-label="Keep building"
            @confirm="confirmCancel"
          >
            <button class="membership-button secondary" :disabled="cancelPending || blocked">{{
              cancelPending ? 'Cancelling…' : 'Cancel build'
            }}</button> </AppConfirmDialog
          ><button v-else class="membership-button secondary" @click="screen = 'settings'">
            Build settings
          </button>
          <button
            v-if="!running && hasSizeException"
            ref="volumeTrigger"
            class="membership-button primary"
            @click="openTool('Size recovery')"
            >Size recovery</button
          >
        </footer>
      </template>
      <template v-else-if="view === 'stale' && artifact">
        <div class="build-strip warning">
          Effective inputs have changed · Earlier output files remain unchanged
        </div>
        <section class="build-card stale-summary">
          <div>
            <h2>Earlier bundle · {{ artifact.id.slice(0, 8) }}</h2>
            <p>
              {{ artifact.pages }} physical pages · {{ artifact.volumes }} volumes ·
              {{ prettyDate(artifact.created_at) }}
            </p>
          </div>
          <div>
            <h2>Current preparation</h2>
            <p> {{ sourcePages }} source pages · {{ included.length }} included documents </p>
          </div>
        </section>
        <h2 class="stale-question">How should this update be prepared?</h2>
        <div class="stale-options">
          <section class="build-card">
            <Package />
            <h2>Rebuild bundle</h2>
            <p>
              Recalculate references for all included documents. The earlier bundle stays unchanged.
            </p>
            <button
              class="membership-button primary"
              @click="storage ? emit('build') : (screen = 'settings')"
            >
              Rebuild
            </button>
          </section>
          <section class="build-card">
            <FileText />
            <h2>Supplementary bundle</h2>
            <p>
              Create a separate plan for selected source pages, linked to this real baseline. Keep
              the earlier bundle intact.
            </p>
            <button
              :disabled="blocked || busy"
              class="membership-button secondary"
              @click="openSupplement()"
            >
              Create supplement
            </button>
          </section>
          <section class="build-card">
            <FileText />
            <h2>Preserve existing references</h2>
            <p
              >Preview supported insert labels, changed physical positions and rebuilt navigation
              against a saved supplemental plan.</p
            >
            <button
              :disabled="blocked || busy || !baselinePlans.length"
              :title="baselinePlans.length ? '' : 'Create and save a supplemental plan first'"
              class="membership-button secondary"
              @click="openSupplement(baselinePlans[0].id, 'policy')"
            >
              Preview inserts
            </button>
          </section>
        </div>
        <footer class="build-footer">
          <button
            class="membership-button quiet"
            @click="emit('review', artifact.id, 'combined.pdf')"
          >
            Open earlier bundle</button
          ><button class="membership-button secondary" @click="screen = 'result'">
            Saved files</button
          ><span>Export does not mark a bundle as served or filed.</span>
        </footer>
      </template>
      <template v-else-if="view === 'changes' && artifact"
        ><BuildComparison
          :matter-id="matterId"
          :export-id="artifact.id"
          @source="(id, page, revision) => emit('source', id, page, revision)"
          :max-pages="maxPages"
          :keep-groups="keepGroups"
          :refresh-key="refreshKey"
        />
        <footer class="build-footer">
          <button v-if="storage" class="membership-button secondary" @click="screen = 'result'"
            >Back to storage</button
          >
          <button
            class="membership-button secondary"
            @click="emit('review', artifact.id, 'combined.pdf')"
          >
            Open earlier bundle</button
          ><button
            class="membership-button quiet"
            @click="storage ? emit('updatePolicy', artifact) : showChoices()"
          >
            Choose update policy</button
          >
        </footer>
      </template>
      <template v-else>
        <div v-if="artifact" class="build-strip" :class="{ warning: artifact.stale }">
          {{ artifact.stale ? 'Previous generated output · Stale' : 'Generated output saved' }}
          · {{ artifact.pages }} physical pages · {{ artifact.volumes }} volumes ·
          {{
            qaPending
              ? 'Loading checks'
              : qa?.can_release
                ? 'Technical and human review recorded'
                : qa?.can_download
                  ? humanReviewed
                    ? 'Technical checks complete · human review recorded'
                    : 'Technical checks complete · human review required'
                  : 'Output checks need attention'
          }}
        </div>
        <div
          v-if="artifact || storage"
          class="build-result-grid"
          :class="{ 'storage-grid': storage }"
        >
          <aside v-if="storage" class="build-card storage-categories">
            <small>MATTER FILES</small>
            <nav aria-label="Stored file categories">
              <button
                :aria-current="storageCategory === 'originals' ? 'page' : undefined"
                @click="storageCategory = 'originals'"
                >Imported originals<small
                  >{{ documents.length + (archivedDocuments?.length || 0) }} files ·
                  preserved</small
                ></button
              >
              <button
                :aria-current="storageCategory === 'working' ? 'page' : undefined"
                @click="storageCategory = 'working'"
                >Working revisions<small>{{ documents.length }} current documents</small></button
              >
              <button
                :aria-current="storageCategory === 'generated' ? 'page' : undefined"
                @click="storageCategory = 'generated'"
                >Generated bundles<small
                  >{{ artifacts.filter((a) => !a.archived_at).length }} saved builds</small
                ></button
              >
              <button
                :aria-current="storageCategory === 'archive' ? 'page' : undefined"
                @click="storageCategory = 'archive'"
                >Archived documents<small
                  >{{ archivedDocuments?.length || 0 }} retained documents</small
                ></button
              >
              <button
                :aria-current="storageCategory === 'archived-builds' ? 'page' : undefined"
                @click="storageCategory = 'archived-builds'"
                >Archived builds<small
                  >{{ artifacts.filter((a) => a.archived_at).length }} preserved builds</small
                ></button
              >
            </nav>
            <p>Imported originals remain unchanged. Edits are stored in working revisions.</p>
            <slot name="storage-tools" />
          </aside>
          <section
            v-if="storage && !['generated', 'archived-builds'].includes(storageCategory)"
            class="build-card stored-documents"
          >
            <h2>{{
              storageCategory === 'originals'
                ? 'Imported originals'
                : storageCategory === 'working'
                  ? 'Working revisions'
                  : 'Archived documents'
            }}</h2>
            <p>{{
              storageCategory === 'originals'
                ? 'Includes reference-only and archived originals. Imported bytes are preserved. Restore an archived document before opening it.'
                : storageCategory === 'working'
                  ? 'Open the current working revision. Matter Undo/Redo remains available in Documents.'
                  : 'Restore returns the retained document to preparation; earlier generated files stay unchanged.'
            }}</p>
            <div v-for="doc in storedDocuments" :key="doc.id" class="stored-document-row">
              <FileText :size="18" /><div
                ><strong>{{ doc.title }}</strong
                ><small
                  >{{ doc.original_name }} · Revision {{ doc.revision }} ·
                  {{ doc.page_count }} pages</small
                ><small>{{
                  doc.archived_at
                    ? 'Archived'
                    : doc.bundle_included
                      ? 'Included in bundle'
                      : 'Reference-only'
                }}</small></div
              >
              <button
                v-if="doc.archived_at"
                class="text-tool"
                :disabled="busy || blocked"
                @click="emit('restoreDocument', doc.id)"
                >Restore</button
              >
              <button
                v-else
                class="text-tool"
                @click="emit('storedDocument', doc.id, storageCategory === 'originals')"
                >{{
                  storageCategory === 'originals' ? 'View original' : 'Open working copy'
                }}</button
              >
              <button
                v-if="storageCategory === 'working'"
                class="text-tool"
                @click="historyDocument = doc"
                >Revision history</button
              >
            </div>
            <p v-if="!storedDocuments.length"
              >No {{ storageCategory === 'archive' ? 'archived documents' : 'documents' }} in this
              matter.</p
            >
          </section>
          <section v-else-if="artifact" class="build-card generated-files">
            <header>
              <h2>{{ storage ? 'Generated bundles' : 'Generated files' }}</h2>
              <AppSelect
                v-model="selectedId"
                aria-label="Saved build"
                @change="!storage && selectBuild()"
              >
                <option v-for="item in visibleArtifacts" :key="item.id" :value="item.id">
                  {{ item.id.slice(0, 8) }} · {{ prettyDate(item.created_at)
                  }}{{ item.stale ? ' · Stale' : '' }}
                </option>
              </AppSelect>
            </header>
            <div class="build-files-scroll">
              <table>
                <thead>
                  <tr>
                    <th>File</th>
                    <th>Pages</th>
                    <th>Measured size</th>
                    <th v-if="storage">Created</th>
                    <th v-if="storage">State</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="file in artifact.files" :key="file.name">
                    <td>
                      <button
                        v-if="file.name.endsWith('.pdf')"
                        class="text-tool"
                        :aria-label="'Review ' + file.name"
                        @click="emit('review', artifact.id, file.name)"
                      >
                        <FileText :size="16" />{{ file.name }}</button
                      ><span v-else>{{ file.name }}</span>
                    </td>
                    <td>{{
                      qa?.run?.measured_files[file.name]?.pages ??
                      (file.name === 'combined.pdf' ? artifact.pages : '—')
                    }}</td>
                    <td :title="`${file.bytes.toLocaleString()} actual bytes`">{{
                      bytesLabel(file.bytes)
                    }}</td>
                    <td v-if="storage">{{ prettyDate(artifact.created_at) }}</td>
                    <td v-if="storage">{{
                      artifact.archived_at
                        ? 'Archived'
                        : artifact.stale
                          ? 'Previous output'
                          : 'Generated'
                    }}</td>
                  </tr>
                </tbody>
              </table>
            </div>
            <p>Build {{ artifact.id }} · Read only</p>
          </section>
          <p v-else class="build-card"
            >No saved builds. Include documents and build a bundle to create an output.</p
          >
          <aside v-if="!storage" class="build-card build-review-checks">
            <h2>Review before sharing</h2>
            <p> Generated files are available. Inspect the pages you will share. </p>
            <label v-for="(label, key) in qa?.review_labels" :key="key" class="build-human-item"
              ><input
                type="checkbox"
                :checked="qa?.review.items[key]"
                :disabled="qaPending || blocked"
                @change="saveReview(String(key), ($event.target as HTMLInputElement).checked)"
              />{{ label }}</label
            ><button class="membership-button secondary" @click="checksOpen = true">
              View output checks</button
            ><small>{{
              qaPending ? 'Loading review…' : 'Review status stays with this build.'
            }}</small>
            <p v-if="qaError" role="alert">{{ qaError }}</p>
          </aside>
        </div>
        <p v-else class="build-card">
          No saved builds. Include documents and build a bundle to create an output.
        </p>
        <footer class="build-footer">
          <button v-if="!storage" class="membership-button secondary" @click="emit('storage')">
            Local storage</button
          ><button
            v-else-if="artifact && ['generated', 'archived-builds'].includes(storageCategory)"
            class="membership-button secondary"
            @click="emit('review', artifact.id, 'combined.pdf')"
            >Review Combined PDF</button
          ><button
            v-if="artifact"
            class="membership-button primary"
            @click="emit('export', artifact)"
          >
            Export files</button
          ><button
            class="membership-button quiet"
            @click="storage ? emit('build') : (screen = 'settings')"
          >
            {{ storage ? 'Build bundle' : 'Rebuild' }}</button
          ><button
            v-if="artifact && inTauri()"
            class="membership-button secondary"
            @click="revealOutput"
            >Show saved file</button
          ><button v-if="artifact?.stale" class="text-tool" @click="showChanges">
            Review changes</button
          ><AppConfirmDialog
            v-if="storage && artifact && ['generated', 'archived-builds'].includes(storageCategory)"
            :title="artifact.archived_at ? 'Restore saved build?' : 'Archive saved build?'"
            description="This only changes its Storage category. All files, review records and baseline references remain preserved."
            :confirm-label="artifact.archived_at ? 'Restore build' : 'Archive build'"
            cancel-label="Cancel"
            @confirm="archiveBuild"
            ><button class="membership-button secondary" :disabled="archivePending || blocked">{{
              artifact.archived_at ? 'Restore build' : 'Archive build'
            }}</button></AppConfirmDialog
          ><span>Later edits leave this build unchanged.</span>
        </footer>
        <p v-if="archiveError" role="alert">{{ archiveError }}</p>
        <p v-if="revealError" role="alert">{{ revealError }}</p>
        <details
          v-if="artifact && $slots['output-tools']"
          ref="extraTools"
          class="build-extra-tools"
        >
          <summary> More output tools · Office drafts, print and protected copies </summary>
          <div role="group" aria-label="Office review exports">
            <slot name="output-tools" :artifact="artifact" />
          </div>
        </details>
      </template>
      <div
        v-if="artifact && ['settings', 'result', 'stale'].includes(view)"
        class="supplement-entry"
        :class="{ 'has-plans': savedSupplements.length > 0 }"
      >
        <button
          v-if="view !== 'stale'"
          class="text-tool"
          :disabled="blocked || busy"
          @click="openSupplement()"
          >Create supplement from build {{ artifact.id.slice(0, 8) }}</button
        >
        <button
          v-for="p in savedSupplements"
          :key="p.id"
          class="text-tool"
          :disabled="blocked || busy"
          @click="openSupplement(p.id)"
          >Resume {{ p.title }}</button
        >
      </div>
      <RevisionHistoryDialog
        v-if="historyDocument"
        :document="historyDocument"
        @close="historyDocument = null"
        @restored="emit('artifactsChanged')"
      />
      <OutputChecksDialog
        v-if="checksOpen && artifact"
        :key="artifact.id"
        :matter-id="matterId"
        :artifact="artifact"
        @close="
          checksOpen = false;
          reloadQa();
        "
        @changed="reloadQa"
        @finding="
          checksOpen = false;
          emit('finding', $event, artifact.id);
        "
        @review="
          checksOpen = false;
          emit('review', artifact.id, 'combined.pdf');
        "
      />
      <slot v-if="optionsOpen" name="processing" :close="closeOptions" />
      <dialog
        v-dismiss-outside
        ref="toolDialog"
        class="build-tool-dialog"
        :aria-label="tool"
        @cancel.prevent="closeTool"
      >
        <header>
          <h2>{{ tool }}</h2>
          <button class="icon-btn" aria-label="Close build tool" @click="closeTool">
            <X :size="18" />
          </button>
        </header>
        <div v-show="tool === 'Preview'"><slot name="preview" /></div>

        <div v-if="tool === 'Technical output tools'">
          <slot name="settings" />
        </div>
      </dialog>
    </section>
  </div>
</template>
