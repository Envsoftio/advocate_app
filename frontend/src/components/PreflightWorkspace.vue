<script setup lang="ts">
import AppSelect from './ui/AppSelect.vue';
import { computed, ref, watch, onBeforeUnmount } from 'vue';
import { Info, TriangleAlert, CircleX, CircleCheck, RefreshCw } from 'lucide-vue-next';
import { api } from '../lib/api';
import { friendlyError } from '../lib/format';
import type { Check, Job } from '../lib/types';
const props = defineProps<{
  matterId: string;
  screeningJob?: Job | null;
  checks: Check[];
  loading: boolean;
  error: string;
  pending: boolean;
  blocked: boolean;
  filter: string;
}>();
const emit = defineEmits<{
  run: [];
  screen: [];
  cancelScreen: [];
  finding: [check: Check];
  build: [];
  resolve: [action: string];
  'update:filter': [value: string];
}>();
const screening = computed(() => ['queued', 'running'].includes(props.screeningJob?.status || ''));
const screeningStatus = computed(() =>
  props.checks.some((c) => c.code === 'PAGE_SCREENING_COMPLETED')
    ? 'Current results available in findings'
    : 'Not run for current inputs',
);
const reviewOpen = ref(false);
const reviewItems = [
  {
    key: 'latest_order',
    label: 'Latest order included',
    help: 'Check the latest applicable order is included, or confirm this does not apply.',
  },
  {
    key: 'sequence',
    label: 'Annexure sequence',
    help: 'Review included documents and their order in Index & bookmarks.',
  },
  {
    key: 'titles',
    label: 'Document titles',
    help: 'Check titles and labels against the source documents.',
  },
  {
    key: 'pagination',
    label: 'Pagination',
    help: 'Review the planned page ranges; check the generated PDF again after building.',
  },
];
type Review = {
  input_digest: string;
  version: number;
  items: Record<string, boolean>;
  warnings: string[];
  updated_at: string | null;
};
const review = ref<Review | null>(null);
const reviewError = ref('');
const reviewBusy = ref(false);
let sequence = 0;
const runId = computed(() => props.checks[0]?.validation_run_id);
const snapshot = computed(() => props.checks[0]?.effective_input_digest);
const current = computed(
  () => !!runId.value && !props.loading && !props.error && !props.pending && !props.blocked,
);
const canReview = computed(
  () => current.value && !reviewBusy.value && review.value?.input_digest === snapshot.value,
);
const reviewed = computed(
  () =>
    props.checks.filter(
      (c) => c.severity === 'warning' && review.value?.warnings.includes(c.finding_id || ''),
    ).length,
);
const remaining = computed(() => reviewItems.filter((i) => !review.value?.items[i.key]).length);
async function loadReview() {
  const call = ++sequence;
  review.value = null;
  reviewError.value = '';
  reviewBusy.value = true;
  try {
    const result = await api<Review>(`/matters/${props.matterId}/preflight/review`);
    if (call === sequence) {
      if (result.input_digest !== snapshot.value)
        throw new Error('Saved inputs changed. Run checks again before reviewing.');
      result.warnings = result.warnings.filter((id) =>
        props.checks.some((c) => c.severity === 'warning' && c.finding_id === id),
      );
      review.value = result;
    }
  } catch (e) {
    if (call === sequence) reviewError.value = friendlyError(e);
  } finally {
    if (call === sequence) reviewBusy.value = false;
  }
}
watch(
  () => [props.matterId, runId.value],
  () => {
    ++sequence;
    review.value = null;
    reviewBusy.value = false;
    if (runId.value) void loadReview();
  },
  { immediate: true },
);
onBeforeUnmount(() => sequence++);
async function saveReview(key?: string, finding?: Check) {
  if (!canReview.value || !review.value) return;
  const call = ++sequence;
  const payload = {
    ...review.value,
    items: { ...review.value.items },
    warnings: [...review.value.warnings],
    run_id: runId.value,
  };
  if (key) payload.items[key] = !payload.items[key];
  if (finding?.finding_id)
    payload.warnings = payload.warnings.includes(finding.finding_id)
      ? payload.warnings.filter((id) => id !== finding.finding_id)
      : [...payload.warnings, finding.finding_id];
  reviewBusy.value = true;
  reviewError.value = '';
  try {
    const result = await api<Review>(`/matters/${props.matterId}/preflight/review`, 'PUT', payload);
    if (call === sequence) review.value = result;
  } catch (e) {
    if (call === sequence) reviewError.value = friendlyError(e);
  } finally {
    if (call === sequence) reviewBusy.value = false;
  }
}
const errors = computed(() => props.checks.filter((c) => c.severity === 'error'));
const warnings = computed(() => props.checks.filter((c) => c.severity === 'warning'));
const sections = computed(() =>
  [
    {
      key: 'error',
      title: errors.value.length ? 'ERRORS · MUST RESOLVE BEFORE THIS BUILD' : 'ERRORS · NONE',
      rows: errors.value,
    },
    { key: 'warning', title: 'WARNINGS · REVIEW RECOMMENDED', rows: warnings.value },
  ].filter((s) => props.filter === 'all' || props.filter === s.key),
);
const notes = computed(() => props.checks.filter((c) => c.severity === 'info'));
function action(check: Check): string | undefined {
  if (check.action) return check.action;
  if (check.code === 'PROFILE_NOT_APPROVED') return 'court-rules';
  if (check.code.includes('BOOKMARK')) return 'bookmarks';
  if (check.code === 'NO_DOCUMENTS') return 'membership';
  if (check.code.startsWith('USER_CHECKLIST')) return 'checklist';
  if (!check.document_id && check.severity === 'error') return 'settings';
}
function resolve(check: Check) {
  const target = action(check);
  if (target === 'checklist') reviewOpen.value = true;
  else if (target) emit('resolve', target);
  else emit('finding', check);
}
const actionLabel = (check: Check) => {
  const target = action(check);
  if (target === 'court-rules') return 'Review court guidance';
  if (target === 'bookmarks') return 'Fix target';
  if (target === 'membership') return 'Choose documents';
  if (target === 'volumes') return 'Review volumes';
  if (target === 'settings') return 'Review settings';
  if (target === 'checklist') return 'Open checklist';
  return check.page_index === undefined
    ? 'Review document'
    : `Review source ${check.page_index + 1}`;
};
</script>
<template>
  <section class="preflight-workspace" aria-label="Current revision Preflight" :aria-busy="loading">
    <header class="build-heading">
      <div
        ><h1 tabindex="-1">Preflight · current revision</h1>
        <p
          >Find issues before building. Technical checks and your human review are tracked
          separately.</p
        ></div
      >
      <div class="preflight-toolbar">
        <label
          >Show<AppSelect
            aria-label="Show"
            :value="filter"
            @change="emit('update:filter', ($event.target as HTMLSelectElement).value)"
          >
            <option value="all">All findings</option><option value="error">Errors</option
            ><option value="warning">Warnings</option><option value="info">Technical notes</option>
          </AppSelect></label
        >
        <button
          class="membership-button primary"
          :disabled="blocked || loading || pending"
          @click="emit('run')"
          ><RefreshCw :size="14" />{{ loading ? 'Checking…' : 'Run checks' }}</button
        >
      </div>
    </header>
    <div class="preflight-summary">
      <div
        ><span>Errors</span
        ><strong class="qa-error">{{ current ? errors.length : '—' }}</strong></div
      >
      <div
        ><span>Warnings</span
        ><strong>{{
          current ? `${warnings.length}${reviewed ? ` · ${reviewed} reviewed` : ''}` : '—'
        }}</strong></div
      >
      <div
        ><span>Technical checks</span
        ><strong>{{
          loading
            ? 'Running'
            : error || blocked
              ? 'Unavailable'
              : pending
                ? 'Pending changes'
                : runId
                  ? errors.length
                    ? 'Needs correction'
                    : 'Input scan completed'
                  : 'Not run'
        }}</strong></div
      >
      <div
        ><span>Human review</span
        ><strong>{{
          reviewBusy
            ? 'Saving / loading…'
            : review
              ? remaining
                ? `${remaining} remaining`
                : '4 acknowledged'
              : 'Not available'
        }}</strong></div
      >
    </div>
    <p v-if="loading" role="status" class="build-strip"
      >Checking saved inputs, working pages and the volume plan…</p
    >
    <p v-else-if="error || blocked" role="alert" class="build-strip danger">{{
      error || 'Local engine unavailable. Reconnect and run checks again.'
    }}</p>
    <p v-else-if="pending" role="status" class="build-strip warning"
      >Inputs have pending changes. Save and run checks again.</p
    >
    <section class="preflight-screening" aria-label="Blank page and OCR screening">
      <div
        ><strong>Blank pages &amp; OCR confidence</strong>
        <p
          >Screen included pages for possible blanks and low-confidence OCR. Read-only; pages are
          never removed or changed.</p
        >
        <small v-if="!screening"
          >{{ screeningStatus }} · A confidence score is not text accuracy.</small
        >
        <p v-if="screening" role="status"
          >{{ screeningJob?.message }} · {{ screeningJob?.progress }}%</p
        >
        <p
          v-else-if="
            screeningJob && ['failed', 'cancelled', 'interrupted'].includes(screeningJob.status)
          "
          role="status"
          >Screening {{ screeningJob.status }}.
          {{
            screeningJob.status === 'failed' ? screeningJob.error : 'No new results were published.'
          }}
          Run screening again to retry.</p
        >
      </div>
      <button
        v-if="screening"
        class="membership-button secondary"
        :disabled="blocked"
        @click="emit('cancelScreen')"
        >Cancel screening</button
      >
      <button
        v-else
        class="membership-button secondary"
        :disabled="!current || !!errors.length"
        @click="emit('screen')"
        >Screen pages</button
      >
    </section>
    <div class="preflight-findings">
      <section
        v-for="section in sections"
        :key="section.key"
        :data-severity="section.key"
        :data-clear="current && !section.rows.length"
      >
        <h2>{{
          !current && section.key === 'error' ? 'ERRORS · AWAITING CURRENT CHECKS' : section.title
        }}</h2>
        <div
          v-for="(check, index) in section.rows"
          :key="check.finding_id || index"
          class="preflight-finding"
        >
          <component :is="section.key === 'error' ? CircleX : TriangleAlert" :size="16" />
          <div
            ><strong>{{ check.code.toLowerCase().split('_').join(' ') }}</strong
            ><p>{{ check.message }}</p></div
          >
          <button
            v-if="check.document_id || action(check)"
            :id="`finding-${check.finding_id}`"
            class="membership-button quiet"
            :disabled="!current"
            @click="resolve(check)"
            >{{ actionLabel(check) }}</button
          >
          <button
            v-if="section.key === 'warning'"
            class="preflight-ack"
            :aria-label="`Acknowledge warning: ${check.code.toLowerCase().split('_').join(' ')}`"
            :aria-pressed="review?.warnings.includes(check.finding_id || '') || false"
            :disabled="!canReview"
            @click="saveReview(undefined, check)"
            >{{
              review?.warnings.includes(check.finding_id || '') ? 'Reviewed' : 'Acknowledge'
            }}</button
          >
        </div>
        <div v-if="!section.rows.length" class="preflight-finding preflight-empty">
          <CircleCheck v-if="current" :size="16" /><Info v-else :size="16" />
          <div
            ><strong>{{
              !current
                ? 'Current results unavailable'
                : section.key === 'error'
                  ? 'No technical blockers'
                  : 'No warnings reported'
            }}</strong
            ><p>{{
              !current
                ? 'Run checks on saved inputs before relying on this summary.'
                : section.key === 'error'
                  ? 'Continue to Build settings, then inspect the generated PDF and its output checks.'
                  : 'Human review is still needed.'
            }}</p></div
          >
        </div>
      </section>
      <details class="preflight-notes" :open="filter === 'info'">
        <summary
          ><Info :size="16" />Technical notes · {{ notes.length
          }}<span>Scope and limitations of these checks</span></summary
        >
        <div v-for="check in notes" :key="check.finding_id || check.code" class="preflight-finding"
          ><div
            ><strong>{{ check.code.toLowerCase().split('_').join(' ') }}</strong
            ><p>{{ check.message }}</p></div
          >
          <button
            v-if="action(check)"
            class="membership-button quiet"
            :disabled="blocked"
            @click="resolve(check)"
            >{{ actionLabel(check) }}</button
          >
        </div>
        <p
          >Preflight checks saved inputs, page rendering and the planned page map. OCR accuracy,
          possible blank pages and legal completeness still require human review. Screen pages
          measures blank-page candidates and OCR confidence for these saved inputs. Generated output
          is checked separately after building.</p
        >
      </details>
      <section class="preflight-human">
        <h2>HUMAN REVIEW</h2>
        <div class="preflight-human-items"
          ><label v-for="item in reviewItems" :key="item.key" :title="item.help"
            ><input
              type="checkbox"
              :checked="review?.items[item.key] || false"
              :disabled="!canReview"
              @change="saveReview(item.key)"
            />{{ item.label }}</label
          ></div
        >
        <p class="preflight-review-help"
          >Acknowledge after reviewing, including whether an item applies. Saved for these inputs
          only; changes require a fresh review. Warnings remain visible when acknowledged.</p
        >
        <p v-if="reviewError" role="alert" class="qa-error"
          >{{ reviewError }}
          <button class="text-tool" :disabled="reviewBusy" @click="loadReview"
            >Retry review</button
          ></p
        >
        <button class="text-tool" :aria-expanded="reviewOpen" @click="reviewOpen = !reviewOpen">{{
          reviewOpen ? 'Close checklist' : 'Open review checklist'
        }}</button>
        <div v-show="reviewOpen"><slot /></div>
      </section>
    </div>
    <footer class="build-footer"
      ><span
        >Passing technical checks does not mean the filing is legally complete or accepted by a
        court.</span
      ><button
        class="membership-button primary"
        :disabled="!current || !!errors.length || reviewBusy"
        @click="emit('build')"
        >Build settings</button
      ></footer
    >
  </section>
</template>
