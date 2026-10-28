<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import {
  Check,
  Package,
  FileText,
  AlertCircle,
  X,
  ArrowRight,
} from "lucide-vue-next";
import { api } from "../lib/api";
import { bytesLabel, prettyDate, friendlyError } from "../lib/format";
import type {
  ExportManifest,
  Job,
  MatterDocument,
  CourtProfile,
  OutputMarks,
} from "../lib/types";
import OutputChecksDialog from "./OutputChecksDialog.vue";
import { useOutputValidation } from "../lib/outputValidation";
import BuildComparison from "./BuildComparison.vue";
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
  storage?: boolean;
}>();
const emit = defineEmits<{
  build: [];
  cancel: [];
  retry: [job: Job];
  review: [id: string, name: string];
  export: [artifact: ExportManifest];
  documents: [];
  checks: [];
  membership: [];
  storage: [];
  backup: [];
  finding: [finding: any, buildId: string];
  configure: [name: string];
}>();
const selectedId = ref(""),
  screen = ref<"settings" | "result" | "stale" | "changes" | "job">("settings");
const artifact = computed(
  () =>
    props.artifacts.find((a) => a.id === selectedId.value) ||
    props.artifacts[0] ||
    null,
);
const checksOpen = ref(false);
const {
  state: qa,
  pending: qaPending,
  error: qaError,
  review: saveReview,
  reload: reloadQa,
} = useOutputValidation(
  () => [props.matterId, artifact.value?.id || ""],
  () => props.refreshKey,
);
const included = computed(() =>
  props.documents.filter((d) => d.bundle_included),
);
const sourcePages = computed(() =>
  included.value.reduce((n, d) => n + d.page_count, 0),
);
const job = computed(() => (props.job?.kind === "build" ? props.job : null));
const running = computed(
  () => job.value && ["running", "queued"].includes(job.value.status),
);
const view = computed(() =>
  props.storage ? "storage" : running.value ? "job" : screen.value,
);
const title = computed(
  () =>
    ({
      settings: "Build bundle",
      result: "Bundle ready for review",
      stale: "Bundle is out of date",
      changes: `Changes since build ${artifact.value?.id.slice(0, 8)}`,
      job: running.value ? "Building bundle" : "Build stopped",
      storage: "Storage",
    })[view.value],
);
type Plan = {
  combined_pages: number;
  volumes: Array<{ page_count: number; index_pages: number }>;
  numbering: string;
};
const plan = ref<Plan | null>(null),
  planError = ref("");
const savedOptions = ref<OutputMarks|null>(null);
const qualityLabel = computed(() => savedOptions.value ? ({original:"Original page quality",lossless:"Lossless optimization",balanced:"Balanced images · 240 dpi",small:"Smaller images · 150 dpi"})[savedOptions.value.processing_quality || "original"] : "Saved output options");
let sequence = 0;
async function refresh() {
  const call = ++sequence;
  plan.value = null;
  planError.value = "";
  if (props.busy || props.blocked || props.dirty) return;
  try {
    const [p, options] = await Promise.all([api<Plan>(
      `/matters/${props.matterId}/bundle-preview`,
      "POST",
      {
        max_pages_per_volume: props.maxPages,
        keep_groups_intact: props.keepGroups,
      },
    ), api<OutputMarks>(`/matters/${props.matterId}/output-marks`)]);
    if (call === sequence) { plan.value = p; savedOptions.value=options; }
  } catch (e) {
    if (call === sequence) planError.value = friendlyError(e);
  }
}
watch(
  () => [
    props.matterId,
    props.refreshKey,
    props.maxPages,
    props.keepGroups,
    props.busy,
    props.dirty,
  ],
  () => void refresh(),
  { immediate: true },
);
watch(
  () => `${props.job?.id}:${props.job?.status}`,
  () => {
    const j = props.job;
    if (j?.kind === "build") {
      if (
        ["failed", "cancelled", "interrupted", "queued", "running"].includes(
          j.status,
        )
      )
        screen.value = "job";
      else if (j.status === "completed") {
        selectedId.value = j.id;
        screen.value = props.artifacts.find((a) => a.id === j.id)?.stale
          ? "stale"
          : "result";
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
      if (screen.value !== "job")
        screen.value = list[0].stale ? "stale" : "result";
    }
  },
  { immediate: true },
);
const optionsOpen = ref(false);
const tool = ref(""),
  toolDialog = ref<HTMLDialogElement | null>(null);
let returnFocus: HTMLElement | null = null;
async function openTool(name: string) {
  if (["Presets", "Preview", "Cover & sections", "Edit numbering", "Numbering"].includes(name)) {
    emit("configure", name);
    return;
  }
  returnFocus = document.activeElement as HTMLElement;
  tool.value = name;
  if(name === "Output options") { optionsOpen.value = true; return; }
  await nextTick();
  toolDialog.value?.showModal();
}
function closeOptions() { optionsOpen.value=false; void nextTick(() => returnFocus?.isConnected && returnFocus.focus()); }
function closeTool() {
  toolDialog.value?.close();
  void nextTick(() => returnFocus?.isConnected && returnFocus.focus());
}
function selectBuild() {
  screen.value = artifact.value?.stale ? "stale" : "result";
}
function showResult() {
  if (artifact.value) {
    screen.value = artifact.value.stale ? "stale" : "result";
  }
}
const extraTools = ref<HTMLDetailsElement | null>(null);
async function showOutputTools() {
  showResult();
  await nextTick();
  if (extraTools.value) {
    extraTools.value.open = true;
    extraTools.value.scrollIntoView({ block: "center" });
  }
}
defineExpose({
  openTool,
  showResult,
  showOutputTools,
  showSettings: () => {
    screen.value = "settings";
  },
});
onBeforeUnmount(() => {
  sequence++;
  toolDialog.value?.close();
});
</script>
<template>
  <section
    class="build-workspace"
    :data-screen="view"
    aria-label="Bundle build workflow"
  >
    <header class="build-heading">
      <div>
        <h1 tabindex="-1">{{ title }}</h1>
        <p v-if="view === 'settings'">
          {{ included.length }} included documents ·
          {{ documents.length - included.length }} reference-only documents
        </p>
        <p v-else-if="view === 'job'">
          {{ job?.id ? `Build ${job.id.slice(0, 8)} · ` : "" }}Your preparation
          work and previous bundles remain saved.
        </p>
        <p v-else-if="artifact">
          Build {{ artifact.id.slice(0, 8) }} ·
          {{ prettyDate(artifact.created_at) }} · Saved on this computer
        </p>
        <p v-else>Local matter files · No generated bundles yet</p>
      </div>
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
      <div class="build-strip">
        <AlertCircle :size="16" />{{ checksText }} · Output checks run on saved
        files after building
      </div>
      <div class="build-settings-grid">
        <section class="build-card build-settings-card">
          <header>
            <div>
              <small>BUNDLE PRESET</small>
              <h2>Configured digital bundle</h2>
            </div>
            <button
              class="membership-button quiet"
              @click="openTool('Presets')"
            >
              Presets</button
            ><button
              class="membership-button secondary"
              @click="openTool('Preview')"
            >
              Preview
            </button>
          </header>
          <section>
            <h3>Output files</h3>
            <div class="build-features">
              <span
                v-for="name in [
                  'Combined bookmarked PDF',
                  'Individual volume PDFs',
                  'CSV index',
                  'ZIP archive',
                ]"
                :key="name"
                ><Check :size="16" />{{ name }}</span
              >
            </div>
          </section>
          <section>
            <h3>Structure &amp; navigation</h3>
            <div class="build-features">
              <span
                v-for="name in [
                  'Generated index pages',
                  'Linked index entries',
                  'Document bookmarks',
                  'Per-volume printed page numbers',
                ]"
                :key="name"
                ><Check :size="16" />{{ name }}</span
              >
            </div>
          </section>
          <section class="build-setting-row">
            <div>
              <h3>Numbering</h3>
              <p>
                {{
                  plan?.numbering ||
                  "Per-volume numbering · configure and preview"
                }}
              </p>
            </div>
            <button
              class="membership-button secondary"
              @click="openTool('Edit numbering')"
            >
              Edit numbering
            </button>
          </section>
          <section class="build-setting-row">
            <div>
              <h3>{{qualityLabel}} · screen output</h3>
              <p>Cleanup, print layout and Office review drafts</p>
            </div>
            <button
              class="membership-button secondary"
              @click="openTool('Output options')"
            >
              Output options
            </button>
          </section>
        </section>
        <aside class="build-card build-summary">
          <h2>Bundle summary</h2>
          <dl>
            <div>
              <dt>Included documents</dt>
              <dd>{{ included.length }}</dd>
            </div>
            <div>
              <dt>Source pages</dt>
              <dd>{{ sourcePages }}</dd>
            </div>
            <div>
              <dt>Generated pages</dt>
              <dd>{{ plan ? plan.combined_pages - sourcePages : "—" }}</dd>
            </div>
            <div>
              <dt>Planned physical pages</dt>
              <dd>{{ plan?.combined_pages ?? "—" }}</dd>
            </div>
            <div>
              <dt>Volumes</dt>
              <dd>{{ plan?.volumes.length ?? "—" }}</dd>
            </div>
            <div>
              <dt>Working revisions</dt>
              <dd>{{ included.length }} recorded at build</dd>
            </div>
          </dl>
          <div class="build-limit">
            <small>OUTPUT SIZE LIMIT</small
            ><strong>{{
              profile?.max_file_mb
                ? `${profile.max_file_mb} MB per PDF`
                : "No configured byte limit"
            }}</strong>
            <p>
              {{ maxPages }} pages per volume ·
              {{
                keepGroups ? "Keep groups together" : "Split between documents"
              }}
            </p>
          </div>
          <button
            class="membership-button quiet"
            @click="openTool('Plan volumes')"
          >
            Plan volumes
          </button>
        </aside>
      </div>
      <footer class="build-footer">
        <span
          >This build keeps a fixed copy of the selected revisions and
          settings.</span
        ><button
          class="membership-button primary"
          :disabled="blocked || busy || !included.length"
          @click="emit('build')"
        >
          <Package :size="16" />Build bundle
        </button>
      </footer>
      <div class="build-secondary">
        <button class="text-tool" @click="emit('membership')">
          Bundle membership</button
        ><button class="text-tool" @click="openTool('Technical output tools')">Technical marks &amp; Part extracts</button
        ><button v-if="artifact" class="text-tool" @click="showResult">
          Previous generated output</button
        ><span v-if="dirty">Unsaved settings — saved before build</span
        ><span v-if="planError" role="status"
          >Page plan unavailable: {{ planError }}</span
        >
      </div>
    </template>
    <template v-else-if="view === 'job'">
      <section class="build-card build-job-card">
        <span
          class="build-state"
          :class="{ danger: job?.status === 'failed' }"
          >{{
            cancelPending
              ? "Cancellation requested"
              : running
                ? "In progress"
                : job?.status === "interrupted"
                  ? "Interrupted by engine restart"
                  : job?.status === "cancelled"
                    ? "Build cancelled"
                    : "Build failed"
          }}</span
        >
        <div class="build-progress-title">
          <h2>
            {{
              running
                ? job?.message
                : job?.status === "cancelled"
                  ? "No new bundle was finalized"
                  : job?.status === "interrupted"
                    ? "The local engine stopped before completion"
                    : "The bundle could not be generated"
            }}
          </h2>
          <strong v-if="running">{{ job?.progress }}%</strong>
        </div>
        <progress
          v-if="running"
          :value="job?.progress"
          max="100"
          aria-label="Build progress"
        />
        <div class="build-job-grid">
          <div>
            <p v-if="running">
              {{
                cancelPending
                  ? "Waiting for the backend to confirm cancellation. A completed publication cannot be undone."
                  : "Processing the submitted membership, working revisions and saved settings."
              }}
            </p>
            <p v-else class="build-error-detail">
              {{
                job?.error ||
                "Inspect the current inputs and start a new build when ready."
              }}
            </p>
            <details v-if="!running">
              <summary>Technical details</summary>
              <p>Job {{ job?.id }} · {{ job?.status }}</p>
              <p>
                No earlier generated file or imported original is replaced by
                this attempt.
              </p>
            </details>
            <p>A failed build has no new output to validate.</p>
            <div class="build-actions" v-if="!running">
              <button
                class="membership-button secondary"
                @click="emit('documents')"
              >
                Inspect working documents</button
              ><button
                class="membership-button secondary"
                @click="emit('checks')"
              >
                Run checks</button
              ><button
                class="membership-button quiet"
                :disabled="busy || blocked"
                @click="
                  job?.status === 'interrupted' && !job.retry_job_id
                    ? emit('retry', job)
                    : emit('build')
                "
              >
                Retry build
              </button>
            </div>
          </div>
          <aside class="build-card">
            <h3>Saved input contract</h3>
            <p>
              Membership and working revisions are checked when the job starts.
            </p>
            <p>Settings are saved before submission.</p>
            <button
              v-if="artifact"
              class="text-tool"
              @click="emit('review', artifact.id, 'combined.pdf')"
            >
              Earlier build {{ artifact.id.slice(0, 8) }} stays available
            </button>
          </aside>
        </div>
      </section>
      <footer class="build-footer">
        <button class="membership-button secondary" @click="emit('documents')">
          Continue working</button
        ><button
          v-if="running"
          class="membership-button secondary"
          :disabled="cancelPending || blocked"
          @click="emit('cancel')"
        >
          {{ cancelPending ? "Cancelling…" : "Cancel build" }}</button
        ><button
          v-else
          class="membership-button secondary"
          @click="screen = 'settings'"
        >
          Build settings
        </button>
      </footer>
    </template>
    <template v-else-if="view === 'stale' && artifact">
      <div class="build-strip warning">
        Effective inputs have changed · Earlier output files remain unchanged
        <button class="text-tool" @click="screen = 'changes'">
          Review changes
        </button>
      </div>
      <section class="build-card stale-summary">
        <div>
          <h2>Earlier bundle · {{ artifact.id.slice(0, 8) }}</h2>
          <p>
            {{ artifact.pages }} physical pages · {{ artifact.volumes }} volumes
            · {{ prettyDate(artifact.created_at) }}
          </p>
        </div>
        <div>
          <h2>Current preparation</h2>
          <p>
            {{ sourcePages }} source pages · {{ included.length }} included
            documents
          </p>
        </div>
      </section>
      <h2 class="stale-question">How should this update be prepared?</h2>
      <div class="stale-options">
        <section class="build-card">
          <Package />
          <h2>Rebuild bundle</h2>
          <p>
            Recalculate references for all included documents. The earlier
            bundle stays unchanged.
          </p>
          <button
            class="membership-button primary"
            @click="screen = 'settings'"
          >
            Rebuild
          </button>
        </section>
        <section class="build-card">
          <FileText />
          <h2>Supplementary bundle</h2>
          <p>
            A separate supplement with selected new material is not available
            yet.
          </p>
          <button disabled class="membership-button secondary">
            Create supplement · unavailable
          </button>
        </section>
        <section class="build-card">
          <FileText />
          <h2>Preserve existing references</h2>
          <p>Insertion labels and preservation policies are planned for B04.</p>
          <button disabled class="membership-button secondary">
            Preview inserts · unavailable
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
        :max-pages="maxPages"
        :keep-groups="keepGroups"
        :refresh-key="refreshKey"
      />
      <footer class="build-footer">
        <button class="membership-button secondary" @click="emit('documents')">
          Open working copy</button
        ><button
          class="membership-button secondary"
          @click="emit('review', artifact.id, 'combined.pdf')"
        >
          Open earlier bundle</button
        ><button class="membership-button quiet" @click="screen = 'stale'">
          Choose update policy</button
        ><button class="membership-button primary" @click="emit('checks')">
          Run checks
        </button>
      </footer>
      <p class="build-secondary">
        Side-by-side PDF comparison is not available yet.
      </p></template
    >
    <template v-else>
      <div
        v-if="artifact"
        class="build-strip"
        :class="{ warning: artifact.stale }"
      >
        {{
          artifact.stale
            ? "Previous generated output · Stale"
            : "Generated output saved"
        }}
        · {{ artifact.pages }} physical pages · {{ artifact.volumes }} volumes ·
        {{
          qaPending
            ? "Loading checks"
            : qa?.can_release
              ? "Technical and human review recorded"
              : qa?.can_download
                ? "Technical checks complete · human review required"
                : "Output checks need attention"
        }}
      </div>
      <div
        v-if="artifact"
        class="build-result-grid"
        :class="{ 'storage-grid': storage }"
      >
        <aside v-if="storage" class="build-card storage-categories">
          <small>MATTER FILES</small>
          <p>Imported originals</p>
          <p>Working revisions</p>
          <strong>Generated bundles · {{ artifacts.length }}</strong>
          <p>
            Imported originals remain unchanged. Edits are stored in working
            revisions.
          </p>
          <slot name="storage-tools" />
        </aside>
        <section class="build-card generated-files">
          <header>
            <h2>Generated files</h2>
            <select
              v-model="selectedId"
              aria-label="Saved build"
              @change="!storage && selectBuild()"
            >
              <option v-for="item in artifacts" :key="item.id" :value="item.id">
                {{ item.id.slice(0, 8) }} · {{ prettyDate(item.created_at)
                }}{{ item.stale ? " · Stale" : "" }}
              </option>
            </select>
          </header>
          <div class="build-files-scroll">
            <table>
              <thead>
                <tr>
                  <th>File</th>
                  <th>Measured size</th>
                  <th>State</th>
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
                  <td :title="`${file.bytes.toLocaleString()} actual bytes`">{{ bytesLabel(file.bytes) }}</td>
                  <td>Generated</td>
                </tr>
              </tbody>
            </table>
          </div>
          <p>Build {{ artifact.id }} · Read only</p>
        </section>
        <aside v-if="!storage" class="build-card build-review-checks">
          <h2>Review before sharing</h2>
          <p>
            Generated files are available. Inspect the pages you will share.
          </p>
          <label
            v-for="(label, key) in qa?.review_labels"
            :key="key"
            class="build-human-item"
            ><input
              type="checkbox"
              :checked="qa?.review.items[key]"
              :disabled="qaPending || blocked"
              @change="
                saveReview(
                  String(key),
                  ($event.target as HTMLInputElement).checked,
                )
              "
            />{{ label }}</label
          ><button
            class="membership-button secondary"
            @click="checksOpen = true"
          >
            View output checks</button
          ><small>{{
            qaPending
              ? "Loading review…"
              : "Review status stays with this build."
          }}</small>
          <p v-if="qaError" role="alert">{{ qaError }}</p>
        </aside>
      </div>
      <p v-else class="build-card">
        No saved builds. Include documents and build a bundle to create an
        output.
      </p>
      <footer class="build-footer">
        <button class="membership-button secondary" @click="emit('storage')">
          Local storage</button
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
          {{ storage ? "Build bundle" : "Rebuild" }}</button
        ><button
          v-if="artifact?.stale"
          class="text-tool"
          @click="screen = 'changes'"
        >
          Review changes</button
        ><span>Later edits leave this build unchanged.</span>
      </footer>
      <details v-if="artifact" ref="extraTools" class="build-extra-tools">
        <summary>
          More output tools · Office drafts, print and protected copies
        </summary>
        <div role="group" aria-label="Office review exports">
          <slot name="output-tools" :artifact="artifact" />
        </div>
      </details>
    </template>
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
      ref="toolDialog"
      class="build-tool-dialog"
      :aria-label="tool"
      @cancel.prevent="closeTool"
    >
      <header>
        <h2>{{ tool }}</h2>
        <button
          class="icon-btn"
          aria-label="Close build tool"
          @click="closeTool"
        >
          <X :size="18" />
        </button>
      </header>
      <div v-show="tool === 'Preview'"><slot name="preview" /></div>
      <div v-show="tool === 'Plan volumes'"><slot name="volumes" /></div>
      <div v-if="tool === 'Technical output tools'">
        <slot name="settings" />
      </div>
    </dialog>
  </section>
</template>
