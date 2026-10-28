<script setup lang="ts">
import {
  computed,
  nextTick,
  onMounted,
  onUnmounted,
  reactive,
  ref,
  watch,
} from "vue";
import {
  ChevronDown,
  LayoutGrid,
  ListTree,
  ListChecks,
  Box,
  Files,
  HardDrive,
  Archive,
  ArrowDownToLine,
  ArrowRight,
  BookMarked,
  BookOpen,
  BriefcaseBusiness,
  Check,
  CheckCircle2,
  CircleAlert,
  CircleHelp,
  CloudOff,
  Ellipsis,
  FileCheck2,
  FilePlus2,
  FileStack,
  FileText,
  FolderOpen,
  GripVertical,
  Info,
  Layers3,
  LoaderCircle,
  LockKeyhole,
  Menu,
  PanelLeftClose,
  PanelLeftOpen,
  Plus,
  RefreshCw,
  Redo2,
  ScanText,
  Search,
  Settings2,
  ShieldAlert,
  ShieldCheck,
  Undo2,
  UploadCloud,
  UserRound,
  WandSparkles,
  X,
} from "lucide-vue-next";
import PdfViewer from "./components/PdfViewer.vue";
import DocumentNavigator from "./components/DocumentNavigator.vue";
import DocumentThumbnail from "./components/DocumentThumbnail.vue";
import PasswordInput from "./components/PasswordInput.vue";
import PageTools from "./components/PageTools.vue";
import BundleMembership from "./components/BundleMembership.vue";
import BundlePreview from "./components/BundlePreview.vue";
import BuildComparison from "./components/BuildComparison.vue";
import FilingReview from "./components/FilingReview.vue";
import IndexImport from "./components/IndexImport.vue";
import BundleStructure from "./components/BundleStructure.vue";
import OutputPresets from "./components/OutputPresets.vue";
import DocumentTable from "./components/DocumentTable.vue";
import EngineNotice from "./components/EngineNotice.vue";
import { engineState, engineBlocked, engineReviewReason, autosaveHeld, disconnectEngine } from "./lib/engineConnection";
import FindingReviewPanel from './components/FindingReviewPanel.vue';
import PreflightWorkspace from './components/PreflightWorkspace.vue';
import OutputChecksDialog from './components/OutputChecksDialog.vue';
import BuildWorkspace from "./components/BuildWorkspace.vue";
import BundleExportDialog from "./components/BundleExportDialog.vue";
import ExportPdfReview from "./components/ExportPdfReview.vue";
import MatterForm from "./components/MatterForm.vue";
import MatterHome from "./components/MatterHome.vue";
import CourtProfileGuide from "./components/CourtProfileGuide.vue";
import { openUrl } from "@tauri-apps/plugin-opener";
import { VueDraggable } from "vue-draggable-plus";
import {
  ApiError,
  api,
  downloadExport,
  downloadOfficeReview,
  downloadProtectedPdf,
  downloadMatter,
  inTauri,
  printableExport,
  printableWorkingPdf,
  restoreMatter,
  uploadDocument,
} from "./lib/api";
import { bytesLabel, friendlyError, prettyDate } from "./lib/format";
import {
  applyAppearance,
  readAppearance,
  type Theme,
  type UiScale,
} from "./lib/appearance";
import {
  confirmSignatureTransformation,
  signatureRiskDocuments,
  signatureStatusLabel,
} from "./lib/signatures";
import type {
  Check as PreflightCheck,
  CourtProfile,
  DocKind,
  ExportManifest,
  Job,
  ManualBookmark,
  Matter,
  MatterDocument,
  MatterNew,
  PageSearchResult,
} from "./lib/types";

type View =
  | "matters"
  | "overview"
  | "documents"
  | "index"
  | "build"
  | "validation"
  | "storage"
  | "rules"
  | "about";
type DocumentDraft = {
  title: string;
  kind: DocKind;
  part: number;
  label: string;
  group_key: string;
  document_date: string;
  date_confirmed: boolean;
  visible_signature_status: MatterDocument["visible_signature_status"];
};
type WorkspaceTab = {
  id: string;
  matterId: string;
  documentId: string | null;
  view: View;
  selectedDocumentId: string | null;
  documentDraft?: DocumentDraft;
  settingsDraft?: { max_pages_per_volume: number; keep_groups_intact: boolean };
  hadUnsavedChanges?: boolean;
  title?: string;
  detailsOpen?: boolean;
};
const workspaceTabs = ref<WorkspaceTab[]>([]);
const activeTabId = ref<string | null>(null);
const tabDiscardRequest = ref<{
  message: string;
  resolve: (allow: boolean) => void;
  focusReturn: HTMLElement | null;
  discardAll: boolean;
  canSave: boolean;
} | null>(null);
const membershipOpen = ref(false);
const membershipDirty = ref(false);
const membershipCollapsed = ref(false);
const membershipPanel = ref<InstanceType<typeof BundleMembership>|null>(null);
let membershipInvoker: HTMLElement|null = null;
async function openMembership() {
  if (!(await guardArtifactTransition('Bundle membership'))) return;
  membershipInvoker = document.activeElement as HTMLElement;
  membershipOpen.value = true;
  membershipCollapsed.value = window.innerWidth < 1200;
}
async function leaveMembership() {
  if (!membershipOpen.value) return true;
  if (membershipPanel.value?.isSaving()) return false;
  if (membershipDirty.value && !(await askTabDiscard('Apply or discard membership changes before leaving?', false))) return false;
  membershipPanel.value?.discard();
  membershipDirty.value = false;
  membershipOpen.value = false;
  await nextTick();
  const target = membershipInvoker?.isConnected ? membershipInvoker : document.querySelector<HTMLElement>('.membership-status-entry');
  target?.focus({preventScroll:true});
  return true;
}
async function membershipApplied() { await loadMatterResources(); }
async function openMembershipDocument(id:string) {
  if (await leaveMembership()) await openDocumentTab(id);
}
const includedDocuments = computed(()=>docs.value.filter(d=>d.bundle_included));
const inclusionSummary = computed(()=>`${includedDocuments.value.length} included / ${includedDocuments.value.reduce((sum,d)=>sum+d.page_count,0)} source pages · ${docs.value.length-includedDocuments.value.length} reference-only`);
const navigationSaving = ref(false);
const navigationError = ref("");
function askTabDiscard(message: string, discardAll = false, canSave = true): Promise<boolean> {
  if (tabDiscardRequest.value) return Promise.resolve(false);
  navigationError.value = "";
  return new Promise((resolve) => {
    tabDiscardRequest.value = {
      message,
      discardAll,
      canSave,
      resolve,
      focusReturn: document.activeElement as HTMLElement | null,
    };
    void nextTick(() =>
      document
        .querySelector<HTMLButtonElement>(".tab-discard-dialog button")
        ?.focus(),
    );
  });
}
function answerTabDiscard(allow: boolean) {
  if (navigationSaving.value) return;
  const request = tabDiscardRequest.value;
  if (allow && request?.discardAll) resetDrafts();
  tabDiscardRequest.value = null;
  request?.resolve(allow);
  if (!allow) void nextTick(() => request?.focusReturn?.focus());
}
function trapNavigationFocus(event: KeyboardEvent) {
  const buttons = [...(event.currentTarget as HTMLElement).querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];
  const first = buttons[0], last = buttons[buttons.length - 1];
  if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
  else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
}
async function saveBeforeNavigation() {
  const request = tabDiscardRequest.value;
  if (!request || navigationSaving.value || busy.value) return;
  navigationSaving.value = true;
  navigationError.value = '';
  try {
    if (tableDirty.value && !(await documentTablePanel.value?.save())) throw new Error('Review changes could not be saved. Check the selected fields and source context.');
    if (membershipDirty.value && !(await membershipPanel.value?.apply(false))) throw new Error('Membership could not be applied.');
    if (documentDirty.value && !(await saveDocument(true))) throw new Error('Document details could not be saved.');
    if (settingsDirty.value && !(await savePolicy(true))) throw new Error('Bundle settings could not be saved.');
    if (formattingDirty.value && !(await outputPresetsPanel.value?.saveDraft())) throw new Error('Output policy could not be saved.');
    if (pdfViewer.value?.hasPendingDraft() && !(await pdfViewer.value.savePendingDraft()))
      throw new Error('The PDF tool draft could not be saved. Complete its required fields or placement before leaving.');
    if (documentDirty.value || settingsDirty.value || formattingDirty.value) throw new Error('Newer unsaved changes remain. Save again before leaving.');
    tabDiscardRequest.value = null;
    request.resolve(true);
    await resumeAutosaveWhenClean();
  } catch (err) { navigationError.value = `${friendlyError(err)} Your work and pending destination are retained.`; }
  finally { navigationSaving.value = false; }
}
async function guardArtifactTransition(destination: string) {
  if (!tableDirty.value && !documentDirty.value && !settingsDirty.value && !formattingDirty.value && !pdfViewer.value?.hasPendingDraft()) return true;
  if (!(await askTabDiscard(`Save changes before opening ${destination}?`, true))) return false;
  pdfViewer.value?.discardPendingDraft();
  outputPresetsPanel.value?.discard();
  formattingDirty.value = false;
  return true;
}
const activeTab = computed(
  () => workspaceTabs.value.find((tab) => tab.id === activeTabId.value) || null,
);
const tabIsActive = (tab: WorkspaceTab) =>
  activeTabId.value === tab.id &&
  !["matters", "rules", "about"].includes(activeView.value);
const tabLabel = (tab: WorkspaceTab) =>
  tab.documentId
    ? (tab.matterId === currentMatterId.value
        ? docs.value.find((doc) => doc.id === tab.documentId)?.title
        : null) ||
      tab.title ||
      "Document"
    : matterList.value.find((matter) => matter.id === tab.matterId)?.name ||
      "Matter";
const matterList = ref<Matter[]>([]);
const archivedMatters = ref<Matter[]>([]);
const currentMatterId = ref<string | null>(null);
const currentMatter = computed(
  () => matterList.value.find((m) => m.id === currentMatterId.value) || null,
);
const docs = ref<MatterDocument[]>([]);
const archivedDocs = ref<MatterDocument[]>([]);
const storageUsage = ref({
  bytes: 0,
  by_category: { originals: 0, working: 0, exports: 0, other: 0 },
});
const profileList = ref<CourtProfile[]>([]);
const exportsList = ref<ExportManifest[]>([]);
const recentJobs = ref<Job[]>([]);
const previewRefreshKey = ref(0);
const bookmarkPreset = ref<{ documentId: string; sequence: number }>({
  documentId: "",
  sequence: 0,
});
const history = ref({
  can_undo: false,
  can_redo: false,
  undo_count: 0,
  redo_count: 0,
  undo_label: null as string | null,
  redo_label: null as string | null,
});
const undoDescription = computed(() =>
  history.value.undo_label
    ? `Undo: ${history.value.undo_label}. Applies to the last change anywhere in this matter.`
    : "Undo last matter change. No change available.",
);
const redoDescription = computed(() =>
  history.value.redo_label
    ? `Redo: ${history.value.redo_label}. Restores the next undone change anywhere in this matter.`
    : "Redo matter change. No change available.",
);
const guideReloadKey = ref(0);
const filingGuide = ref<InstanceType<typeof CourtProfileGuide> | null>(null);
const checks = ref<PreflightCheck[]>([]);
const manualBookmarks = ref<ManualBookmark[]>([]);
const selectedDocId = ref<string | null>(null);
const pdfViewer = ref<InstanceType<typeof PdfViewer> | null>(null);
const selectedDoc = computed(
  () => docs.value.find((d) => d.id === selectedDocId.value) || null,
);
type OcrRun = {
  language: string;
  provider: string;
  pymupdf_version: string;
  tesseract_cli_version: string | null;
  model_sha256: Record<string, string>;
  pages_ocr_applied: number;
  applied_at: string;
  applied_revision: number;
};
const selectedOcrRuns = computed<OcrRun[]>(() => {
  try {
    const runs = JSON.parse(selectedDoc.value?.ocr_provenance_json || "[]");
    return Array.isArray(runs) ? runs : [];
  } catch {
    return [];
  }
});
const detailsOpen = ref(false);
const compactDocuments = ref(false);
const documentDetailsPanel = ref<HTMLElement | null>(null);
let detailsTrigger: HTMLElement | null = null;
async function toggleDocumentDetails() {
  if (detailsOpen.value) return closeDocumentDetails();
  detailsTrigger = document.activeElement as HTMLElement | null;
  detailsOpen.value = true;
  if(preparationVisible.value && compactDocuments.value) preparationListOpen.value = false;
  await nextTick();
  documentDetailsPanel.value?.querySelector<HTMLButtonElement>("button")?.focus({preventScroll:true});
  window.requestAnimationFrame(() => {
    if (detailsOpen.value && document.activeElement === detailsTrigger)
      documentDetailsPanel.value?.querySelector<HTMLButtonElement>("button")?.focus({preventScroll:true});
  });
}
async function closeDocumentDetails() {
  detailsOpen.value = false;
  await nextTick();
  const trigger = detailsTrigger?.isConnected ? detailsTrigger :
    documentsWorkspace.value?.querySelector<HTMLElement>('[aria-controls="document-details-panel"]');
  const menu = trigger?.closest('details');
  (menu && !menu.open ? menu.querySelector<HTMLElement>('summary') : trigger)?.focus({preventScroll:true});
}
const preparationMode = ref(false);
const preparationVisible = computed(() => preparationMode.value && activeView.value === 'documents' && !documentTableOpen.value);
const preparationListOpen = ref(true);
async function showPreparation() {
  if (!(await leaveMembership())) return;
  if (pdfViewer.value?.hasPendingDraft() && !(await guardArtifactTransition('Documents'))) return;
  await showView('documents');
  if (activeView.value !== 'documents') return;
  await nextTick();
  fitDocumentsWorkspace();
  preparationMode.value = true;
  preparationListOpen.value = !compactDocuments.value;
  readerFocus.value = true;
  detailsOpen.value = !compactDocuments.value;
  editorExpanded.value = false;
  await nextTick();
  fitDocumentsWorkspace();
}
async function closePreparationList() {
  preparationListOpen.value = false;
  await nextTick();
  documentsWorkspace.value?.querySelector<HTMLButtonElement>('.preparation-list-toggle')?.focus({preventScroll:true});
}
async function selectPreparationDocument(id:string) {
  await openDocumentTab(id);
  if (selectedDocId.value !== id) return;
  preparationMode.value = true;
  detailsOpen.value = !compactDocuments.value;
}
function openEditor() {
  preparationMode.value = false;
  detailsOpen.value = false;
  readerFocus.value = true;
  void nextTick(() => documentsWorkspace.value?.querySelector<HTMLElement>(".viewer-root")?.focus({preventScroll:true}));
}
async function openPreparationSearch() {
  if (membershipOpen.value) { if (await leaveMembership()) await openDocumentSearch(); return; }
  preparationMode.value = false;
  readerFocus.value = true;
  await toggleDocumentBrowser();
}
const readerFocus = ref(true);
const workspaceMenuOpen = ref(false);
const tabsMenuOpen = ref(false);
const editorExpanded = ref(false);
async function toggleDocumentBrowser() {
  readerFocus.value = !readerFocus.value;
  if (!readerFocus.value && compactDocuments.value) detailsOpen.value = false;
  await nextTick();
  if (!readerFocus.value) documentsWorkspace.value?.querySelector<HTMLInputElement>('[aria-label="Search documents"]')?.focus({preventScroll:true});
  else documentsWorkspace.value?.querySelector<HTMLButtonElement>('.document-browser-trigger')?.focus({preventScroll:true});
}
async function closeDocumentBrowser() {
  readerFocus.value = false;
  await toggleDocumentBrowser();
}
async function browseMatterDocuments() {
  await showView('documents');
  if (activeView.value !== 'documents') return;
  readerFocus.value = true;
  await toggleDocumentBrowser();
}
async function activateTabFromMenu(tab:WorkspaceTab) {
  await activateTab(tab);
  if (activeTabId.value === tab.id && activeView.value === tab.view) tabsMenuOpen.value = false;
}

let previousReaderFocus = false;
function toggleEditorExpanded() {
  if (editorExpanded.value) {
    editorExpanded.value = false;
    readerFocus.value = selectedDocId.value ? previousReaderFocus : false;
  } else {
    previousReaderFocus = readerFocus.value;
    readerFocus.value = true;
    editorExpanded.value = true;
  }
}

const documentTableOpen = ref(false);
const documentTablePanel = ref<InstanceType<typeof DocumentTable>|null>(null);
const tableDirty = ref(false);
const pageToolsOpen = ref(false);
const pageToolsDialog = ref<HTMLDialogElement | null>(null);
const documentsWorkspace = ref<HTMLElement | null>(null);
const documentsWorkspaceHeight = ref<number | null>(null);
let pendingDocumentSearchFocus = false;
function restoreDocumentSearchFocus() {
  if (!pendingDocumentSearchFocus || activeView.value !== 'documents') return;
  const input = documentsWorkspace.value?.querySelector<HTMLInputElement>('[aria-label="Search documents"]');
  if (input?.getClientRects().length) {
    input.focus({preventScroll:true});
    pendingDocumentSearchFocus = false;
  }
}
async function openDocumentSearch() {
  pendingDocumentSearchFocus = true;
  if (activeView.value !== 'documents') await showView('documents');
  if (activeView.value !== 'documents') { pendingDocumentSearchFocus = false; return; }
  preparationMode.value = false;
  readerFocus.value = false;
  workspaceMenuOpen.value = false;
  tabsMenuOpen.value = false;
  if (compactDocuments.value) detailsOpen.value = false;
  await nextTick();
  restoreDocumentSearchFocus();
}
watch(documentsWorkspace, restoreDocumentSearchFocus, {flush:'post'});
const activeView = ref<View>("documents");
async function focusMain() {
  await nextTick();
  const regions = document.querySelectorAll<HTMLElement>("#main-content");
  regions[regions.length - 1]?.focus();
}
watch([activeView, selectedDocId, documentTableOpen], () => {
  if (
    editorExpanded.value &&
    (activeView.value !== "documents" ||
      !selectedDocId.value ||
      documentTableOpen.value)
  )
    toggleEditorExpanded();
});
const sidebarCollapsed = ref(false);
const smallScreen = ref(window.matchMedia("(max-width: 900px)").matches);
const mobileNavOpen = ref(false);
const searching = ref("");
const pageQuery = ref("");
const pageSearch = ref<PageSearchResult | null>(null);
const pageSearchBusy = ref(false);
const pageSearchError = ref("");
const pageTarget = ref<{ documentId: string; page: number; sequence: number; revision?: number }>({
  documentId: "",
  page: 1,
  sequence: 0,
});
let searchTimer: ReturnType<typeof setTimeout> | null = null;
let searchSequence = 0;
const creating = ref(false);
const modalOpen = ref(false);
const appearance = readAppearance();
const theme = ref<Theme>(appearance.theme);
const uiScale = ref<UiScale>(appearance.scale);
const busy = ref(false);
const hoverUpload = ref(false);
const apiStatus = ref<"connecting" | "online" | "offline">("connecting");
const vaultState = ref<
  | "checking"
  | "setup"
  | "locked"
  | "unlocked"
  | "show_recovery"
  | "change_password"
>("checking");
const vaultEnabled = ref(false);
const vaultCredential = ref("");
const vaultConfirm = ref("");
const vaultRecoveryMode = ref(false);
const vaultRecoveryKey = ref("");
const vaultBusy = ref(false);
const vaultError = ref("");
const protectedRestoreFile = ref<File | null>(null);
const protectedRestoreCredential = ref("");
const protectedRestoreRecovery = ref(false);
const protectedRestoreError = ref("");
const toast = ref<{
  message: string;
  kind: "success" | "error" | "info";
} | null>(null);
const activeJob = ref<Job | null>(null);
const cancelPending = ref(false);
const buildSubmitting = ref(false);
const buildWorkspace = ref<InstanceType<typeof BuildWorkspace> | null>(null);
const outputChecksArtifact = ref<ExportManifest|null>(null);
const preflightFilter = ref('all');
const exportDialogArtifact = ref<ExportManifest | null>(null);
const buildShell = computed(() => !!currentMatterId.value && ['index','build','storage','validation'].includes(activeView.value));
const expandedBuildShell = computed(() => buildShell.value && !reviewingPdf.value);

const retryOcrLanguage = ref<"eng" | "hin" | "eng+hin">("eng");
const savingExport = ref(false);
const printRequest = ref<
  | (
      | { kind: "export"; exportId: string; filename: string }
      | {
          kind: "working";
          documentId: string;
          revision: number;
          filename: string;
          acknowledged: boolean;
        }
    )
  | null
>(null);
const printPages = ref("all");
const printOrientation = ref<"source" | "portrait" | "landscape">("source");
const printScale = ref<"fit" | "actual">("fit");
const printBusy = ref(false);
const printError = ref("");
const printPreviewUrl = ref("");
const printFrame = ref<HTMLIFrameElement | null>(null);
const reviewingPdf = ref<{ exportId: string; filename: string; originalId?: string } | null>(null);
const artifactOptions = computed(() => [
  {value:'working',label:'Working Copy · Editable'},
  {value:'original',label:'Imported original · Read only'},
  ...exportsList.value.flatMap(build => build.files.filter(file => file.name.endsWith('.pdf')).map(file => ({
    value:`export:${build.id}:${file.name}`,
    label:`Build ${build.id.slice(0,8)} · ${file.name}${build.stale ? ' · Stale' : ''} · Read only`,
  }))),
]);
async function retryWorkingReader() {
  if (engineBlocked.value || !(await guardArtifactTransition('this working PDF again'))) return;
  await pdfViewer.value?.reloadReader();
}
async function switchArtifact(value: string) {
  if (value === 'working') {
    reviewingPdf.value = null;
    if (selectedDocId.value && activeView.value !== 'documents') await openDocumentTab(selectedDocId.value);
    return;
  }
  const selected = selectedDoc.value;
  if (!selected || !(await guardArtifactTransition(artifactOptions.value.find(item => item.value === value)?.label || 'artifact'))) return;
  if (value === 'original') reviewingPdf.value = {exportId:'', filename:selected.original_name, originalId:selected.id};
  else {
    const [,exportId,filename] = value.split(':');
    if (exportsList.value.some(build => build.id === exportId && build.files.some(file => file.name === filename)))
      reviewingPdf.value = {exportId,filename};
  }
}
const preflightContext = ref<{finding:PreflightCheck, tabId:string, matterId:string, focusId:string,outputBuildId?:string}|null>(null);
let findingNavigationSequence=0;
async function openFinding(check: PreflightCheck, outputBuildId?: string) {
  if (!check.document_id || !currentMatterId.value || !activeTabId.value) return;
  const call=++findingNavigationSequence;
  const origin = {outputBuildId, finding:{...check},tabId:activeTabId.value,matterId:currentMatterId.value,focusId:`finding-${check.finding_id}`};
  try {
    const currentDocs = await api<MatterDocument[]>(`/matters/${origin.matterId}/documents`);
    const doc = currentDocs.find(item => item.id === check.document_id);
    if (!doc) throw new Error("This finding’s document is unavailable. Run checks again.");
    if (doc.archived_at || (check.target_revision !== undefined && doc.revision !== check.target_revision) ||
        (check.page_index !== undefined && (check.page_index < 0 || check.page_index >= doc.page_count)))
      throw new Error('This finding targets an unavailable or older revision. Run checks again; no substitute page was opened.');
    if (currentMatterId.value !== origin.matterId || activeTabId.value !== origin.tabId || call !== findingNavigationSequence) return;
    docs.value = currentDocs;
    await openDocumentTab(doc.id, check.page_index === undefined ? undefined : check.page_index + 1, check.target_revision);
    if (selectedDocId.value === doc.id && activeView.value === 'documents') {
      preparationMode.value = false;
      detailsOpen.value = false;
      preflightContext.value = origin;
      if (check.artifact === 'original') await switchArtifact('original');
    }
  } catch (err) { issue(err); }
}
async function returnToPreflight() {
  const context = preflightContext.value;
  if (!context) return;
  const tab = workspaceTabs.value.find(item => item.id === context.tabId);
  if (!tab) { notify('The originating Preflight tab was closed.', 'error'); return; }
  tab.view = context.outputBuildId ? 'build' : 'validation';
  await activateTab(tab);
  if (activeTabId.value !== tab.id) return;
  pendingFindingFocusId = context.focusId;
  await validate();
  if(context.outputBuildId) {
    // Source edits cannot repair an immutable output. Recheck its actual bytes,
    // retaining the originating historical run/selection for review.
    try { await api(`/matters/${context.matterId}/exports/${context.outputBuildId}/validation/run`,'POST',{}); }
    catch (err) { issue(err); }
    if (activeTabId.value !== tab.id || currentMatterId.value !== context.matterId) return;
    outputChecksArtifact.value=exportsList.value.find(b=>b.id===context.outputBuildId)||null;
  }
  await nextTick();
  restoreFindingFocus();
  preflightContext.value = null;
}
const pdfImportRequest = ref<{ name: string; error: string } | null>(null);
const pdfImportPassword = ref("");
let pdfImportResolve: ((password: string | null) => void) | null = null;
const protectedExportRequest = ref<{
  exportId: string;
  filename: string;
} | null>(null);
const protectedExportPassword = ref("");
const protectedExportConfirm = ref("");
const protectedExportError = ref("");
const fileInput = ref<HTMLInputElement | null>(null);
const matterFileInput = ref<HTMLInputElement | null>(null);
const docEditor = reactive({
  title: "",
  kind: "other" as DocKind,
  part: 1,
  label: "",
  group_key: "",
  document_date: "",
  date_confirmed: false,
  visible_signature_status:
    "unreviewed" as MatterDocument["visible_signature_status"],
});
const matterEditor = reactive({
  max_pages_per_volume: 300,
  keep_groups_intact: true,
});
const documentAutosaveState = ref<"idle" | "saving" | "failed">("idle");
const settingsAutosaveState = ref<"idle" | "saving" | "failed">("idle");
let documentAutosaveTimer: ReturnType<typeof setTimeout> | null = null;
let settingsAutosaveTimer: ReturnType<typeof setTimeout> | null = null;
const formattingDirty = ref(false);
const outputPresetsPanel = ref<InstanceType<typeof OutputPresets> | InstanceType<typeof BundleStructure> | null>(null);
const structureMode = ref("map");
async function openStructure(name:string) { structureMode.value=name; await showView("index"); }
async function openStructurePolicy(name:string) { if (!(await guardArtifactTransition(name))) return; await showView("build"); await nextTick(); buildWorkspace.value?.openTool(name); }
async function structureModified() { await refreshMatters(); resetDrafts(); await loadMatterResources(); }
let jobTimer: ReturnType<typeof setInterval> | null = null;
let jobPollSequence=0;
watch(currentMatterId,()=>{jobPollSequence++;if(jobTimer)clearInterval(jobTimer);jobTimer=null;activeJob.value=null;cancelPending.value=false;exportDialogArtifact.value=null;});
let toastTimer: ReturnType<typeof setTimeout> | null = null;
let healthTimer: ReturnType<typeof setInterval> | null = null;
let connecting = false;

const filteredDocs = computed(() =>
  docs.value.filter((d) =>
    (d.title + " " + d.label + " " + d.kind)
      .toLowerCase()
      .includes(searching.value.toLowerCase()),
  ),
);
const selectedProfile = computed(
  () =>
    profileList.value.find(
      (p) => p.id === currentMatter.value?.court_profile,
    ) || profileList.value[0],
);
const delhiGuidance = computed(
  () =>
    profileList.value.find((p) => p.id === "delhi-hc-illustrative")
      ?.filing_guidance,
);
const isJobActive = computed(
  () =>
    !!activeJob.value &&
    (activeJob.value.status === "running" ||
      activeJob.value.status === "queued"),
);
const activeExport = computed(() => exportsList.value[0] || null);
const comparisonExportId = ref("");
const comparisonExport = computed(
  () =>
    exportsList.value.find((item) => item.id === comparisonExportId.value) ||
    activeExport.value,
);
const exportsPanel = ref<HTMLElement | null>(null);
let focusExportsAfterEnter = false;
function scrollToExports() {
  const target =
    exportsPanel.value?.querySelector<HTMLElement>(".office-quick-export") ||
    exportsPanel.value;
  void buildWorkspace.value?.showOutputTools();
  target?.scrollIntoView({ behavior: "instant", block: "center" });
}
function focusExportActions() {
  void nextTick(() => {
    scrollToExports();
    // Build previews load after the view enters and can move the export panel.
    for (const delay of [300, 1000])
      window.setTimeout(() => {
        if (activeView.value === "build") scrollToExports();
      }, delay);
  });
}
function openExports() {
  if (!comparisonExport.value) {
    showView("build");
    return;
  }
  if (activeView.value === "build") {
    focusExportActions();
    return;
  }
  focusExportsAfterEnter = true;
  showView("build");
}
let pendingFindingFocusId: string | null = null;
function restoreFindingFocus() {
  if (!pendingFindingFocusId || activeView.value !== 'validation') return;
  const entry = document.getElementById(pendingFindingFocusId);
  if (entry) { entry.focus(); pendingFindingFocusId = null; }
  else {
    const runChecks = document.querySelector<HTMLButtonElement>('.preflight-workspace .build-heading button');
    if (runChecks) { runChecks.focus(); pendingFindingFocusId = null; notify('The original finding is no longer in the current results. Review the updated checks.', 'info'); }
  }
}
function afterWorkspaceSectionEnter() {
  fitDocumentsWorkspace();
  restoreDocumentSearchFocus();
  restoreFindingFocus();
  if (focusExportsAfterEnter && activeView.value === "build") {
    focusExportsAfterEnter = false;
    focusExportActions();
  }
}
watch(exportsList, (items) => {
  if (!items.some((item) => item.id === comparisonExportId.value))
    comparisonExportId.value = items[0]?.id || "";
});
watch(
  () => exportsList.value[0]?.id,
  (id, previous) => {
    if (id && id !== previous) comparisonExportId.value = id;
  },
);
const interruptedJobs = computed(() =>
  recentJobs.value.filter((j) => j.status === "interrupted" && !j.retry_job_id),
);
const errorChecks = computed(() =>
  checks.value.filter((c) => c.severity === "error"),
);
const warningChecks = computed(() =>
  checks.value.filter((c) => c.severity === "warning"),
);
const signatureRiskDocs = computed(() => signatureRiskDocuments(includedDocuments.value));
const totalPages = computed(() =>
  docs.value.reduce((n, d) => n + d.page_count, 0),
);
const documentKindOptions: DocKind[] = [
  "petition",
  "affidavit",
  "order",
  "annexure",
  "application",
  "evidence",
  "other",
];
const documentDirty = computed(
  () =>
    !!selectedDoc.value &&
    (docEditor.title !== selectedDoc.value.title ||
      docEditor.kind !== selectedDoc.value.kind ||
      docEditor.part !== selectedDoc.value.part ||
      docEditor.label !== selectedDoc.value.label ||
      docEditor.group_key !== selectedDoc.value.group_key ||
      docEditor.visible_signature_status !==
        selectedDoc.value.visible_signature_status ||
      docEditor.document_date !== selectedDoc.value.document_date ||
      docEditor.date_confirmed !==
        (selectedDoc.value.date_status === "confirmed")),
);
const settingsDirty = computed(
  () =>
    !!currentMatter.value &&
    (matterEditor.max_pages_per_volume !==
      currentMatter.value.max_pages_per_volume ||
      matterEditor.keep_groups_intact !==
        currentMatter.value.keep_groups_intact),
);
function queueDocumentAutosave() {
  if (documentAutosaveTimer) clearTimeout(documentAutosaveTimer);
  const id = selectedDocId.value;
  if (engineBlocked.value || autosaveHeld.value || !documentDirty.value || !id || !docEditor.title.trim()) return;
  if (documentAutosaveState.value === "failed")
    documentAutosaveState.value = "idle";
  documentAutosaveTimer = setTimeout(() => {
    documentAutosaveTimer = null;
    if (engineBlocked.value || autosaveHeld.value || selectedDocId.value !== id || !documentDirty.value) return;
    if (busy.value || tabDiscardRequest.value) {
      queueDocumentAutosave();
      return;
    }
    void saveDocument(true);
  }, 1200);
}
watch(
  () => [
    selectedDocId.value,
    docEditor.title,
    docEditor.kind,
    docEditor.part,
    docEditor.label,
    docEditor.group_key,
    docEditor.document_date,
    docEditor.date_confirmed,
    docEditor.visible_signature_status,
    documentDirty.value,
  ],
  queueDocumentAutosave,
);
function queueSettingsAutosave() {
  if (settingsAutosaveTimer) clearTimeout(settingsAutosaveTimer);
  const id = currentMatterId.value;
  if (engineBlocked.value || autosaveHeld.value || !settingsDirty.value || !id) return;
  if (settingsAutosaveState.value === "failed")
    settingsAutosaveState.value = "idle";
  settingsAutosaveTimer = setTimeout(() => {
    settingsAutosaveTimer = null;
    if (engineBlocked.value || autosaveHeld.value || currentMatterId.value !== id || !settingsDirty.value) return;
    if (busy.value || isJobActive.value || tabDiscardRequest.value) {
      queueSettingsAutosave();
      return;
    }
    void savePolicy(true);
  }, 1500);
}
watch(
  () => [
    currentMatterId.value,
    matterEditor.max_pages_per_volume,
    matterEditor.keep_groups_intact,
    settingsDirty.value,
  ],
  queueSettingsAutosave,
);

function resetDrafts() {
  documentTablePanel.value?.discard();
  const doc = selectedDoc.value;
  docEditor.title = doc?.title || "";
  docEditor.kind = doc?.kind || "other";
  docEditor.part = doc?.part || 1;
  docEditor.label = doc?.label || "";
  docEditor.group_key = doc?.group_key || "";
  docEditor.document_date = doc?.document_date || "";
  docEditor.date_confirmed = doc?.date_status === "confirmed";
  docEditor.visible_signature_status =
    doc?.visible_signature_status || "unreviewed";
  const matter = currentMatter.value;
  matterEditor.max_pages_per_volume = matter?.max_pages_per_volume || 300;
  matterEditor.keep_groups_intact = matter?.keep_groups_intact ?? true;
}
function confirmDiscardDrafts() {
  if (!tableDirty.value && !documentDirty.value && !settingsDirty.value && !formattingDirty.value)
    return true;
  if (
    !window.confirm(
      "Discard unsaved document details, bundle settings or output policy?",
    )
  )
    return false;
  resetDrafts();
  outputPresetsPanel.value?.discard();
  formattingDirty.value = false;
  return true;
}
function captureActiveTab() {
  const tab = activeTab.value;
  if (!tab || tab.matterId !== currentMatterId.value) return;
  tab.view = activeView.value;
  tab.selectedDocumentId = selectedDocId.value;
  tab.detailsOpen = detailsOpen.value;
  if (selectedDocId.value) tab.documentDraft = { ...docEditor };
  tab.settingsDraft = { ...matterEditor };
  tab.hadUnsavedChanges =
    documentDirty.value || settingsDirty.value || formattingDirty.value;
}
function restoreTabDraft(tab: WorkspaceTab) {
  if (activeTabId.value !== tab.id || currentMatterId.value !== tab.matterId)
    return;
  if (tab.documentDraft && tab.selectedDocumentId === selectedDocId.value)
    Object.assign(docEditor, tab.documentDraft);
  if (tab.settingsDraft) Object.assign(matterEditor, tab.settingsDraft);
  detailsOpen.value = !!tab.detailsOpen;
}
function tabHasDraft(tab: WorkspaceTab) {
  if (tab.id === activeTabId.value) captureActiveTab();
  if (tab.id !== activeTabId.value) return !!tab.hadUnsavedChanges;
  const matter = matterList.value.find((item) => item.id === tab.matterId);
  const doc = docs.value.find(
    (item) =>
      item.id === tab.selectedDocumentId &&
      currentMatterId.value === tab.matterId,
  );
  const settingsDirtyInTab =
    !!matter &&
    !!tab.settingsDraft &&
    (tab.settingsDraft.max_pages_per_volume !== matter.max_pages_per_volume ||
      tab.settingsDraft.keep_groups_intact !== matter.keep_groups_intact);
  const documentDirtyInTab =
    !!doc &&
    !!tab.documentDraft &&
    (tab.documentDraft.title !== doc.title ||
      tab.documentDraft.kind !== doc.kind ||
      tab.documentDraft.part !== doc.part ||
      tab.documentDraft.label !== doc.label ||
      tab.documentDraft.group_key !== doc.group_key ||
      tab.documentDraft.document_date !== doc.document_date ||
      tab.documentDraft.date_confirmed !== (doc.date_status === "confirmed") ||
      tab.documentDraft.visible_signature_status !==
        doc.visible_signature_status);
  return (
    settingsDirtyInTab ||
    documentDirtyInTab ||
    (tab.id === activeTabId.value && formattingDirty.value)
  );
}
let tabActivationSequence = 0;
async function activateTab(tab: WorkspaceTab) {
  if (tableDirty.value && !(await guardArtifactTransition("the selected tab"))) return;
  if (!(await leaveMembership())) return;
  if (
    activeTabId.value === tab.id &&
    activeView.value === tab.view &&
    currentMatterId.value === tab.matterId
  )
    return;
  const pendingPdfAction =
    activeTabId.value !== tab.id && !!pdfViewer.value?.hasPendingDraft();
  if (
    (pendingPdfAction || formattingDirty.value) &&
    !(await askTabDiscard(
      "Discard the unfinished PDF tool action or output policy before switching tabs?",
    ))
  )
    return;
  if (pendingPdfAction) pdfViewer.value?.discardPendingDraft();
  if (formattingDirty.value) {
    outputPresetsPanel.value?.discard();
    formattingDirty.value = false;
  }
  const activation = ++tabActivationSequence;
  if (activeTabId.value !== tab.id) captureActiveTab();
  activeTabId.value = tab.id;
  pageToolsOpen.value = false;
  documentTableOpen.value = false;
  readerFocus.value = true;
  const matterChanged = currentMatterId.value !== tab.matterId;
  currentMatterId.value = tab.matterId;
  selectedDocId.value =
    tab.documentId ||
    tab.selectedDocumentId ||
    (matterChanged ? null : docs.value[0]?.id || null);
  activeView.value = tab.view;
  mobileNavOpen.value = false;
  if (matterChanged) await loadMatterResources();
  await nextTick();
  if (activation !== tabActivationSequence || activeTabId.value !== tab.id || currentMatterId.value !== tab.matterId) return;
  resetDrafts();
  restoreTabDraft(tab);
}
async function openMatterTab(matterId: string, view: View = "overview") {
  if (tableDirty.value && !(await guardArtifactTransition(view))) return;
  if (view!=="documents") {documentTableOpen.value=false;tableDirty.value=false;}
  if (!(await leaveMembership())) return;
  const id = `matter:${matterId}`;
  let tab = workspaceTabs.value.find((item) => item.id === id);
  const created = !tab;
  const previousView = tab?.view;
  if (!tab) {
    tab = { id, matterId, documentId: null, view, selectedDocumentId: null };
    workspaceTabs.value.push(tab);
  } else if (activeTabId.value === id && activeView.value !== view) {
    const pendingPdfAction = !!pdfViewer.value?.hasPendingDraft();
    if (
      (pendingPdfAction || formattingDirty.value) &&
      !(await askTabDiscard(
        "Discard the unfinished PDF tool action or output policy before changing views?",
      ))
    )
      return;
    if (pendingPdfAction) pdfViewer.value?.discardPendingDraft();
    if (formattingDirty.value) {
      outputPresetsPanel.value?.discard();
      formattingDirty.value = false;
    }
    tab.view = view;
    activeView.value = view;
    return;
  } else tab.view = view;
  await activateTab(tab);
  if (activeTabId.value !== tab.id) {
    if (created)
      workspaceTabs.value = workspaceTabs.value.filter(
        (item) => item.id !== tab.id,
      );
    else if (previousView) tab.view = previousView;
  }
}
async function openDocumentTab(documentId: string, page?: number, revision?: number) {
  const matterId = currentMatterId.value;
  if (!matterId) return;
  const id = `document:${matterId}:${documentId}`;
  let tab = workspaceTabs.value.find((item) => item.id === id);
  const created = !tab;
  if (!tab) {
    tab = {
      id,
      matterId,
      documentId,
      view: "documents",
      selectedDocumentId: documentId,
      title:
        docs.value.find((doc) => doc.id === documentId)?.title || "Document",
    };
    workspaceTabs.value.push(tab);
  }
  tab.view = "documents";
  await activateTab(tab);
  if (created && activeTabId.value !== tab.id)
    workspaceTabs.value = workspaceTabs.value.filter(
      (item) => item.id !== tab.id,
    );
  if (activeTabId.value === tab.id) { preparationMode.value = false; readerFocus.value = true; workspaceMenuOpen.value = false; tabsMenuOpen.value = false; }
  if (activeTabId.value === tab.id && page !== undefined)
    pageTarget.value = {
      documentId,
      page,
      revision,
      sequence: pageTarget.value.sequence + 1,
    };
}
async function closeWorkspaceTab(tab: WorkspaceTab) {
  if (tableDirty.value && !(await guardArtifactTransition("closing this tab"))) return;
  if (!(await leaveMembership())) return;
  const closing = tab.documentId
    ? [tab]
    : workspaceTabs.value.filter((item) => item.matterId === tab.matterId);
  const closesActive = closing.some(item => item.id === activeTabId.value);
  const pendingPdfAction = closesActive && !!pdfViewer.value?.hasPendingDraft();
  if (
    (pendingPdfAction || closing.some(tabHasDraft)) &&
    !(await askTabDiscard("Save or discard unsaved changes in the closing tab(s)?", closesActive, !closing.some(item => item.id !== activeTabId.value && item.hadUnsavedChanges)))
  )
    return;
  if (pendingPdfAction) pdfViewer.value?.discardPendingDraft();
  const closingIds = new Set(closing.map((item) => item.id));
  const wasActive = !!activeTabId.value && closingIds.has(activeTabId.value);
  workspaceTabs.value = workspaceTabs.value.filter(
    (item) => !closingIds.has(item.id),
  );
  if (!wasActive) return;
  formattingDirty.value = false;
  activeTabId.value = null;
  const next = workspaceTabs.value[workspaceTabs.value.length - 1];
  if (next) await activateTab(next);
  else {
    activeView.value = "matters";
    selectedDocId.value = null;
    readerFocus.value = false;
  }
}
function selectDocument(id: string) {
  void openDocumentTab(id);
}
function openPreviewDocument(id: string) {
  void openDocumentTab(id);
}
async function applyIncludedDocumentSort(ordered:MatterDocument[]) {
  let index=0;
  await applyDocumentSort(docs.value.map(d=>d.bundle_included?ordered[index++]!:d));
}
async function moveIncludedDocument(id:string,direction:number) {
  const ordered=[...includedDocuments.value], index=ordered.findIndex(d=>d.id===id), target=index+direction;
  if(index<0||target<0||target>=ordered.length)return;
  [ordered[index],ordered[target]]=[ordered[target]!,ordered[index]!];
  await applyIncludedDocumentSort(ordered);
}
async function prepareIndexBookmark(id: string) {
  if (outputPresetsPanel.value && "showBookmarks" in outputPresetsPanel.value) outputPresetsPanel.value.showBookmarks();
  await nextTick();
  bookmarkPreset.value = {
    documentId: id,
    sequence: bookmarkPreset.value.sequence + 1,
  };
}
watch([activeView, currentMatterId], () => { workspaceMenuOpen.value = false; tabsMenuOpen.value = false; });
watch(selectedDocId, (id) => {
  pageToolsOpen.value = false;
  if (!id) readerFocus.value = false;
});
function fitDocumentsWorkspace() {
  const workspace = documentsWorkspace.value;
  if (!workspace) {
    documentsWorkspaceHeight.value = null;
    return;
  }
  const scale = uiScale.value / 100;
  // Retain all three panels when there is room for a roughly 500px reader.
  compactDocuments.value = workspace.getBoundingClientRect().width / scale < 1150;
  if (window.innerWidth <= 700 && !editorExpanded.value) {
    documentsWorkspaceHeight.value = null;
    return;
  }
  const minimum = window.innerWidth <= 1120 ? 240 : 320;
  documentsWorkspaceHeight.value = Math.max(
    minimum,
    Math.floor(
      (window.innerHeight - workspace.getBoundingClientRect().top) / scale - 28,
    ),
  );
}
watch([uiScale, activeView, currentMatterId, editorExpanded, documentTableOpen, preflightContext], async () => {
  await nextTick();
  onResize();
  window.requestAnimationFrame(fitDocumentsWorkspace);
});
watch(pageToolsOpen, async (open) => {
  await nextTick();
  const dialog = pageToolsDialog.value;
  if (open && dialog && !dialog.open) dialog.showModal();
  if (!open && dialog?.open) dialog.close();
});
async function showView(view: View) {
  if (!(await leaveMembership())) return;
  workspaceMenuOpen.value = false;
  if (activeView.value === view) {
    mobileNavOpen.value = false;
    return;
  }
  if (
    view !== "rules" &&
    view !== "about" &&
    view !== "matters" &&
    currentMatterId.value
  ) {
    await openMatterTab(currentMatterId.value, view);
    if (view === "validation") void validate();
    return;
  }
  if (!(await guardArtifactTransition(view))) return;
  captureActiveTab();
  activeView.value = view;
  mobileNavOpen.value = false;
  if (view === "validation") void validate();
}
function toggleNavigation() {
  workspaceMenuOpen.value = !workspaceMenuOpen.value;
  void nextTick(() => window.requestAnimationFrame(fitDocumentsWorkspace));
}
function onResize() {
  const narrow = window.matchMedia(activeView.value === "documents" ? "(max-width: 1120px)" : "(max-width: 900px)").matches;
  if (smallScreen.value !== narrow) mobileNavOpen.value = false;
  smallScreen.value = narrow;
  fitDocumentsWorkspace();
}
function beforeUnload(event: BeforeUnloadEvent) {
  if (
    !membershipDirty.value &&
    !tableDirty.value &&
    !documentDirty.value &&
    !settingsDirty.value &&
    !formattingDirty.value &&
    !workspaceTabs.value.some((tab) => tab.hadUnsavedChanges) &&
    !pdfViewer.value?.hasPendingDraft()
  )
    return;
  event.preventDefault();
  event.returnValue = "";
}
function onKeydown(event: KeyboardEvent) {
  if (documentTableOpen.value && !tabDiscardRequest.value && (event.ctrlKey || event.metaKey) && event.key.toLowerCase()==='s') {event.preventDefault();void documentTablePanel.value?.save();return;}

  if (event.key === "Escape" && tabDiscardRequest.value) {
    answerTabDiscard(false);
    return;
  }
  if (tabDiscardRequest.value) {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') { event.preventDefault(); void saveBeforeNavigation(); }
    return;
  }
  if (membershipOpen.value) {
    if (event.key === 'Escape') { event.preventDefault(); void leaveMembership(); return; }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'z') { event.preventDefault(); return; }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') { event.preventDefault(); void membershipPanel.value?.apply(); return; }
    if (event.key === '/' || ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k')) { event.preventDefault(); void openPreparationSearch(); return; }
  }
  if (event.key === "Escape" && reviewingPdf.value) {
    reviewingPdf.value = null;
    return;
  }
  if (event.key === "Escape" && mobileNavOpen.value) {
    mobileNavOpen.value = false;
    return;
  }
  if (event.defaultPrevented || modalOpen.value || event.altKey) return;
  const target = event.target as HTMLElement | null;
  const editingField =
    target?.isContentEditable ||
    !!target?.closest('input, textarea, select, [contenteditable="true"]');
  if (
    event.key === "Escape" &&
    editorExpanded.value &&
    !pageToolsOpen.value &&
    !editingField
  ) {
    toggleEditorExpanded();
    return;
  }
  if (event.key === "Escape" && toast.value) {
    toast.value = null;
    return;
  }
  if (editingField) return;
  if (
    event.key === "/" ||
    ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k")
  ) {
    event.preventDefault();
    void openDocumentSearch();
  }
  if (
    (event.ctrlKey || event.metaKey) &&
    event.key.toLowerCase() === "z" &&
    currentMatter.value &&
    !busy.value
  ) {
    event.preventDefault();
    if (event.shiftKey && history.value.can_redo) void traverseHistory("redo");
    else if (!event.shiftKey && history.value.can_undo)
      void traverseHistory("undo");
  }
}
watch([theme, uiScale], () => applyAppearance(theme.value, uiScale.value), {
  immediate: true,
});

watch(
  selectedDoc,
  (doc, previous) => {
    if (
      doc &&
      previous?.id === doc.id &&
      (docEditor.title !== previous.title ||
        docEditor.kind !== previous.kind ||
        docEditor.part !== previous.part ||
        docEditor.label !== previous.label ||
        docEditor.group_key !== previous.group_key ||
        docEditor.document_date !== previous.document_date ||
        docEditor.date_confirmed !== (previous.date_status === "confirmed") ||
        docEditor.visible_signature_status !==
          previous.visible_signature_status)
    )
      return;
    docEditor.title = doc?.title || "";
    docEditor.kind = doc?.kind || "other";
    docEditor.part = doc?.part || 1;
    docEditor.label = doc?.label || "";
    docEditor.group_key = doc?.group_key || "";
    docEditor.document_date = doc?.document_date || "";
    docEditor.date_confirmed = doc?.date_status === "confirmed";
    docEditor.visible_signature_status =
      doc?.visible_signature_status || "unreviewed";
  },
  { immediate: true },
);
watch(
  currentMatter,
  (item, previous) => {
    if (
      item &&
      previous?.id === item.id &&
      (matterEditor.max_pages_per_volume !== previous.max_pages_per_volume ||
        matterEditor.keep_groups_intact !== previous.keep_groups_intact)
    )
      return;
    matterEditor.max_pages_per_volume = item?.max_pages_per_volume || 300;
    matterEditor.keep_groups_intact = item?.keep_groups_intact ?? true;
  },
  { immediate: true },
);
watch(currentMatterId, () => {
  detailsOpen.value = false;
  documentTableOpen.value = false;
  pageQuery.value = "";
  pageSearch.value = null;
  searchSequence++;
  if (currentMatterId.value) {
    void loadMatterResources();
  }
}, {flush: "sync"});
watch(pageQuery, () => {
  if (searchTimer) clearTimeout(searchTimer);
  if (pageQuery.value.trim().length < 2) {
    pageSearch.value = null;
    pageSearchError.value = "";
    pageSearchBusy.value = false;
    searchSequence++;
    return;
  }
  searchTimer = setTimeout(() => void searchPages(), 350);
});

async function searchPages() {
  const id = currentMatterId.value;
  const q = pageQuery.value.trim();
  const sequence = ++searchSequence;
  if (!id || q.length < 2) return;
  pageSearchBusy.value = true;
  pageSearchError.value = "";
  try {
    const result = await api<PageSearchResult>(
      `/matters/${id}/search?q=${encodeURIComponent(q)}`,
      "GET",
      undefined,
      5 * 60_000,
    );
    if (sequence === searchSequence && id === currentMatterId.value)
      pageSearch.value = result;
  } catch (err) {
    if (sequence === searchSequence) pageSearchError.value = friendlyError(err);
  } finally {
    if (sequence === searchSequence) pageSearchBusy.value = false;
  }
}
function openSearchMatch(documentId: string, page: number) {
  void openDocumentTab(documentId, page);
}
function highlightedSnippet(snippet: string, query: string) {
  const index = snippet.toLocaleLowerCase().indexOf(query.toLocaleLowerCase());
  return index < 0
    ? { before: snippet, match: "", after: "" }
    : {
        before: snippet.slice(0, index),
        match: snippet.slice(index, index + query.length),
        after: snippet.slice(index + query.length),
      };
}

function notify(message: string, kind: "success" | "error" | "info" = "info") {
  toast.value = { message, kind };
  if (toastTimer) clearTimeout(toastTimer);
  toastTimer = setTimeout(
    () => (toast.value = null),
    kind === "error" && message.length > 140 ? 12000 : 6500,
  );
}
function issue(err: unknown) {
  notify(friendlyError(err), "error");
}

async function openOfficialReference(url: string) {
  const parsed = new URL(url);
  const allowedHosts = new Set([
    "delhihighcourt.nic.in",
    "www.delhihighcourt.nic.in",
    "dhcefiling.nic.in",
  ]);
  if (parsed.protocol !== "https:" || !allowedHosts.has(parsed.hostname)) {
    notify("Blocked an unexpected external reference URL.", "error");
    return;
  }
  try {
    if (inTauri()) await openUrl(parsed.href);
    else window.open(parsed.href, "_blank", "noopener,noreferrer");
  } catch (err) {
    issue(err);
  }
}
async function openReviewedExternalLink(url: string) {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    notify("This reviewed external link is invalid.", "error");
    return;
  }
  if (!["https:", "http:", "mailto:"].includes(parsed.protocol)) {
    notify("Blocked a reviewed link with an unsafe URL scheme.", "error");
    return;
  }
  try {
    if (inTauri()) await openUrl(parsed.href);
    else window.open(parsed.href, "_blank", "noopener,noreferrer");
  } catch (err) {
    issue(err);
  }
}

async function init() {
  if (engineBlocked.value && currentMatterId.value) { await retryEngine(); return; }
  if (connecting) return;
  const recoveringStartup = engineBlocked.value;
  connecting = true;
  apiStatus.value = "connecting";
  try {
    let connected = false;
    const deadline = Date.now() + (inTauri() ? 60_000 : 12_000);
    while (Date.now() < deadline) {
      try {
        await api("/health", "GET", undefined, 1_500);
        connected = true;
        break;
      } catch {
        await new Promise((resolve) => setTimeout(resolve, 400));
      }
    }
    if (!connected)
      throw new Error(
        inTauri()
          ? "Local engine did not start. Quit and reopen CourtBundle. If this continues, check the packaged sidecar and local port."
          : "Local engine did not start. Run backend/dev.sh and retry.",
      );
    const vault = await api<{
      state: "setup" | "locked" | "unlocked";
      enabled: boolean;
    }>("/vault/status");
    vaultEnabled.value = vault.enabled;
    if (vault.state !== "unlocked") {
      vaultState.value = vault.state;
      apiStatus.value = "online";
      return;
    }
    vaultState.value = "unlocked";
    profileList.value = await api<CourtProfile[]>("/profiles");
    await refreshMatters();
    apiStatus.value = "online";
    engineState.value = "ready";
    if (recoveringStartup) { await loadMatterResources(); await resumeAutosaveWhenClean(); }
  } catch (err) {
    apiStatus.value = "offline";
    disconnectEngine();
    issue(err);
  } finally {
    connecting = false;
  }
}
async function setupVault() {
  if (vaultBusy.value) return;
  if (vaultCredential.value !== vaultConfirm.value) {
    vaultError.value = "Passwords do not match.";
    return;
  }
  vaultBusy.value = true;
  vaultError.value = "";
  try {
    const result = await api<{ recovery_key: string }>(
      "/vault/setup",
      "POST",
      { password: vaultCredential.value },
      5 * 60_000,
    );
    vaultCredential.value = "";
    vaultConfirm.value = "";
    vaultRecoveryKey.value = result.recovery_key;
    vaultState.value = "show_recovery";
  } catch (err) {
    vaultError.value = friendlyError(err);
  } finally {
    vaultBusy.value = false;
  }
}
async function unlockVault() {
  if (vaultBusy.value) return;
  vaultBusy.value = true;
  vaultError.value = "";
  try {
    await api(
      "/vault/unlock",
      "POST",
      { credential: vaultCredential.value, recovery: vaultRecoveryMode.value },
      5 * 60_000,
    );
    vaultCredential.value = "";
    vaultState.value = "unlocked";
    await init();
  } catch (err) {
    vaultError.value = friendlyError(err);
  } finally {
    vaultBusy.value = false;
  }
}
async function rotateVault() {
  if (vaultBusy.value) return;
  if (vaultCredential.value !== vaultConfirm.value) {
    vaultError.value = "Passwords do not match.";
    return;
  }
  vaultBusy.value = true;
  vaultError.value = "";
  try {
    const result = await api<{ recovery_key: string }>(
      "/vault/rotate",
      "POST",
      { new_password: vaultCredential.value },
    );
    vaultCredential.value = "";
    vaultConfirm.value = "";
    vaultRecoveryKey.value = result.recovery_key;
    vaultState.value = "show_recovery";
  } catch (err) {
    vaultError.value = friendlyError(err);
  } finally {
    vaultBusy.value = false;
  }
}
async function finishVaultSetup() {
  vaultRecoveryKey.value = "";
  vaultState.value = "unlocked";
  await init();
}
async function checkHealth() {
  if (connecting || apiStatus.value === "offline") return;
  try {
    await api("/health", "GET", undefined, 1_500);
  } catch {
    if (apiStatus.value === "online") {
      disconnectEngine();
      apiStatus.value = "offline";
      notify(
        inTauri()
          ? "Local engine stopped. Quit and reopen CourtBundle to start a new session."
          : "Local engine stopped. Restart backend/dev.sh and retry.",
        "error",
      );
    }
  }
}
async function refreshMatters() {
  const [all, archived] = await Promise.all([
    api<Matter[]>("/matters"),
    api<Matter[]>("/matters/archived"),
  ]);
  matterList.value = all;
  archivedMatters.value = archived;
  if (currentMatterId.value && !all.some((m) => m.id === currentMatterId.value))
    currentMatterId.value = null;
  if (!currentMatterId.value && all.length) currentMatterId.value = all[0].id;
  const available = new Set(all.map((item) => item.id));
  workspaceTabs.value = workspaceTabs.value.filter((tab) =>
    available.has(tab.matterId),
  );
  if (
    activeTabId.value &&
    !workspaceTabs.value.some((tab) => tab.id === activeTabId.value)
  )
    activeTabId.value = null;
  if (
    !activeTabId.value &&
    currentMatterId.value &&
    activeView.value !== "matters"
  ) {
    const id = `matter:${currentMatterId.value}`;
    if (!workspaceTabs.value.some((tab) => tab.id === id))
      workspaceTabs.value.push({
        id,
        matterId: currentMatterId.value,
        documentId: null,
        view: activeView.value,
        selectedDocumentId: selectedDocId.value,
      });
    activeTabId.value = id;
  }
}
let resourceSequence = 0;
let validationSequence = 0;
const checksLoading = ref(false);
const checksError = ref("");
let recoverySnapshot = '';
let recoveryInputSnapshot: string | null | undefined;
function savedContext() { return JSON.stringify({matter: currentMatter.value, documents: docs.value}); }
watch(engineState, (state, previous) => {
  if (state === 'disconnected') {
    if (previous === 'ready') {
      recoverySnapshot = savedContext();
      recoveryInputSnapshot = checks.value.find(check => check.input_snapshot)?.input_snapshot;
    }
    apiStatus.value = 'offline';
    ++resourceSequence; ++validationSequence;
    checks.value = []; checksLoading.value = false;
    checksError.value = 'Local engine unavailable. Run checks again after recovery.';
    if (documentAutosaveTimer) clearTimeout(documentAutosaveTimer);
    if (settingsAutosaveTimer) clearTimeout(settingsAutosaveTimer);
  }
  void nextTick(fitDocumentsWorkspace);
}, {flush: 'sync'});
async function retryEngine() {
  if (engineState.value === 'checking') return;
  if (!currentMatterId.value) { await init(); return; }
  engineState.value = 'checking';
  const id = currentMatterId.value;
  try {
    await api('/health', 'GET', undefined, 1500);
    const [matters, documents, findings] = await Promise.all([
      api<Matter[]>('/matters'), api<MatterDocument[]>(`/matters/${id}/documents`),
      api<PreflightCheck[]>(`/matters/${id}/validate`),
    ]);
    if (id !== currentMatterId.value) throw new Error('Workspace changed during recovery. Retry for this workspace.');
    const fresh = JSON.stringify({matter: matters.find(item => item.id === id), documents});
    const fingerprint = findings.find(check => check.input_snapshot)?.input_snapshot;
    const changed = fresh !== recoverySnapshot || !!(recoveryInputSnapshot && fingerprint !== recoveryInputSnapshot);
    if (changed || workspaceTabs.value.some(tab => tab.id !== activeTabId.value && tab.hadUnsavedChanges)) {
      engineReviewReason.value = changed ? 'Saved inputs changed. Review the latest state before editing.' : 'Retained drafts in other tabs need review before saving.';
      engineState.value = 'changed';
      return;
    }
    // Preserve mounted readers, query, positions and drafts. Never replay saves/jobs.
    checks.value = findings; checksError.value = '';
    engineState.value = 'ready'; apiStatus.value = 'online';
    previewRefreshKey.value++;
    await nextTick();
    (document.querySelector<HTMLElement>('.export-review[open] [aria-controls="artifact-navigation"]') || document.querySelector<HTMLElement>('.viewer-root'))?.focus({preventScroll:true});
    notify('Local engine reconnected. Retained drafts require an explicit Save; no operations were replayed.', 'info');
  } catch (err) { disconnectEngine(); issue(err); }
}
async function reviewRecoveredState() {
  // The existing focus-contained guard keeps drafts until explicit discard.
  if (!(await askTabDiscard(`${engineReviewReason.value} Discard retained drafts in all open tabs and load the latest state? Existing Preflight targets remain tied to their old revision.`, false, false))) return;
  membershipPanel.value?.discard();
  membershipDirty.value = false; membershipOpen.value = false;
  pdfViewer.value?.discardPendingDraft(); outputPresetsPanel.value?.discard();
  workspaceTabs.value.forEach(tab => { delete tab.documentDraft; delete tab.settingsDraft; tab.hadUnsavedChanges = false; });
  formattingDirty.value = false;
  try {
    await api('/health', 'GET', undefined, 1500);
    // Stay blocked until all required current resources have been read successfully.
    const id = currentMatterId.value;
    const [matters, documents] = await Promise.all([api<Matter[]>('/matters'), api<MatterDocument[]>(`/matters/${id}/documents`), api<PreflightCheck[]>(`/matters/${id}/validate`)]);
    if (!matters.some(item => item.id === id)) throw new Error('This matter is no longer available. Retained reading context remains open.');
    matterList.value = matters; docs.value = documents;
    resetDrafts();
    engineState.value = 'ready'; apiStatus.value = 'online';
    await loadMatterResources();
    await resumeAutosaveWhenClean();
    await nextTick(); pdfViewer.value && document.querySelector<HTMLElement>('.viewer-root')?.focus();
  } catch (err) { disconnectEngine(); issue(err); }
}
async function loadMatterResources() {
  if (engineBlocked.value || !currentMatterId.value) return;
  const id = currentMatterId.value;
  const call = ++resourceSequence;
  const validationCall = ++validationSequence;
  checksLoading.value = true; checksError.value = ""; checks.value = [];
  try {
    const [d, c, e, j, h, archived, usage, bookmarks] = await Promise.all([
      api<MatterDocument[]>(`/matters/${id}/documents`),
      api<PreflightCheck[]>(`/matters/${id}/validate`),
      api<ExportManifest[]>(`/matters/${id}/exports`),
      api<Job[]>(`/matters/${id}/jobs`),
      api<typeof history.value>(`/matters/${id}/history`),
      api<MatterDocument[]>(`/matters/${id}/archived-documents`),
      api<typeof storageUsage.value>(`/matters/${id}/storage`),
      api<ManualBookmark[]>(`/matters/${id}/bookmarks`),
    ]);
    if (engineBlocked.value || currentMatterId.value !== id || call !== resourceSequence) return;
    docs.value = d;
    if (validationCall === validationSequence) checks.value = c;
    exportsList.value = e;
    recentJobs.value = j;
    history.value = h;
    manualBookmarks.value = bookmarks;
    previewRefreshKey.value += 1;
    if (pageQuery.value.trim().length >= 2) void searchPages();
    archivedDocs.value = archived;
    storageUsage.value = usage;
    const ongoing = j.find(
      (job) => job.status === "queued" || job.status === "running",
    );
    if (ongoing && (!activeJob.value || activeJob.value.id !== ongoing.id))
      watchJob(ongoing);
    if (!ongoing) { activeJob.value = j.find(item=>item.id===activeJob.value?.id) || j[0] || null; cancelPending.value=false; }
    if (
      !selectedDocId.value ||
      !d.some((doc) => doc.id === selectedDocId.value)
    )
      selectedDocId.value = d[0]?.id || null;
  } catch (err) {
    if (call === resourceSequence) { checksError.value = "Checks unavailable; retry before relying on them."; issue(err); }
  } finally { if (validationCall === validationSequence) checksLoading.value = false; }
}
async function refreshHistory() {
  if (currentMatterId.value)
    history.value = await api<typeof history.value>(
      `/matters/${currentMatterId.value}/history`,
    );
}
async function traverseHistory(direction: "undo" | "redo") {
  if (membershipOpen.value) return;
  if (!currentMatterId.value || busy.value || !confirmDiscardDrafts()) return;
  if (filingGuide.value && !(await filingGuide.value.flushPendingSave())) {
    notify("Save the Memo of Parties before using Undo or Redo.", "error");
    return;
  }
  busy.value = true;
  try {
    await api(`/matters/${currentMatterId.value}/history/${direction}`, "POST");
    await refreshMatters();
    await loadMatterResources();
    guideReloadKey.value += 1;
    notify(
      direction === "undo" ? "Last edit undone." : "Edit restored.",
      "success",
    );
  } catch (err) {
    issue(err);
  } finally {
    busy.value = false;
  }
}
async function createMatter(fields: MatterNew) {
  creating.value = true;
  try {
    const value = await api<Matter>("/matters", "POST", fields);
    await refreshMatters();
    await openMatterTab(value.id, "documents");
    modalOpen.value = false;
    notify("Matter created. Import your first document to begin.", "success");
  } catch (err) {
    issue(err);
  } finally {
    creating.value = false;
  }
}
async function createSampleMatter() {
  if (creating.value) return;
  creating.value = true;
  try {
    const value = await api<Matter>("/matters", "POST", {
      name: `Synthetic Sample Matter ${new Date().toLocaleDateString("en-CA")}`,
      court_profile: "general-demo",
      case_type: "Synthetic demonstration",
      petitioner: "Sample Applicant",
      respondent: "Sample Respondent",
      advocate: "",
      case_number: "",
      max_pages_per_volume: 10,
      keep_groups_intact: true,
    });
    await api(`/matters/${value.id}/cover`, "POST");
    await refreshMatters();
    await openMatterTab(value.id, "documents");
    notify(
      "Opened a separate synthetic sample matter. Its draft cover is not a court-approved form.",
      "success",
    );
  } catch (err) {
    issue(err);
  } finally {
    creating.value = false;
  }
}
async function backupCurrentMatter() {
  if (!currentMatterId.value || busy.value || !confirmDiscardDrafts()) return;
  busy.value = true;
  try {
    if (await downloadMatter(currentMatterId.value))
      notify("Portable matter backup saved.", "success");
  } catch (err) {
    issue(err);
  } finally {
    busy.value = false;
  }
}
async function importMatterPackage(files: FileList | null) {
  const file = files?.item(0);
  if (!file || busy.value || !confirmDiscardDrafts()) return;
  busy.value = true;
  try {
    const restored = await restoreMatter(file);
    await useRestoredMatter(restored);
  } catch (err) {
    if (
      err instanceof ApiError &&
      err.status === 422 &&
      err.message.includes("backup needs")
    ) {
      protectedRestoreFile.value = file;
      protectedRestoreError.value = "";
    } else issue(err);
  } finally {
    busy.value = false;
    if (matterFileInput.value) matterFileInput.value.value = "";
  }
}
async function useRestoredMatter(restored: Matter) {
  await refreshMatters();
  await openMatterTab(restored.id, "documents");
  notify("Matter restored with originals and working revisions.", "success");
}
async function submitProtectedRestore() {
  if (!protectedRestoreFile.value || busy.value) return;
  busy.value = true;
  protectedRestoreError.value = "";
  try {
    const restored = await restoreMatter(
      protectedRestoreFile.value,
      protectedRestoreCredential.value,
      protectedRestoreRecovery.value,
    );
    protectedRestoreFile.value = null;
    protectedRestoreCredential.value = "";
    await useRestoredMatter(restored);
  } catch (err) {
    protectedRestoreError.value = friendlyError(err);
  } finally {
    busy.value = false;
  }
}
async function onUpload(files: FileList | File[]) {
  if (!currentMatterId.value || busy.value) return;
  const incoming = Array.from(files).filter((f) =>
    /\.(pdf|png|jpe?g)$/i.test(f.name),
  );
  if (!incoming.length) {
    notify("Choose PDF, JPG or PNG files.", "error");
    return;
  }
  busy.value = true;
  let imported = 0;
  try {
    for (const file of incoming) {
      let password: string | undefined;
      let acknowledged = false;
      while (true) {
        try {
          await uploadDocument(
            currentMatterId.value,
            file,
            password,
            acknowledged,
          );
          imported++;
          break;
        } catch (err) {
          if (
            err instanceof ApiError &&
            err.status === 422 &&
            /PDF password required|Incorrect PDF password/.test(err.message)
          ) {
            const supplied = await askPdfImportPassword(
              file.name,
              err.message.includes("Incorrect")
                ? "Incorrect PDF password. Try again or cancel."
                : "",
            );
            password = supplied === null ? undefined : supplied;
            if (supplied === null) {
              notify(`${file.name} import cancelled.`, "info");
              break;
            }
            continue;
          }
          if (
            err instanceof ApiError &&
            err.status === 409 &&
            err.message.includes("Protected PDF signature review required")
          ) {
            acknowledged = window.confirm(
              "This protected PDF may contain certificate signatures. Creating a decrypted working copy can invalidate them. Certificate validity is not verified. The encrypted original is preserved. Continue importing?",
            );
            if (!acknowledged) {
              notify(`${file.name} import cancelled.`, "info");
              break;
            }
            continue;
          }
          throw err;
        }
      }
      password = undefined;
    }
    await loadMatterResources();
    if (imported)
      notify(
        `${imported} document(s) imported as reference-only. Choose Bundle membership to include them. Original files preserved.`,
        "success",
      );
  } catch (err) {
    issue(err);
    await loadMatterResources();
  } finally {
    busy.value = false;
    if (fileInput.value) fileInput.value.value = "";
  }
}
function askPdfImportPassword(
  name: string,
  error: string,
): Promise<string | null> {
  pdfImportPassword.value = "";
  pdfImportRequest.value = { name, error };
  return new Promise((resolve) => {
    pdfImportResolve = resolve;
  });
}
function finishPdfImportPassword(password: string | null) {
  const resolve = pdfImportResolve;
  pdfImportResolve = null;
  pdfImportPassword.value = "";
  pdfImportRequest.value = null;
  resolve?.(password);
}
function onDropUpload(e: DragEvent) {
  hoverUpload.value = false;
  if (e.dataTransfer?.files) void onUpload(e.dataTransfer.files);
}
async function generateCover() {
  if (!currentMatterId.value) return;
  busy.value = true;
  try {
    await api(`/matters/${currentMatterId.value}/cover`, "POST");
    await loadMatterResources();
    notify("Draft cover sheet created from matter details.", "success");
  } catch (err) {
    issue(err);
  } finally {
    busy.value = false;
  }
}
async function resumeAutosaveWhenClean() {
  await nextTick();
  if (!engineBlocked.value && !documentDirty.value && !settingsDirty.value && !formattingDirty.value &&
      !pdfViewer.value?.hasPendingDraft() && !filingGuide.value?.hasPendingDraft() && !workspaceTabs.value.some(tab => tab.id !== activeTabId.value && tab.hadUnsavedChanges))
    autosaveHeld.value = false;
}
async function saveDocument(quiet = false) {
  if (!selectedDoc.value) return false;
  const document = selectedDoc.value;
  const draft = { ...docEditor };
  busy.value = true;
  documentAutosaveState.value = "saving";
  try {
    const preserveAmbiguous =
      !draft.document_date && document.date_status === "ambiguous";
    const updated = await api<MatterDocument>(
      `/documents/${document.id}`,
      "PATCH",
      {
        title: draft.title,
        kind: draft.kind,
        part: draft.part,
        label: draft.label,
        group_key: draft.group_key,
        document_date: draft.document_date,
        visible_signature_status: draft.visible_signature_status,
        date_status: draft.document_date
          ? draft.date_confirmed
            ? "confirmed"
            : document.date_status
          : preserveAmbiguous
            ? "ambiguous"
            : "unknown",
        date_source: draft.document_date
          ? draft.date_confirmed
            ? "user"
            : document.date_source
          : preserveAmbiguous
            ? document.date_source
            : "none",
        date_note: preserveAmbiguous ? document.date_note : "",
      },
    );
    docs.value = docs.value.map((x) => (x.id === updated.id ? updated : x));
    await refreshHistory();
    await validate();
    await loadMatterResources();
    documentAutosaveState.value = "idle";
    await resumeAutosaveWhenClean();
    if (!quiet) notify("Document details updated.", "success");
    return true;
  } catch (err) {
    documentAutosaveState.value = "failed";
    issue(err);
    return false;
  } finally {
    busy.value = false;
  }
}
async function closeDocumentTable() {
  if (!(await guardArtifactTransition(documentTablePanel.value?.isSuggestionsOpen() ? 'batch review' : 'Documents'))) return;
  if(documentTablePanel.value?.isSuggestionsOpen()) documentTablePanel.value.closeSuggestions();
  else {documentTableOpen.value=false; tableDirty.value=false; await nextTick(); document.querySelector<HTMLElement>('.navigator-actions button')?.focus();}
}
async function openDocumentTable() {
  if (!(await guardArtifactTransition('Batch review'))) return;
  documentTableOpen.value=true;
}
async function documentTableModified() {
  await refreshMatters();
  await loadMatterResources();
  notify(
    "Document table changes saved. Bundle order and freshness were refreshed.",
    "success",
  );
}
async function openDocumentFromTable(id: string) {
  if (!(await guardArtifactTransition("this document"))) return;
  documentTableOpen.value = false;
  detailsOpen.value = true;
  void openDocumentTab(id);
}
async function deleteDocument() {
  if (
    !selectedDoc.value ||
    !confirmDiscardDrafts() ||
    !window.confirm(
      `Archive ${selectedDoc.value.title}? Its records and original remain available for restore.`,
    )
  )
    return;
  busy.value = true;
  try {
    await api(`/documents/${selectedDoc.value.id}`, "DELETE");
    selectedDocId.value = null;
    await loadMatterResources();
    notify(
      "Document archived. It can be restored from this matter.",
      "success",
    );
  } catch (err) {
    issue(err);
  } finally {
    busy.value = false;
  }
}
async function persistDocumentOrder(ids: string[]) {
  if (!currentMatterId.value || busy.value) return;
  busy.value = true;
  try {
    docs.value = await api<MatterDocument[]>(
      `/matters/${currentMatterId.value}/documents/reorder`,
      "POST",
      { ids },
    );
    await refreshHistory();
    await validate();
    await loadMatterResources();
    notify("Document order and index page ranges updated.", "success");
  } catch (err) {
    issue(err);
    await loadMatterResources();
  } finally {
    busy.value = false;
  }
}
async function submitDocumentOrder(ids: string[]) {
  if (!currentMatterId.value || busy.value || !confirmDiscardDrafts()) return;
  await persistDocumentOrder(ids);
}
function applyDocumentSort(reordered: MatterDocument[]) {
  const ids = reordered.map((doc) => doc.id);
  const currentIds = docs.value.map((doc) => doc.id);
  const sameDocuments =
    ids.length === currentIds.length &&
    ids.every((id) => currentIds.includes(id));
  if (
    !sameDocuments ||
    busy.value ||
    searching.value.trim() ||
    !confirmDiscardDrafts()
  ) {
    // A new array forces Vue to restore Sortable's moved DOM when a drop is rejected.
    docs.value = [...docs.value];
    return;
  }
  if (ids.every((id, index) => id === currentIds[index])) return;
  docs.value = [...reordered];
  void persistDocumentOrder(ids);
}
async function moveDocumentById(id: string, direction: number) {
  if (!currentMatterId.value) return;
  const ids = docs.value.map((d) => d.id);
  const position = ids.indexOf(id);
  const next = position + direction;
  if (position < 0 || next < 0 || next >= ids.length) return;
  [ids[position], ids[next]] = [ids[next], ids[position]];
  await submitDocumentOrder(ids);
}
function moveDocumentFromHandle(
  id: string,
  direction: number,
  filtered = false,
) {
  if (busy.value || (filtered && searching.value.trim())) return;
  void moveDocumentById(id, direction);
}
async function refreshDocument() {
  await loadMatterResources();
  await resumeAutosaveWhenClean();
}
async function refreshGuide() {
  await refreshHistory();
  await resumeAutosaveWhenClean();
}
async function startOcr(language: "eng" | "hin" | "eng+hin") {
  if (!selectedDoc.value || isJobActive.value) return;
  if (!(await guardArtifactTransition("OCR for the working copy"))) return;
  const signature = confirmSignatureTransformation([selectedDoc.value], "OCR");
  if (!signature.proceed) return;
  try {
    const job = await api<Job>(
      `/documents/${selectedDoc.value.id}/ocr`,
      "POST",
      {
        acknowledge_signature_risk: signature.acknowledged,
        expected_revision: selectedDoc.value.revision,
        language,
      },
    );
    watchJob(job);
    notify(
      "OCR started on scanned and mixed-content pages. Review recognized text.",
      "info",
    );
  } catch (err) {
    issue(err);
  }
}
async function validate(): Promise<PreflightCheck[]> {
  const id = currentMatterId.value;
  if (!id || engineBlocked.value) return [];
  const call = ++validationSequence;
  checksLoading.value = true; checksError.value = ''; checks.value = [];
  try {
    const result = await api<PreflightCheck[]>(`/matters/${id}/validate`);
    if (!engineBlocked.value && id === currentMatterId.value && call === validationSequence) checks.value = result;
    return result;
  } catch (err) {
    if (id === currentMatterId.value && call === validationSequence) {
      checksError.value = 'Technical checks could not be loaded. Previous results are not current.';
      issue(err);
    }
    return [{severity:'error',code:'VALIDATION_UNAVAILABLE',message:'Technical checks could not be loaded.',document_id:null}];
  } finally { if (call === validationSequence) checksLoading.value = false; }
}
async function savePolicy(quiet = false) {
  if (!currentMatter.value) return false;
  settingsAutosaveState.value = "saving";
  try {
    const updated = await api<Matter>(
      `/matters/${currentMatter.value.id}`,
      "PATCH",
      { ...matterEditor },
    );
    matterList.value = matterList.value.map((m) =>
      m.id === updated.id ? updated : m,
    );
    await refreshHistory();
    await loadMatterResources();
    settingsAutosaveState.value = "idle";
    await resumeAutosaveWhenClean();
    if (!quiet) notify("Bundle settings saved.", "success");
    return true;
  } catch (err) {
    settingsAutosaveState.value = "failed";
    issue(err);
    return false;
  }
}
async function applyOutputPreset(presetId: string) {
  if (!currentMatterId.value || !confirmDiscardDrafts()) return;
  try {
    await api(
      `/matters/${currentMatterId.value}/output-preset/${presetId}`,
      "POST",
    );
    await refreshMatters();
    resetDrafts();
    await loadMatterResources();
    notify(
      "Preset version applied. Preview marks on actual pages before building.",
      "success",
    );
  } catch (err) {
    issue(err);
  }
}
async function startBuild(): Promise<boolean> {
  if (!currentMatterId.value || isJobActive.value || buildSubmitting.value) return false;
  buildSubmitting.value=true;
  const submittedMatterId=currentMatterId.value;
  try {
    if (formattingDirty.value) {
      notify("Save numbering and annexure policy before building.", "error");
      return false;
    }
    if (!(await savePolicy()) || currentMatterId.value!==submittedMatterId) return false;
    const signature = confirmSignatureTransformation(
      includedDocuments.value,
      "Bundle generation",
    );
    if (!signature.proceed) return false;
    const job = await api<Job>(
      `/matters/${submittedMatterId}/build`,
      "POST",
      {
        acknowledge_signature_risk: signature.acknowledged,
      },
    );
    if(currentMatterId.value===submittedMatterId)watchJob(job);
    return true;
  } catch (err) {
    issue(err);
    return false;
  } finally { buildSubmitting.value=false; }
}
function watchJob(job: Job) {
  activeJob.value = job;
  cancelPending.value = false;
  const matterId = currentMatterId.value;
  const pollSequence=++jobPollSequence;
  if (jobTimer) clearInterval(jobTimer);
  let fetching = false;
  jobTimer = setInterval(async () => {
    if (fetching) return;
    fetching = true;
    try {
      const next = await api<Job>(`/jobs/${job.id}`);
      if (pollSequence!==jobPollSequence || currentMatterId.value !== matterId || activeJob.value?.id !== job.id) return;
      activeJob.value = next;
      if (["completed", "failed", "cancelled", "interrupted"].includes(next.status)) {
        if (jobTimer) clearInterval(jobTimer);
        jobTimer = null;
        cancelPending.value = false;
        if (next.status === "failed")
          notify(next.error || "Job failed", "error");
        if (next.status === "completed")
          notify(
            next.kind === "ocr"
              ? "OCR processing finished."
              : next.kind === "compress"
                ? "PDF compression finished. Review the size and quality."
                : "Filing bundle generated successfully.",
            "success",
          );
        if (next.status === "cancelled")
          notify("Processing cancelled.", "info");
        await loadMatterResources();
      }
    } catch (err) {
      if(pollSequence!==jobPollSequence)return;
      if (jobTimer) clearInterval(jobTimer);
      jobTimer = null;
      issue(err);
    } finally {
      fetching = false;
    }
  }, 650);
}
async function cancelJob() {
  if (!activeJob.value || cancelPending.value) return;
  const id = activeJob.value.id;
  cancelPending.value = true;
  try { await api(`/jobs/${id}/cancel`, "POST"); }
  catch (err) { if (activeJob.value?.id===id) cancelPending.value=false; issue(err); }
}
async function retryInterrupted(job: Job) {
  try {
    const relevant =
      job.kind === "ocr"
        ? docs.value.filter((document) => document.id === job.target_id)
        : includedDocuments.value;
    const signature = confirmSignatureTransformation(
      relevant,
      job.kind === "ocr" ? "Retrying OCR" : "Retrying bundle generation",
    );
    if (!signature.proceed) return;
    const retry = await api<Job>(`/jobs/${job.id}/retry`, "POST", {
      acknowledge_signature_risk: signature.acknowledged,
      ...(job.kind === "ocr" ? { language: retryOcrLanguage.value } : {}),
    });
    watchJob(retry);
    await loadMatterResources();
    notify(`Restarted ${job.kind} as a new job.`, "info");
  } catch (err) {
    issue(err);
  }
}
async function download(exportId: string, filename: string, saveAs = false) {
  if (!currentMatterId.value || savingExport.value) return;
  savingExport.value = true;
  try {
    const saved = await downloadExport(
      currentMatterId.value,
      exportId,
      filename,
      saveAs,
    );
    if (saved && inTauri()) notify(`${filename} saved.`, "success");
  } catch (err) {
    issue(err);
  } finally {
    savingExport.value = false;
  }
}
async function saveOfficeReview(
  exportId: string,
  filename: string,
  kind: "docx" | "xlsx" | "pptx",
) {
  if (!currentMatterId.value || savingExport.value) return;
  savingExport.value = true;
  try {
    const saved = await downloadOfficeReview(
      currentMatterId.value,
      exportId,
      filename,
      kind,
    );
    if (saved && inTauri())
      notify(
        `${kind.toUpperCase()} review draft saved. Compare it with the PDF before use.`,
        "success",
      );
  } catch (err) {
    issue(err);
  } finally {
    savingExport.value = false;
  }
}
function clearPrintPreview() {
  if (printPreviewUrl.value) URL.revokeObjectURL(printPreviewUrl.value);
  printPreviewUrl.value = "";
}
watch([printPages, printOrientation, printScale], clearPrintPreview);
function beginPrint(exportId: string, filename: string) {
  clearPrintPreview();
  printRequest.value = { kind: "export", exportId, filename };
  printPages.value = "all";
  printOrientation.value = "source";
  printScale.value = "fit";
  printError.value = "";
}
function beginWorkingPrint(request: {
  documentId: string;
  revision: number;
  title: string;
  acknowledged: boolean;
}) {
  clearPrintPreview();
  printRequest.value = {
    kind: "working",
    documentId: request.documentId,
    revision: request.revision,
    filename: request.title,
    acknowledged: request.acknowledged,
  };
  printPages.value = "all";
  printOrientation.value = "source";
  printScale.value = "fit";
  printError.value = "";
}
function closePrint() {
  clearPrintPreview();
  printRequest.value = null;
  printError.value = "";
}
async function preparePrint() {
  if (!printRequest.value || printBusy.value) return;
  clearPrintPreview();
  printBusy.value = true;
  printError.value = "";
  try {
    const options = {
      pages: printPages.value,
      orientation: printOrientation.value,
      scale: printScale.value,
    };
    const request = printRequest.value;
    const file =
      request.kind === "working"
        ? await printableWorkingPdf(
            request.documentId,
            request.revision,
            options,
            request.acknowledged,
          )
        : await printableExport(
            currentMatterId.value!,
            request.exportId,
            request.filename,
            options,
          );
    printPreviewUrl.value = URL.createObjectURL(file);
  } catch (err) {
    printError.value = friendlyError(err);
  } finally {
    printBusy.value = false;
  }
}
function openPrintDialog() {
  try {
    printFrame.value?.contentWindow?.focus();
    printFrame.value?.contentWindow?.print();
  } catch {
    printError.value =
      "The system print dialog could not open. Save the PDF and print it in your desktop PDF viewer.";
  }
}
function beginProtectedExport(exportId: string, filename: string) {
  protectedExportPassword.value = "";
  protectedExportConfirm.value = "";
  protectedExportError.value = "";
  protectedExportRequest.value = { exportId, filename };
}
function cancelProtectedExport() {
  protectedExportPassword.value = "";
  protectedExportConfirm.value = "";
  protectedExportError.value = "";
  protectedExportRequest.value = null;
}
async function saveProtectedExport() {
  const request = protectedExportRequest.value;
  if (!request || !currentMatterId.value || savingExport.value) return;
  if (protectedExportPassword.value !== protectedExportConfirm.value) {
    protectedExportError.value = "Passwords do not match.";
    return;
  }
  if (
    !protectedExportPassword.value ||
    new TextEncoder().encode(protectedExportPassword.value).length > 40
  ) {
    protectedExportError.value = "Enter a password of 1 to 40 UTF-8 bytes.";
    return;
  }
  savingExport.value = true;
  protectedExportError.value = "";
  try {
    const saved = await downloadProtectedPdf(
      currentMatterId.value,
      request.exportId,
      request.filename,
      protectedExportPassword.value,
    );
    if (saved) {
      cancelProtectedExport();
      notify(
        "Password-protected PDF saved. Reopen it with the password before use.",
        "success",
      );
    }
  } catch (err) {
    protectedExportError.value = friendlyError(err);
  } finally {
    savingExport.value = false;
  }
}
async function openGeneratedSource(id:string, page:number, revision:number) {
  const doc=docs.value.find(item=>item.id===id);
  if (!doc || doc.revision!==revision) { notify('The working revision has changed or is unavailable. The generated PDF remains the authoritative earlier output.', 'error'); return; }
  reviewingPdf.value=null;
  await openDocumentTab(id,page,revision);
}
async function reviewGeneratedPdf(exportId: string, filename: string) {
  if (!(await guardArtifactTransition(`build ${exportId.slice(0,8)} · ${filename}`))) return;
  reviewingPdf.value = { exportId, filename };
}
function selectMatter(id: string, view: View = "documents") {
  workspaceMenuOpen.value = false;
  void openMatterTab(id, view);
}
async function removeMatter() {
  if (
    !currentMatter.value ||
    !confirmDiscardDrafts() ||
    !window.confirm(
      "Archive this matter? Its records, originals and working files remain available for restore.",
    )
  )
    return;
  try {
    await api(`/matters/${currentMatter.value.id}`, "DELETE");
    currentMatterId.value = null;
    docs.value = [];
    await refreshMatters();
    notify("Matter archived.", "info");
  } catch (err) {
    issue(err);
  }
}
async function restoreArchivedMatter(id: string) {
  if (!confirmDiscardDrafts()) return;
  try {
    await api(`/matters/${id}/restore`, "POST");
    await refreshMatters();
    selectMatter(id);
    notify("Archived matter restored.", "success");
  } catch (err) {
    issue(err);
  }
}
async function restoreArchivedDocument(id: string) {
  if (!currentMatterId.value) return;
  try {
    await api(
      `/matters/${currentMatterId.value}/archived-documents/${id}/restore`,
      "POST",
    );
    await loadMatterResources();
    notify("Archived document restored.", "success");
  } catch (err) {
    issue(err);
  }
}
const iconsBySeverity = {
  error: CircleAlert,
  warning: ShieldAlert,
  info: Info,
};
const matterMore = ref<HTMLDetailsElement | null>(null);
function closeWorkspaceTabsOutside(event: Event) {
  const target = event.target as HTMLElement;
  if (tabsMenuOpen.value && !target.closest('.open-workspace-tabs, .application-matter, dialog, [role="alertdialog"]')) tabsMenuOpen.value = false;
}
function closeMatterMoreOnOutsideClick(event: PointerEvent) {
  closeWorkspaceTabsOutside(event);
  if (matterMore.value && !matterMore.value.contains(event.target as Node)) {
    matterMore.value.open = false;
  }
}
onMounted(() => {
  try {
    sidebarCollapsed.value =
      localStorage.getItem("courtbundle.sidebarCollapsed.v1") === "true";
  } catch {
    /* Storage may be unavailable. */
  }
  void init();
  healthTimer = setInterval(() => void checkHealth(), 5_000);
  window.addEventListener("beforeunload", beforeUnload);
  window.addEventListener("keydown", onKeydown);
  window.addEventListener("resize", onResize);
  document.addEventListener("pointerdown", closeMatterMoreOnOutsideClick);
  document.addEventListener("focusin", closeWorkspaceTabsOutside);
});
onUnmounted(() => {
  clearPrintPreview();
  if (documentAutosaveTimer) clearTimeout(documentAutosaveTimer);
  if (settingsAutosaveTimer) clearTimeout(settingsAutosaveTimer);
  if (jobTimer) clearInterval(jobTimer);
  if (toastTimer) clearTimeout(toastTimer);
  if (healthTimer) clearInterval(healthTimer);
  window.removeEventListener("beforeunload", beforeUnload);
  window.removeEventListener("keydown", onKeydown);
  window.removeEventListener("resize", onResize);
  document.removeEventListener("pointerdown", closeMatterMoreOnOutsideClick);
  document.removeEventListener("focusin", closeWorkspaceTabsOutside);
});
watch(sidebarCollapsed, (collapsed) => {
  try {
    localStorage.setItem("courtbundle.sidebarCollapsed.v1", String(collapsed));
  } catch {
    /* Storage may be unavailable. */
  }
});
</script>

<template>
  <main
    v-if="vaultState !== 'unlocked' && apiStatus !== 'offline'"
    class="vault-screen"
  >
    <section
      :class="['vault-card', 'vault-state-' + vaultState]"
      aria-labelledby="vault-title"
    >
      <div
        :class="['vault-mark', { loading: vaultState === 'checking' }]"
        aria-hidden="true"
      >
        <LoaderCircle v-if="vaultState === 'checking'" :size="27" />
        <LockKeyhole v-else :size="27" />
      </div>
      <div class="vault-heading">
        <span>{{
          vaultState === "checking"
            ? "LOCAL ENGINE"
            : vaultState === "show_recovery"
              ? "RECOVERY KEY"
              : "PROTECTED WORKSPACE"
        }}</span>
        <h1 id="vault-title">
          {{
            vaultState === "setup"
              ? "Protect local matters"
              : vaultState === "locked"
                ? "Unlock CourtBundle"
                : vaultState === "show_recovery"
                  ? "Save your recovery key"
                  : vaultState === "change_password"
                    ? "Change vault password"
                    : "Starting local engine"
          }}
        </h1>
      </div>
      <p v-if="vaultState === 'checking'">
        Opening the encrypted local workspace. CourtBundle stays on this
        computer and does not upload matter files.
      </p>
      <p v-if="vaultState === 'setup'">
        Create a password for the local matter store. The existing local data
        will be migrated before the workspace opens. Keep the separate recovery
        key outside this computer.
      </p>
      <p v-else-if="vaultState === 'locked'">
        Enter your password or recovery key. If both are lost, encrypted matter
        data cannot be recovered.
      </p>
      <p v-else-if="vaultState === 'change_password'">
        Changing the password creates a new recovery key. Previous recovery keys
        stop unlocking this local store.
      </p>
      <p v-else-if="vaultState === 'show_recovery'">
        Write down this key and keep it separately. It is shown only now and is
        not stored in the app.
      </p>
      <div
        v-if="vaultState === 'checking'"
        class="vault-progress"
        role="progressbar"
        aria-label="Starting local engine"
      >
        <span></span>
      </div>
      <p v-if="vaultError" class="notice amber" role="alert">
        {{ vaultError }}
      </p>
      <form
        v-if="vaultState === 'setup' || vaultState === 'change_password'"
        @submit.prevent="vaultState === 'setup' ? setupVault() : rotateVault()"
      >
        <PasswordInput
          v-model="vaultCredential"
          label="New password"
          autocomplete="new-password"
          :minlength="6"
        />
        <PasswordInput
          v-model="vaultConfirm"
          label="Confirm password"
          autocomplete="new-password"
          :minlength="6"
        />
        <small
          >Minimum 6 characters. A longer password is safer; keep your recovery
          key separately.</small
        >
        <button class="btn primary" type="submit" :disabled="vaultBusy">
          {{
            vaultBusy
              ? "Protecting local files…"
              : vaultState === "setup"
                ? "Protect and open"
                : "Change password"
          }}
        </button>
        <button
          v-if="vaultState === 'change_password'"
          class="btn secondary"
          type="button"
          @click="
            vaultState = 'unlocked';
            vaultCredential = '';
            vaultConfirm = '';
            vaultError = '';
          "
        >
          Cancel
        </button>
      </form>
      <form v-else-if="vaultState === 'locked'" @submit.prevent="unlockVault()">
        <label v-if="vaultRecoveryMode"
          >Recovery key<input
            v-model="vaultCredential"
            type="text"
            autocomplete="off"
            required
        /></label>
        <PasswordInput
          v-else
          v-model="vaultCredential"
          label="Password"
          autocomplete="current-password"
        />
        <label class="vault-choice"
          ><input v-model="vaultRecoveryMode" type="checkbox" /> Use recovery
          key</label
        >
        <button class="btn primary" type="submit" :disabled="vaultBusy">
          {{ vaultBusy ? "Unlocking…" : "Unlock" }}
        </button>
      </form>
      <div v-else-if="vaultState === 'show_recovery'" class="vault-recovery">
        <div class="vault-recovery-key">
          <span>RECOVERY KEY</span><code>{{ vaultRecoveryKey }}</code>
        </div>
        <div class="vault-recovery-warning">
          <ShieldAlert :size="18" /><span
            >Store this separately from your password. Anyone with this key can
            unlock the local matter store.</span
          >
        </div>
        <button class="btn primary" @click="finishVaultSetup">
          I saved the recovery key
        </button>
      </div>
    </section>
  </main>
  <div v-else class="app-shell design-shell" :data-batch="documentTableOpen" :data-finding="!!preflightContext && activeView==='documents'" :data-preparation="preparationVisible || documentTableOpen || buildShell" :data-build="expandedBuildShell" :data-membership="membershipOpen" :data-membership-collapsed="membershipCollapsed" :data-compact-preparation="compactDocuments" :data-engine-unavailable="engineBlocked" :data-editor-expanded="editorExpanded" :data-documents="activeView === 'documents'" :style="{ '--documents-interface-scale': uiScale / 100 }">
    <a class="skip-link" href="#main-content" @click.prevent="focusMain"
      >Skip to main content</a
    >
    <header class="application-bar">
      <button class="application-brand" aria-label="Workspace navigation" :aria-expanded="workspaceMenuOpen" @click="workspaceMenuOpen=!workspaceMenuOpen"><span><FileStack :size="14"/></span><strong>CourtBundle</strong></button>
      <button class="application-matter" aria-label="Open workspaces" :aria-expanded="tabsMenuOpen" @click="tabsMenuOpen=!tabsMenuOpen">{{currentMatter?.name || 'Your matters'}}<ChevronDown :size="13"/></button>
      <span class="application-local"><ShieldCheck :size="16"/> Local workspace</span>
      <button class="icon-btn small" aria-label="Workspace settings" title="Workspace settings" @click="workspaceMenuOpen=!workspaceMenuOpen"><Settings2 :size="16"/></button>
    </header>
    <nav class="matter-rail" aria-label="Workspace navigation rail">
      <div v-if="membershipOpen || expandedBuildShell" class="membership-matter"><strong>{{currentMatter?.name}}</strong><span>{{currentMatter?.case_number || 'Draft matter'}}</span></div>
      <button aria-label="Matters" title="Matters" @click="showView('matters')"><BriefcaseBusiness :size="18"/></button>
      <template v-if="currentMatter">
        <button aria-label="Overview" title="Overview" :aria-current="!membershipOpen && activeView==='overview'?'page':undefined" @click="showView('overview')"><LayoutGrid :size="18"/><span v-if="membershipOpen || expandedBuildShell">Overview</span></button>
        <button aria-label="Documents" title="Documents" :aria-current="!membershipOpen && activeView==='documents'?'page':undefined" @click="showPreparation"><Files v-if="membershipOpen || expandedBuildShell" :size="18"/><FileStack v-else :size="18"/><span v-if="membershipOpen || expandedBuildShell">Documents</span></button>
        <button aria-label="Index & bookmarks" title="Index & bookmarks" :aria-current="membershipOpen || activeView==='index'?'page':undefined" @click="showView('index')"><ListTree v-if="membershipOpen || expandedBuildShell" :size="18"/><BookMarked v-else :size="18"/><span v-if="membershipOpen || expandedBuildShell">Index & Bookmarks</span></button>
        <button aria-label="Preflight checks" title="Preflight checks" :aria-current="!membershipOpen && activeView==='validation'?'page':undefined" @click="showView('validation')"><ListChecks v-if="membershipOpen || expandedBuildShell" :size="18"/><FileCheck2 v-else :size="18"/><span v-if="membershipOpen || expandedBuildShell">Preflight</span></button>
        <button aria-label="Build bundle" title="Build bundle" :aria-current="!membershipOpen && activeView==='build'?'page':undefined" @click="showView('build')"><Box v-if="membershipOpen || expandedBuildShell" :size="18"/><Layers3 v-else :size="18"/><span v-if="membershipOpen || expandedBuildShell">Build bundle</span></button>
        <button aria-label="Storage" title="Storage" :aria-current="!membershipOpen && activeView==='storage'?'page':undefined" @click="showView('storage')"><HardDrive v-if="membershipOpen || expandedBuildShell" :size="18"/><Archive v-else :size="18"/><span v-if="membershipOpen || expandedBuildShell">Storage</span></button>
      </template>
      <button v-if="!membershipOpen && !expandedBuildShell" class="rail-settings" aria-label="Navigation and appearance" title="Navigation and appearance" @click="workspaceMenuOpen=!workspaceMenuOpen"><PanelLeftOpen :size="18"/></button>
    <div v-if="membershipOpen || expandedBuildShell" class="membership-rail-bottom"><button disabled title="Opening a matter folder is not exposed by this local service"><FolderOpen :size="16"/><span>Open matter folder</span></button><button :aria-label="membershipCollapsed?'Expand sidebar':'Collapse sidebar'" @click="membershipCollapsed=!membershipCollapsed"><PanelLeftOpen :size="16"/><span>{{membershipCollapsed?'Expand sidebar':'Collapse sidebar'}}</span></button></div></nav>
    <button v-if="workspaceMenuOpen" class="navigation-scrim" aria-label="Dismiss navigation backdrop" @click="workspaceMenuOpen=false"></button>
    <aside
      v-show="workspaceMenuOpen"
      class="sidebar workspace-drawer"
      :class="{
        collapsed: false,
        'mobile-open': true,
      }"
      @keydown.esc="workspaceMenuOpen=false"
    >
      <div class="brand-area">
        <div class="brand-mark"><Layers3 :size="22" :stroke-width="2.3" /></div>
        <div class="brand-copy">
          <div class="brand-name">
            CourtBundle<span class="brand-plus">.</span>
          </div>
          <div class="brand-sub">LEGAL WORKSPACE / POC</div>
        </div>
        <button class="sidebar-toggle" aria-label="Close workspace navigation" title="Close workspace navigation" @click="workspaceMenuOpen=false">
          <X :size="19" />
        </button>
      </div>
      <div class="side-label">WORKSPACE</div>
      <nav class="side-nav">
        <button
          class="side-link"
          aria-label="Matters"
          title="Matters"
          :class="{ active: activeView !== 'rules' && activeView !== 'about' }"
          @click="showView('matters')"
        >
          <BriefcaseBusiness :size="18" /><span class="nav-label">Matters</span
          ><span class="side-count">{{ matterList.length }}</span>
        </button>
        <button
          class="side-link"
          aria-label="Court rules"
          title="Court rules"
          :class="{ active: activeView === 'rules' }"
          @click="showView('rules')"
        >
          <BookOpen :size="18" /><span class="nav-label">Court rules</span>
        </button>
        <button
          class="side-link"
          aria-label="About POC"
          title="About POC"
          :class="{ active: activeView === 'about' }"
          @click="showView('about')"
        >
          <CircleHelp :size="18" /><span class="nav-label">About POC</span>
        </button>
      </nav>
      <input
        ref="matterFileInput"
        type="file"
        accept=".cbmatter"
        hidden
        @change="
          (e) => importMatterPackage((e.target as HTMLInputElement).files)
        "
      />
      <button
        class="side-link"
        aria-label="Restore matter"
        title="Restore matter"
        :disabled="busy"
        @click="matterFileInput?.click()"
      >
        <FolderOpen :size="18" /><span class="nav-label">Restore matter</span>
      </button>
      <div class="side-label matter-section-label">
        <span class="nav-label">YOUR MATTERS</span
        ><button
          aria-label="New matter"
          title="New matter"
          class="side-plus"
          @click="modalOpen = true"
        >
          <Plus :size="17" />
        </button>
      </div>
      <div class="matter-list">
        <button
          v-for="m in matterList"
          :key="m.id"
          class="matter-link"
          :aria-label="m.name"
          :title="m.name"
          :class="{
            active:
              currentMatterId === m.id &&
              activeView !== 'rules' &&
              activeView !== 'about',
          }"
          @click="selectMatter(m.id)"
        >
          <div class="matter-initial">
            {{ m.name.slice(0, 1).toUpperCase() }}
          </div>
          <div class="matter-link-label">
            <span>{{ m.name }}</span
            ><small>{{ m.case_type }}</small>
          </div>
        </button>
        <div v-if="!matterList.length" class="sidebar-empty">
          No matters yet
        </div>
      </div>
      <div v-if="archivedMatters.length" class="side-label">
        ARCHIVED MATTERS
      </div>
      <div v-if="archivedMatters.length" class="matter-list">
        <button
          v-for="m in archivedMatters"
          :key="m.id"
          class="matter-link"
          :aria-label="'Restore ' + m.name"
          :title="'Restore ' + m.name"
          @click="restoreArchivedMatter(m.id)"
        >
          <Archive :size="17" /><span>{{ m.name }}</span>
        </button>
      </div>
      <div class="sidebar-bottom">
        <button
          v-if="vaultEnabled"
          class="side-link"
          @click="
            vaultError = '';
            vaultState = 'change_password';
          "
        >
          <LockKeyhole :size="17" /><span>Change vault password</span>
        </button>
        <div class="appearance-controls">
          <label
            >Theme<select v-model="theme" aria-label="Theme">
              <option value="system">System</option>
              <option value="light">Light</option>
              <option value="dark">Dark</option>
            </select></label
          >
          <label
            >Interface size<select
              v-model.number="uiScale"
              aria-label="Interface size"
            >
              <option :value="90">90%</option>
              <option :value="100">100%</option>
              <option :value="110">110%</option>
              <option :value="125">125%</option>
            </select></label
          >
          <small>/ search · ⌘/Ctrl+Z undo · Shift+⌘/Ctrl+Z redo</small>
        </div>
        <div class="offline-status">
          <CloudOff :size="17" />
          <div>
            <strong>Offline-first</strong
            ><small>Files stay on this computer</small>
          </div>
        </div>
        <div class="version-chip">
          <span :class="['status-dot', apiStatus]"></span>
          {{
            apiStatus === "online"
              ? "Local engine connected"
              : apiStatus === "offline"
                ? "Engine disconnected"
                : "Connecting…"
          }}
          <span class="version-text">v0.1</span>
        </div>
        <div class="sidebar-profile" aria-label="Local workspace profile">
          <span class="sidebar-profile-avatar"><UserRound :size="18" /></span
          ><span class="sidebar-profile-copy"
            ><strong>Local workspace</strong><small>On this device</small></span
          >
        </div>
      </div>
    </aside>

    <div
      class="app-main"
      :class="{
        'workspace-main':
          currentMatter && activeView !== 'rules' && activeView !== 'about',
      }"
    >
      <nav
        v-show="tabsMenuOpen || preparationVisible || documentTableOpen || membershipOpen || buildShell"
        class="open-workspace-tabs"
        aria-label="Open matter and document tabs"
        @keydown.esc="tabsMenuOpen=false"
      >
        <button
          v-if="smallScreen"
          class="mobile-nav-toggle"
          type="button"
          :aria-label="mobileNavOpen ? 'Close navigation' : 'Open navigation'"
          :aria-expanded="mobileNavOpen"
          @click="toggleNavigation"
        >
          <Menu :size="19" />
        </button>
        <span v-if="!workspaceTabs.length" class="tabs-empty-label">{{
          activeView === "rules"
            ? "Court rules"
            : activeView === "about"
              ? "About POC"
              : "Matters"
        }}</span>
        <div
          v-for="tab in workspaceTabs"
          :key="tab.id"
          class="open-workspace-tab"
          :class="{ active: tabIsActive(tab) }"
        >
          <button
            type="button"
            class="open-workspace-tab-select"
            :aria-current="tabIsActive(tab) ? 'page' : undefined"
            :title="`${tab.documentId ? 'Document' : 'Matter'}: ${tabLabel(tab)}`"
            @click="activateTabFromMenu(tab)"
          >
            <FileText v-if="tab.documentId" :size="14" /><BriefcaseBusiness
              v-else
              :size="14"
            /><span>{{ tabLabel(tab) }}</span
            ><span
              v-if="
                tab.id === activeTabId
                  ? tableDirty || documentDirty || settingsDirty || formattingDirty
                  : tab.hadUnsavedChanges
              "
              class="tab-dirty-dot"
              aria-label="Unsaved changes"
              title="Unsaved changes"
              >●</span
            >
          </button>
          <button
            type="button"
            class="open-workspace-tab-close"
            :aria-label="`Close ${tabLabel(tab)} tab`"
            :title="`Close ${tabLabel(tab)} tab`"
            @click="closeWorkspaceTab(tab)"
          >
            <X :size="13" />
          </button>
        </div>
      </nav>

      <div v-if="preparationVisible || documentTableOpen || membershipOpen || buildShell" class="preparation-utility">
        <button class="preparation-search-trigger" @click="openPreparationSearch"><Search :size="16"/> Search documents and page text <kbd>Ctrl K</kbd></button>
        <span class="preparation-save-state"><CircleAlert v-if="tableDirty || documentDirty || settingsDirty || formattingDirty || membershipDirty" :size="14"/><CheckCircle2 v-else-if="membershipOpen" class="membership-saved-icon" :size="14"/>{{membershipDirty ? 'Unapplied changes' : (tableDirty || documentDirty || settingsDirty || formattingDirty) ? 'Unsaved changes' : 'Saved locally'}}</span>
        <button class="icon-btn small" aria-label="Undo last matter change" :disabled="membershipOpen || engineBlocked || busy || !history.can_undo" :title="undoDescription" @click="traverseHistory('undo')"><Undo2 :size="16"/></button>
        <button class="icon-btn small" aria-label="Redo last matter change" :disabled="membershipOpen || engineBlocked || busy || !history.can_redo" :title="redoDescription" @click="traverseHistory('redo')"><Redo2 :size="16"/></button>
        <span class="preparation-engine"><CheckCircle2 :size="14"/>{{engineBlocked?'Engine disconnected':'Engine connected'}}</span>
      </div>

      <div
        v-if="tabDiscardRequest"
        class="vault-restore-overlay"
        role="presentation"
      >
        <section
          class="vault-card tab-discard-dialog"
          @keydown.tab="trapNavigationFocus"
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="tab-discard-title"
          aria-describedby="tab-discard-message"
        >
          <h2 id="tab-discard-title">Unsaved work in this tab</h2>
          <p id="tab-discard-message">{{ tabDiscardRequest.message }}</p>
          <p v-if="navigationError" role="alert" class="error-msg">{{navigationError}}</p>
          <p v-if="!tabDiscardRequest.canSave" class="hint">Activate each inactive tab to save its retained draft before closing these tabs.</p>
          <div class="tab-discard-actions">
            <button class="btn secondary" :disabled="navigationSaving" @click="answerTabDiscard(false)">
              Keep editing</button
            ><button class="btn danger" :disabled="navigationSaving" @click="answerTabDiscard(true)">
              Discard changes
            </button><button class="btn primary" :disabled="engineBlocked || navigationSaving || busy || !tabDiscardRequest.canSave" @click="saveBeforeNavigation">{{navigationSaving ? 'Saving…' : 'Save changes'}}</button>
          </div>
        </section>
      </div>
      <BundleMembership v-if="membershipOpen && currentMatterId" ref="membershipPanel" :matter-id="currentMatterId" :blocked="engineBlocked || busy || isJobActive" @dirty="membershipDirty=$event" @applied="membershipApplied" @close="leaveMembership" @open="openMembershipDocument" @retry="retryEngine" @review="reviewRecoveredState" />
      <EngineNotice v-if="!membershipOpen" @retry="retryEngine" @review="reviewRecoveredState" />

      <FindingReviewPanel v-if="preflightContext && activeView === 'documents' && currentMatterId === preflightContext.matterId" :matter-id="currentMatterId" :finding="preflightContext.finding" :changed="preflightContext.finding.target_revision !== undefined && selectedDoc?.revision !== preflightContext.finding.target_revision" @return="returnToPreflight" @tools="pageToolsOpen=true"/>

      <Transition name="view" mode="out-in" @after-enter="fitDocumentsWorkspace">
        <main
          v-if="activeView === 'rules'"
          id="main-content"
          tabindex="-1"
          class="page-content rules-page"
        >
          <div class="page-eyebrow">LEGAL KNOWLEDGE</div>
          <h1>Court rules & technical profiles</h1>
          <p class="page-description">
            Source-linked preparation checks. These do not replace registry
            instructions or an advocate's review.
          </p>
          <div class="notice subtle profile-scope">
            <Info :size="18" /><span
              ><strong v-if="currentMatter"
                >{{ selectedProfile?.name }} is selected for
                {{ currentMatter.name }}. </strong
              >Profiles currently affect searchable-page warnings and the export
              file size cap. The filing guidance and linked forms below are
              references for advocate review; they are not applied
              automatically.</span
            >
          </div>
          <div class="rules-grid">
            <div
              v-for="profile in profileList"
              :key="profile.id"
              class="card rule-card"
            >
              <div class="rule-icon"><BookMarked :size="24" /></div>
              <div class="badge muted">
                {{
                  profile.status === "DEMO_ONLY"
                    ? "DEMO ONLY"
                    : "NOT LEGALLY APPROVED"
                }}
              </div>
              <div
                v-if="currentMatter?.court_profile === profile.id"
                class="badge profile-selected"
              >
                SELECTED FOR THIS MATTER
              </div>
              <h2>{{ profile.name }}</h2>
              <p>{{ profile.description }}</p>
              <div class="rule-facts">
                <div>
                  <span>File size setting</span
                  ><strong>{{
                    profile.max_file_mb ? profile.max_file_mb + " MB" : "None"
                  }}</strong>
                </div>
                <div>
                  <span>Searchable text check</span
                  ><strong>{{
                    profile.require_searchable_text ? "Enabled" : "Off"
                  }}</strong>
                </div>
                <div>
                  <span>Source review date</span
                  ><strong>{{ profile.last_checked }}</strong>
                </div>
              </div>
              <div class="rule-sources" v-if="profile.sources.length">
                <span>OFFICIAL REFERENCE LINKS</span
                ><a
                  v-for="source in profile.sources"
                  :key="source.url"
                  :href="source.url"
                  target="_blank"
                  rel="noopener noreferrer"
                  @click.prevent="openOfficialReference(source.url)"
                  >{{ source.title }} <ArrowRight :size="13"
                /></a>
              </div>
            </div>
          </div>
          <CourtProfileGuide
            v-if="delhiGuidance"
            :key="guideReloadKey"
            ref="filingGuide"
            :guidance="delhiGuidance"
            :matter-id="currentMatterId"
            :can-undo="history.can_undo"
            :can-redo="history.can_redo"
            @open-url="openOfficialReference"
            @modified="refreshGuide"
            @undo="traverseHistory('undo')"
            @redo="traverseHistory('redo')"
          />
          <div class="notice amber">
            <ShieldAlert :size="19" /> These rules are only a selected technical
            subset. Case-type documents, notarisation, signatures, fees,
            limitation and court acceptance are outside the POC's validation
            scope.
          </div>
        </main>

        <main
          v-else-if="activeView === 'about'"
          id="main-content"
          tabindex="-1"
          class="page-content about-page"
        >
          <div class="page-eyebrow">PRODUCT DEMONSTRATION</div>
          <h1>Built for structured legal filing preparation.</h1>
          <p class="page-description">
            A local-first proof of concept connecting Tauri, Vue and Python to
            convert source documents into indexed legal bundles.
          </p>
          <div class="feature-strip">
            <div>
              <FileStack :size="23" />
              <h3>Preserve originals</h3>
              <p>
                Working copies are edited separately from imported source
                documents.
              </p>
            </div>
            <div>
              <WandSparkles :size="23" />
              <h3>Deterministic assembly</h3>
              <p>
                Volume splitting, bookmarks and indexes share the same page map.
              </p>
            </div>
            <div>
              <ShieldCheck :size="23" />
              <h3>Visible limitations</h3>
              <p>
                Illustrative rules and a validation report, not a filing
                approval.
              </p>
            </div>
          </div>
          <div class="card about-card">
            <h2>What is included?</h2>
            <p>
              Matters, PDF/JPEG/PNG import, working-copy page operations,
              English and Hindi OCR, drag-drop ordering, annexure/part metadata,
              auto index, volume splitting, linked bookmarks, technical
              validation, PDF/ZIP export and editable Office review drafts.
            </p>
            <h2>What is not included?</h2>
            <p>
              Actual court submission, legally approved templates, digital
              signing, layout-faithful Office conversion, comprehensive PDF text
              editing, cloud sharing or guaranteed legal compliance.
            </p>
          </div>
        </main>

        <MatterHome
          v-else-if="activeView === 'matters'"
          :matters="matterList"
          :creating="creating"
          @open="(id) => selectMatter(id, 'overview')"
          @create="modalOpen = true"
          @sample="createSampleMatter"
        />

        <main
          v-else-if="!currentMatter"
          id="main-content"
          tabindex="-1"
          class="page-content welcome-page"
        >
          <div class="welcome-icon"><FolderOpen :size="32" /></div>
          <div class="page-eyebrow">YOUR FIRST MATTER</div>
          <h1>From scattered files to<br />a structured court bundle.</h1>
          <p>
            Create a matter to import documents, organise annexures, generate
            indexes and prepare a complete PDF package—offline.
          </p>
          <div class="welcome-actions">
            <button class="btn primary large" @click="modalOpen = true">
              <Plus :size="18" /> Create your first matter</button
            ><button
              class="btn secondary large"
              :disabled="creating"
              @click="createSampleMatter"
            >
              <WandSparkles :size="18" />
              {{ creating ? "Opening sample…" : "Open sample matter" }}
            </button>
          </div>
          <div class="welcome-steps">
            <span>01 &nbsp; Add documents</span><ArrowRight :size="18" /><span
              >02 &nbsp; Collate & OCR</span
            ><ArrowRight :size="18" /><span>03 &nbsp; Build bundle</span>
          </div>
        </main>

        <main
          v-else
          id="main-content"
          tabindex="-1"
          class="page-content workspace-page" :inert="membershipOpen" :aria-hidden="membershipOpen || undefined"
        >
          <div v-if="activeView !== 'documents' && !buildShell" class="matter-heading">
            <div class="matter-heading-copy">
              <h1>{{ currentMatter.name }}</h1>
              <p class="matter-description">
                <span>{{ currentMatter.case_number || "Draft" }}</span
                ><span class="dot-sep">·</span>{{ currentMatter.case_type }}
                <span class="dot-sep">·</span> {{ selectedProfile?.name }}
              </p>
            </div>
            <div class="matter-heading-actions">
              <button
                class="btn secondary small matter-icon-action"
                aria-label="Undo"
                :title="undoDescription"
                :aria-description="undoDescription"
                :disabled="busy || !history.can_undo"
                @click="traverseHistory('undo')"
              >
                <Undo2 :size="16" />
              </button>
              <button
                class="btn secondary small matter-icon-action"
                aria-label="Redo"
                :title="redoDescription"
                :aria-description="redoDescription"
                :disabled="busy || !history.can_redo"
                @click="traverseHistory('redo')"
              >
                <Redo2 :size="16" />
              </button>
              <details ref="matterMore" class="matter-more">
                <summary
                  class="btn secondary small matter-icon-action"
                  aria-label="More actions"
                  title="More actions"
                >
                  <Ellipsis :size="17" />
                </summary>
                <div class="matter-more-menu">
                  <button :disabled="busy" @click="backupCurrentMatter">
                    <ArrowDownToLine :size="15" /> Back up matter
                  </button>
                  <button
                    @click="
                      confirmDiscardDrafts() && void loadMatterResources()
                    "
                  >
                    <RefreshCw :size="15" /> Refresh
                  </button>
                  <button :disabled="busy" @click="removeMatter">
                    <Archive :size="15" /> Archive matter
                  </button>
                </div>
              </details>
              <button class="btn primary" @click="showView('build')">
                <FileStack :size="17" /> Build filing bundle
              </button>
            </div>
          </div>
          <div v-if="activeView !== 'documents' && !buildShell" class="matter-summary" aria-label="Matter summary">
            <span
              ><strong>{{ docs.length }}</strong> documents</span
            ><span
              ><strong>{{ totalPages }}</strong> source pages</span
            ><span
              ><strong>{{ warningChecks.length + errorChecks.length }}</strong>
              review flags</span
            ><span
              ><strong>{{ exportsList.length }}</strong> prepared packages</span
            >
          </div>
          <nav v-show="false" class="workspace-tabs" aria-label="Matter preparation">
            <button
              :class="{ active: activeView === 'overview' }"
              @click="showView('overview')"
            >
              <BriefcaseBusiness :size="16" /> Overview</button
            ><button
              :class="{ active: activeView === 'documents' }"
              @click="showView('documents')"
            >
              <FileStack :size="16" /> Documents</button
            ><button
              :class="{ active: activeView === 'index' }"
              @click="showView('index')"
            >
              <BookMarked :size="16" /> Index & bookmarks</button
            ><button
              :class="{ active: activeView === 'build' }"
              @click="showView('build')"
            >
              <Layers3 :size="16" /> Build bundle</button
            ><button
              :class="{ active: activeView === 'validation' }"
              @click="showView('validation')"
            >
              <ShieldCheck :size="16" /> Preflight checks
              <span
                v-if="warningChecks.length + errorChecks.length"
                class="count-bubble"
                >{{ warningChecks.length + errorChecks.length }}</span
              ></button
            ><button
              :class="{ active: activeView === 'storage' }"
              @click="showView('storage')"
            >
              <Archive :size="16" /> Storage
            </button>
          </nav>

          <Transition
            name="section"
            :css="!inTauri()"
            :mode="inTauri() ? undefined : 'out-in'"
            @after-enter="afterWorkspaceSectionEnter"
          >
            <section
              v-if="activeView === 'overview'"
              class="matter-overview"
              aria-label="Matter overview"
            >
              <div class="overview-lead panel">
                <div>
                  <div class="eyebrow">PREPARE YOUR BUNDLE</div>
                  <h2>
                    {{
                      docs.length
                        ? "Continue preparing this matter"
                        : "Begin with your source documents"
                    }}
                  </h2>
                  <p>
                    Review working copies, arrange the index, check the result
                    and build your filing package.
                  </p>
                </div>
                <button class="btn primary" @click="showView('documents')">
                  <FolderOpen :size="17" />
                  {{ docs.length ? "Open documents" : "Add documents" }}
                </button>
              </div>
              <section
                class="panel desktop-tools"
                aria-label="Desktop file tools"
              >
                <div>
                  <div class="eyebrow">DESKTOP FILE TOOLS</div>
                  <h3>Open, save, print and export</h3>
                  <p>
                    Work on PDF copies, autosave details, manage local storage,
                    and export review files after building a bundle.
                  </p>
                </div>
                <div class="desktop-tool-actions">
                  <button class="btn secondary" @click="showView('documents')">
                    <FileText :size="16" /> PDF tools & print</button
                  ><button class="btn secondary" @click="showView('storage')">
                    <Archive :size="16" /> Local storage</button
                  ><button class="btn secondary" @click="openExports">
                    <ArrowDownToLine :size="16" /> Office exports
                  </button>
                </div>
              </section>
              <div class="overview-steps">
                <button
                  class="panel overview-step"
                  @click="showView('documents')"
                >
                  <span class="step-number">01</span>
                  <h3>Documents</h3>
                  <p>Import, read and edit working copies.</p>
                  <strong
                    >{{ docs.length }} documents · {{ totalPages }} pages
                    <ArrowRight :size="16"
                  /></strong>
                </button>
                <button class="panel overview-step" @click="showView('index')">
                  <span class="step-number">02</span>
                  <h3>Index & bookmarks</h3>
                  <p>Arrange bundle order and review page destinations.</p>
                  <strong>Review page map <ArrowRight :size="16" /></strong>
                </button>
                <button
                  class="panel overview-step"
                  @click="showView('validation')"
                >
                  <span class="step-number">03</span>
                  <h3>Preflight checks</h3>
                  <p>Run technical checks and inspect flagged items.</p>
                  <strong>Run checks <ArrowRight :size="16" /></strong>
                </button>
                <button class="panel overview-step" @click="showView('build')">
                  <span class="step-number">04</span>
                  <h3>Build & export</h3>
                  <p>Generate, review and save the finished PDFs.</p>
                  <strong
                    >{{ exportsList.length }} prepared packages
                    <ArrowRight :size="16"
                  /></strong>
                </button>
              </div>
              <div class="overview-bottom">
                <section class="panel overview-context">
                  <h2>Case details</h2>
                  <dl>
                    <dt>Petitioner</dt>
                    <dd>{{ currentMatter.petitioner || "Not entered" }}</dd>
                    <dt>Respondent</dt>
                    <dd>{{ currentMatter.respondent || "Not entered" }}</dd>
                    <dt>Advocate</dt>
                    <dd>{{ currentMatter.advocate || "Not entered" }}</dd>
                    <dt>Court profile</dt>
                    <dd>{{ selectedProfile?.name }}</dd>
                  </dl>
                </section>
                <section class="panel overview-context">
                  <h2>Latest package</h2>
                  <template v-if="activeExport"
                    ><p>
                      {{ prettyDate(activeExport.created_at) }} ·
                      {{ activeExport.volumes }} volume(s) ·
                      {{ activeExport.pages }} pages
                    </p>
                    <p>
                      {{
                        activeExport.stale
                          ? "Inputs changed. Rebuild before using this package."
                          : "Review the generated PDF before use."
                      }}
                    </p>
                    <button class="btn secondary" @click="openExports">
                      Review exports
                    </button></template
                  >
                  <p v-else>
                    No package built yet. Start with documents and review the
                    page map before building.
                  </p>
                </section>
              </div>
            </section>
            <DocumentTable ref="documentTablePanel"
              v-else-if="activeView === 'documents' && documentTableOpen"
              :matter-id="currentMatterId!"
              :documents="docs"
              :busy="busy || isJobActive"
              :keep-groups="currentMatter.keep_groups_intact"
              @dirty-changed="tableDirty=$event"
              @modified="documentTableModified"
              @open-document="openDocumentFromTable"
              @error="issue"
              @close="closeDocumentTable"
            ><template #history><div class="table-history"><button class="icon-btn small" aria-label="Undo last matter change" :title="undoDescription" :disabled="membershipOpen || engineBlocked || busy || !history.can_undo" @click="traverseHistory('undo')"><Undo2 :size="16"/></button><button class="icon-btn small" aria-label="Redo last matter change" :title="redoDescription" :disabled="membershipOpen || engineBlocked || busy || !history.can_redo" @click="traverseHistory('redo')"><Redo2 :size="16"/></button></div></template></DocumentTable>
            <div
              v-else-if="activeView === 'documents'"
              ref="documentsWorkspace"
              class="documents-workspace"
              :class="{
                'details-open': detailsOpen && !!selectedDoc,
                'preparation-workspace': preparationVisible,
                'navigator-open': preparationListOpen,
                'reader-focus': readerFocus,
                'compact-documents': compactDocuments,
              }"
              :style="
                documentsWorkspaceHeight
                  ? { height: documentsWorkspaceHeight + 'px' }
                  : undefined
              "
            >
              <DocumentNavigator v-if="preparationVisible && preparationListOpen" :documents="docs" :selected-id="selectedDocId" :busy="engineBlocked || busy || isJobActive" @select="selectPreparationDocument" @open="openDocumentTab" @reorder="applyDocumentSort" @import="fileInput?.click()" @table="openDocumentTable" @cover="generateCover" @search-pages="openPreparationSearch" @collapse="closePreparationList"/>
              <section
                v-show="!preparationVisible && (!readerFocus || !selectedDoc) && !(compactDocuments && detailsOpen && selectedDoc)"
                class="panel docs-panel"
                @keydown.esc.stop.prevent="closeDocumentBrowser"
                :class="{ 'has-documents': docs.length > 0 }"
              >
                <div class="panel-header">
                  <button v-if="selectedDoc" class="icon-btn small close-document-browser" aria-label="Close document browser" @click="closeDocumentBrowser"><X :size="16"/></button>
                  <div>
                    <h2>Documents <span class="document-count">{{ docs.length }}</span></h2>
                    <span>Drag or use ↑/↓ to reorder</span>
                  </div>
                  <div class="panel-header-actions">
                    <button
                      class="text-tool"
                      :disabled="!docs.length"
                      @click="openDocumentTable"
                    >
                      Open table</button
                    ><button
                      class="icon-btn"
                      title="Generate draft cover sheet"
                      :disabled="busy"
                      @click="generateCover"
                    >
                      <FilePlus2 :size="19" />
                    </button>
                  </div>
                </div>
                <div class="docs-search">
                  <Search :size="16" /><input
                    v-model="searching"
                    placeholder="Search documents..."
                    aria-label="Search documents"
                  />
                </div>
                <div class="docs-search">
                  <Search :size="16" /><input
                    v-model="pageQuery"
                    placeholder="Find text on pages..."
                    aria-label="Find text on pages"
                  />
                </div>
                <div
                  v-if="pageQuery.trim().length >= 2"
                  class="page-search-results"
                  aria-live="polite"
                >
                  <small v-if="pageSearchBusy">Searching working PDFs…</small>
                  <small v-else-if="pageSearchError" class="error-msg">{{
                    pageSearchError
                  }}</small>
                  <template v-else-if="pageSearch">
                    <small
                      >{{ pageSearch.matches.length }} matching page(s){{
                        pageSearch.truncated ? " · first 100 shown" : ""
                      }}</small
                    >
                    <button
                      v-for="match in pageSearch.matches"
                      :key="match.document_id + '-' + match.page"
                      class="page-search-hit"
                      @click="openSearchMatch(match.document_id, match.page)"
                    >
                      <strong>{{ match.title }} · page {{ match.page }}</strong
                      ><span
                        >{{
                          highlightedSnippet(match.snippet, pageQuery.trim())
                            .before
                        }}<mark>{{
                          highlightedSnippet(match.snippet, pageQuery.trim())
                            .match
                        }}</mark
                        >{{
                          highlightedSnippet(match.snippet, pageQuery.trim())
                            .after
                        }}</span
                      >
                    </button>
                  </template>
                </div>
                <VueDraggable
                  v-if="filteredDocs.length"
                  :model-value="filteredDocs"
                  class="docs-list"
                  item-key="id"
                  handle=".drag-handle"
                  direction="vertical"
                  :disabled="busy || !!searching.trim()"
                  :animation="220"
                  easing="cubic-bezier(.2,.7,.2,1)"
                  :force-fallback="true"
                  :fallback-tolerance="5"
                  :fallback-on-body="true"
                  ghost-class="sortable-ghost"
                  chosen-class="sortable-chosen"
                  drag-class="sortable-drag"
                  fallback-class="sortable-fallback"
                  @update:model-value="applyDocumentSort"
                >
                  <div
                    v-for="doc in filteredDocs"
                    :key="doc.id"
                    class="doc-item"
                    :data-document-id="doc.id"
                    :class="{ selected: selectedDocId === doc.id }"
                    role="button"
                    tabindex="0"
                    :aria-label="`Open ${doc.title}, ${doc.page_count} ${doc.page_count === 1 ? 'page' : 'pages'}`"
                    @click="selectDocument(doc.id)"
                    @keydown.enter.prevent="selectDocument(doc.id)"
                    @keydown.space.prevent="selectDocument(doc.id)"
                  >
                    <span
                      class="grip drag-handle"
                      role="button"
                      :tabindex="busy || !!searching.trim() ? -1 : 0"
                      :aria-label="
                        'Reorder document ' +
                        doc.title +
                        '. Press Up or Down arrow keys.'
                      "
                      @click.stop
                      @keydown.up.prevent="
                        moveDocumentFromHandle(doc.id, -1, true)
                      "
                      @keydown.down.prevent="
                        moveDocumentFromHandle(doc.id, 1, true)
                      "
                      ><GripVertical :size="16"
                    /></span>
                    <DocumentThumbnail
                      :document-id="doc.id"
                      :revision="doc.revision"
                    />
                    <div class="doc-info">
                      <strong :title="doc.title">{{ doc.title }}</strong
                      ><span
                        ><span v-if="doc.label" class="doc-label"
                          >{{ doc.label }} · </span
                        >Part {{ doc.part }} · {{ doc.page_count }}
                        {{ doc.page_count === 1 ? "page" : "pages" }}</span
                      >
                    </div>
                  </div>
                </VueDraggable>
                <div v-else-if="docs.length" class="no-matching">
                  No matching documents
                </div>
                <div v-else class="no-documents">
                  <FileStack :size="30" /><strong>No documents added</strong
                  ><small>Import a PDF or scanned image to begin.</small>
                </div>
                <div v-if="archivedDocs.length" class="docs-help">
                  <Archive :size="14" /> Archived documents
                </div>
                <div
                  v-for="doc in archivedDocs"
                  :key="doc.id"
                  class="export-file"
                >
                  <FileText :size="16" />
                  <div>
                    <strong>{{ doc.title }}</strong
                    ><small>{{ doc.original_name }}</small>
                  </div>
                  <button
                    class="text-tool"
                    @click="restoreArchivedDocument(doc.id)"
                  >
                    Restore
                  </button>
                </div>
                <div
                  class="upload-zone"
                  :class="{ over: hoverUpload }"
                  @dragover.prevent="hoverUpload = true"
                  @dragleave="hoverUpload = false"
                  @drop.prevent="onDropUpload"
                >
                  <input
                    ref="fileInput"
                    type="file"
                    accept=".pdf,.jpg,.jpeg,.png"
                    multiple
                    hidden
                    @change="
                      (e) =>
                        onUpload((e.target as HTMLInputElement).files || [])
                    "
                  /><UploadCloud :size="25" /><strong>Drop files here</strong
                  ><span>PDF, JPG, PNG · up to 80 MB/file</span
                  ><button
                    class="btn secondary small"
                    :disabled="busy"
                    @click="fileInput?.click()"
                  >
                    {{ busy ? "Importing…" : "Browse files" }}
                  </button>
                </div>
                <div class="docs-total">{{ docs.length }} documents · {{ totalPages }} source pages</div>
                <div class="docs-help">
                  <LockKeyhole :size="14" /> Original files are never
                  overwritten.
                </div>
              </section>

              <section class="panel preview-panel">
                <PdfViewer
                  ref="pdfViewer"
                  :key="currentMatterId || 'none'"
                  :preparation="preparationVisible"
                  :expanded="editorExpanded"
                  @toggle-expanded="toggleEditorExpanded"
                  :focus-mode="readerFocus"
                  @toggle-focus="readerFocus = !readerFocus"
                  :document="selectedDoc"
                  :documents="docs"
                  :busy="busy || isJobActive"
                  :disabled="engineBlocked || busy || isJobActive || documentDirty"
                  :can-undo="history.can_undo"
                  :can-redo="history.can_redo"
                  :undo-label="history.undo_label"
                  :redo-label="history.redo_label"
                  :details-open="detailsOpen"
                  :details-dirty="documentDirty"
                  :bookmark-refresh-key="previewRefreshKey"
                  :page-target="pageTarget"
                  :artifact-options="artifactOptions"
                  @artifact="switchArtifact"
                  @browse-documents="showPreparation"
                  @open-editor="openEditor"
                  @show-document-list="preparationListOpen=!preparationListOpen; compactDocuments && preparationListOpen && (detailsOpen=false)"
                  @save-details="saveDocument()"
                  @retry-reader="retryWorkingReader"
                  @modified="refreshDocument"
                  @undo="traverseHistory('undo')"
                  @redo="traverseHistory('redo')"
                  @error="issue"
                  @page-tools="pageToolsOpen = true"
                  @open-bookmarks="showView('index')"
                  @toggle-details="toggleDocumentDetails"
                  @ocr="startOcr"
                  @archive-document="deleteDocument"
                  @open-internal-link="openSearchMatch"
                  @open-external-link="openReviewedExternalLink"
                  @print-working="beginWorkingPrint"
                />
              </section>

              <Transition name="inspector">
                <section
                  v-if="detailsOpen && selectedDoc"
                  id="document-details-panel"
                  ref="documentDetailsPanel"
                  @keydown.esc.stop.prevent="closeDocumentDetails"
                  class="panel inspector-panel"
                  aria-label="Document details"
                >
                  <div class="panel-header">
                    <div>
                      <h2>Document details</h2>
                      <span>Metadata for collation</span>
                    </div>
                    <button
                      class="icon-btn"
                      aria-label="Close document details"
                      @click="closeDocumentDetails"
                    >
                      <X :size="18" />
                    </button>
                  </div>
                  <div class="inspector-content">
                    <div v-if="preparationVisible" class="preparation-metadata-state" :class="{'dirty':documentDirty}" role="status"><CircleAlert v-if="documentDirty" :size="14"/><CheckCircle2 v-else :size="14"/>{{documentAutosaveState==='failed'?'Save failed — retry below':documentAutosaveState==='saving'?'Saving…':documentDirty?'Unsaved changes':'Saved locally'}}</div>
                    <div class="inspector-heading">
                      <div class="doc-icon big"><FileText :size="22" /></div>
                      <div>
                        <strong>{{ selectedDoc.original_name }}</strong
                        ><small
                          >{{ selectedDoc.page_count }} pages ·
                          {{
                            selectedDoc.ocr_status.split("_").join(" ")
                          }}</small
                        >
                      </div>
                    </div>
                    <div
                      v-if="selectedOcrRuns.length"
                      class="notice subtle ocr-provenance"
                      role="note"
                    >
                      <ScanText :size="18" /><span
                        ><strong>OCR provenance · unverified text</strong>
                        <small
                          v-for="run in selectedOcrRuns"
                          :key="run.applied_revision"
                        >
                          Revision {{ run.applied_revision }}:
                          {{ run.language.split("+").join(" + ") }} ·
                          {{ run.pages_ocr_applied }}
                          {{ run.pages_ocr_applied === 1 ? "page" : "pages" }} ·
                          PyMuPDF {{ run.pymupdf_version
                          }}<template v-if="run.tesseract_cli_version">
                            · {{ run.tesseract_cli_version }} (CLI)</template
                          ><br />
                          <span
                            v-for="(hash, language) in run.model_sha256"
                            :key="language"
                            :title="hash"
                            >{{ language }} model SHA-256: {{ hash }}</span
                          >
                        </small>
                        <small
                          >These identifiers describe the OCR software and model
                          files. Recognized text still needs review against the
                          page.</small
                        >
                      </span>
                    </div>
                    <label class="field"
                      >Display title<input
                        v-model="docEditor.title"
                        maxlength="250"
                    /></label>
                    <label class="field"
                      >Document date<input
                        v-model="docEditor.document_date"
                        type="date"
                        @input="docEditor.date_confirmed = true"
                      /><span
                        v-if="
                          selectedDoc.date_status === 'ambiguous' &&
                          !docEditor.document_date
                        "
                        class="hint"
                        >Ambiguous {{ selectedDoc.date_source }} text:
                        {{ selectedDoc.date_note }}. Enter and confirm a date
                        only after checking the document.</span
                      ><span v-else class="hint"
                        >Unknown is preserved when blank. A file timestamp is
                        never treated as the document date.</span
                      ></label
                    >
                    <label v-if="docEditor.document_date" class="checkbox-line"
                      ><input
                        v-model="docEditor.date_confirmed"
                        type="checkbox"
                      /><span
                        ><strong>Confirmed from the document</strong
                        ><small
                          v-if="
                            selectedDoc.date_status === 'suggested' &&
                            !docEditor.date_confirmed
                          "
                          >Current value is an unreviewed
                          {{ selectedDoc.date_source }} suggestion.</small
                        ></span
                      ></label
                    >
                    <label class="field"
                      >Document type<select v-model="docEditor.kind">
                        <option
                          v-for="kind in documentKindOptions"
                          :key="kind"
                          :value="kind"
                        >
                          {{ kind[0].toUpperCase() + kind.slice(1) }}
                        </option>
                      </select></label
                    >
                    <div class="field-row">
                      <label class="field"
                        >Part<select v-model.number="docEditor.part">
                          <option v-for="part in 4" :value="part">
                            Part {{ part }}
                          </option>
                        </select></label
                      ><label class="field"
                        >Label<input
                          v-model="docEditor.label"
                          placeholder="P-1"
                          maxlength="60"
                      /></label>
                    </div>
                    <details class="metadata-signature-review" :open="!preparationVisible"><summary>Signature review</summary>
                    <div
                      :class="[
                        'notice',
                        [
                          'certificate_signature_detected',
                          'inspection_inconclusive',
                          'visible_signature_only',
                        ].includes(selectedDoc.signature_status)
                          ? 'amber'
                          : 'subtle',
                      ]"
                    >
                      <ShieldAlert :size="18" /><span
                        ><strong>{{
                          signatureStatusLabel(selectedDoc.signature_status)
                        }}</strong
                        ><small
                          >{{ selectedDoc.signature_inspection_note }} This does
                          not validate a certificate, signer, scanned mark or
                          current signature coverage.</small
                        ></span
                      >
                    </div>
                    <label class="field"
                      >Visible/scanned signature review
                      <select v-model="docEditor.visible_signature_status">
                        <option value="unreviewed">Not reviewed</option>
                        <option value="present">
                          Visible or scanned signature appears present
                        </option>
                        <option value="absent">
                          No visible or scanned signature found in review
                        </option>
                      </select>
                      <span class="hint"
                        >This is a reviewer classification only. It is separate
                        from PDF certificate indicators and does not verify a
                        signature.</span
                      >
                    </label>
                    </details>
                    <label class="field"
                      >Keep-together group<input
                        v-model="docEditor.group_key"
                        placeholder="Optional: annexures-a"
                      /><span class="hint"
                        >Adjacent documents with the same group stay together
                        when enabled.</span
                      ></label
                    >
                    <p
                      v-if="!preparationVisible || documentDirty || documentAutosaveState==='failed'"
                      role="status"
                      class="hint metadata-status"
                      :class="{
                        'is-dirty': documentDirty,
                        'is-saving': documentAutosaveState === 'saving',
                        'is-failed': documentAutosaveState === 'failed',
                      }"
                    >
                      {{
                        autosaveHeld && documentDirty
                          ? "Unsaved draft retained · use Save after recovery"
                          : documentAutosaveState === "failed"
                          ? "Autosave failed. Retry Save."
                          : documentAutosaveState === "saving"
                            ? "Saving document details…"
                            : documentDirty
                              ? "Unsaved document changes · autosaves after a short pause"
                              : "Document details saved"
                      }}
                    </p>
                  </div>
                  <div class="inspector-save-footer">
                    <button
                      class="btn primary block"
                      :disabled="
                        engineBlocked || busy || !docEditor.title.trim() || !documentDirty
                      "
                      @click="saveDocument()"
                    >
                      <Check :size="16" /> Save document details
                    </button>
                  </div>
                </section>
              </Transition>
            </div>

            <BundleStructure v-else-if="activeView === 'index'" :key="currentMatterId!" ref="outputPresetsPanel"
              :matter-id="currentMatterId!" :documents="docs" :refresh-key="previewRefreshKey" :initial-mode="structureMode" :bookmark-preset="bookmarkPreset"
              :disabled="engineBlocked || busy || isJobActive || documentDirty || settingsDirty"
              @dirty-changed="formattingDirty=$event" @modified="structureModified" @membership="openMembership"
              @build="showView('build')" @policy="openStructurePolicy">
              <template #order>              <section
                v-if="docs.length"
                class="panel index-order-panel"
                aria-label="Index document order"
              >
                <div class="panel-header">
                  <div>
                    <h2>Document order</h2>
                    <span
                      >Drag to reorder · {{includedDocuments.length}} included documents</span
                    >
                  </div>
                </div>
                <VueDraggable
                  :model-value="includedDocuments"
                  class="index-order-list"
                  item-key="id"
                  handle=".drag-handle"
                  :disabled="busy"
                  :animation="160"
                  :force-fallback="true"
                  :fallback-tolerance="5"
                  :fallback-on-body="true"
                  ghost-class="sortable-ghost"
                  chosen-class="sortable-chosen"
                  drag-class="sortable-drag"
                  fallback-class="sortable-fallback"
                  @update:model-value="applyIncludedDocumentSort"
                  ><div
                    v-for="(doc, index) in includedDocuments"
                    :key="doc.id"
                    class="index-order-item"
                    :data-document-id="doc.id"
                  >
                    <h3 v-if="index===0||includedDocuments[index-1].part!==doc.part" class="index-part-heading">⌄ Part {{doc.part}}</h3>
                    <div class="index-order-row" :class="{selected:doc.id===selectedDocId}"><span
                      class="grip drag-handle"
                      role="button"
                      :tabindex="busy ? -1 : 0"
                      :aria-label="
                        'Reorder document ' +
                        doc.title +
                        '. Press Up or Down arrow keys.'
                      "
                      @click.stop
                      @keydown.up.prevent="moveIncludedDocument(doc.id, -1)"
                      @keydown.down.prevent="moveIncludedDocument(doc.id, 1)"
                      ><GripVertical :size="16" /></span
                    ><strong
                      >{{ doc.label ? doc.label + " · " : ""
                      }}{{ doc.title }}</strong
                    ><span>{{ doc.page_count }}</span></div>
                  </div></VueDraggable
                >
              </section>
</template>
              <template #map="{recalculateKey}">              <BundlePreview
                  :inputs-pending="engineBlocked || busy || isJobActive || checksLoading || documentDirty || settingsDirty || formattingDirty"
                :matter-id="currentMatterId!"
                :documents="docs" :recalculate-key="recalculateKey"
                :max-pages="matterEditor.max_pages_per_volume"
                :keep-groups="matterEditor.keep_groups_intact"
                :refresh-key="previewRefreshKey"
                @open-document="openPreviewDocument"
                @bookmark-document="prepareIndexBookmark"
              />
</template>
              <template #import>              <IndexImport
                :matter-id="currentMatterId!"
                :documents="docs"
                :disabled="engineBlocked || busy || isJobActive"
                @modified="loadMatterResources"
              />
</template>

            </BundleStructure>

            <BuildWorkspace v-else-if="activeView === 'build'" :key="currentMatterId!" ref="buildWorkspace"
              :matter-id="currentMatterId!" :documents="docs" :artifacts="exportsList" :job="activeJob"
              :busy="isJobActive" :blocked="engineBlocked || busy || buildSubmitting" :cancel-pending="cancelPending"
              :max-pages="matterEditor.max_pages_per_volume" :keep-groups="matterEditor.keep_groups_intact"
              :refresh-key="previewRefreshKey" :dirty="settingsDirty || formattingDirty" :profile="selectedProfile"
              :checks-text="checksLoading ? 'Preflight loading' : checksError || `${errorChecks.length} preflight blockers · ${warningChecks.length} warnings to review`"
              @build="startBuild" @cancel="cancelJob" @retry="retryInterrupted" @review="reviewGeneratedPdf"
              @finding="openFinding" @export="exportDialogArtifact=$event" @documents="showPreparation" @checks="showView('validation')"
              @configure="openStructure" @membership="openMembership" @storage="showView('storage')" @backup="backupCurrentMatter">
              <template #settings>                <OutputPresets
                  :key="currentMatterId!"
                  ref="outputPresetsPanel"
                  :matter-id="currentMatterId!"
                  :max-pages="matterEditor.max_pages_per_volume"
                  :keep-groups="matterEditor.keep_groups_intact"
                  :settings-dirty="settingsDirty"
                  :disabled="isJobActive"
                  :refresh-key="previewRefreshKey"
                  :documents="docs"
                  @apply-requested="applyOutputPreset" @configure="openStructure"
                  @dirty-changed="formattingDirty = $event"
                  @modified="loadMatterResources"
                  @open-document="openPreviewDocument"
                />
</template>
              <template #preview>                <BundlePreview
                  :inputs-pending="engineBlocked || busy || isJobActive || checksLoading || documentDirty || settingsDirty || formattingDirty"
                  :matter-id="currentMatterId!"
                  :max-pages="matterEditor.max_pages_per_volume"
                  :keep-groups="matterEditor.keep_groups_intact"
                  :refresh-key="previewRefreshKey"
                  @open-document="openPreviewDocument"
                />
</template>
              <template #volumes><div class="build-volume-controls"><label class="field">Maximum pages per volume<input v-model.number="matterEditor.max_pages_per_volume" type="number" min="2" max="2000" :disabled="isJobActive"/></label><label class="check-row"><input v-model="matterEditor.keep_groups_intact" type="checkbox" :disabled="isJobActive"/>Keep document groups together</label><button class="btn primary" :disabled="engineBlocked || isJobActive || !settingsDirty" @click="savePolicy()">Save bundle settings</button><p role="status">{{settingsAutosaveState==='failed'?'Settings save failed — retry Save':settingsDirty?'Unsaved bundle settings':'Bundle settings saved'}}</p></div>                <BundlePreview
                  :inputs-pending="engineBlocked || busy || isJobActive || checksLoading || documentDirty || settingsDirty || formattingDirty"
                  :matter-id="currentMatterId!"
                  :max-pages="matterEditor.max_pages_per_volume"
                  :keep-groups="matterEditor.keep_groups_intact"
                  :refresh-key="previewRefreshKey"
                  @open-document="openPreviewDocument"
                />
</template>
              <template #output-tools="{artifact}">                    <div
                      v-for="file in artifact.files"
                      :key="file.name"
                      class="export-file"
                    >
                      <FileText :size="18" />
                      <div>
                        <strong>{{ file.name }}</strong
                        ><small>{{ bytesLabel(file.bytes) }}</small>
                      </div>
                      <button
                        v-if="file.name.endsWith('.pdf')"
                        class="text-tool"
                        :aria-label="'Review ' + file.name"
                        @click="
                          reviewGeneratedPdf(artifact.id, file.name)
                        "
                      >
                        Review PDF</button
                      ><button
                        class="icon-btn"
                        :disabled="savingExport"
                        :title="
                          inTauri()
                            ? 'Save ' + file.name
                            : 'Download ' + file.name
                        "
                        :aria-label="
                          inTauri()
                            ? 'Save ' + file.name
                            : 'Download ' + file.name
                        "
                        @click="download(artifact.id, file.name)"
                      >
                        <ArrowDownToLine :size="18" /></button
                      ><button
                        v-if="inTauri()"
                        class="text-tool"
                        :disabled="savingExport"
                        :title="'Save ' + file.name + ' as'"
                        @click="download(artifact.id, file.name, true)"
                      >
                        Save As</button
                      ><button
                        v-if="file.name.endsWith('.pdf')"
                        class="text-tool"
                        :aria-label="'Print ' + file.name"
                        @click="beginPrint(artifact.id, file.name)"
                      >
                        Print…</button
                      ><button
                        v-if="file.name.endsWith('.pdf')"
                        class="text-tool"
                        :disabled="savingExport"
                        :aria-label="'Save Word review draft from ' + file.name"
                        @click="
                          saveOfficeReview(
                            artifact.id,
                            file.name,
                            'docx',
                          )
                        "
                      >
                        Word draft</button
                      ><button
                        v-if="file.name.endsWith('.pdf')"
                        class="text-tool"
                        :disabled="savingExport"
                        :aria-label="
                          'Save PowerPoint review draft from ' + file.name
                        "
                        @click="
                          saveOfficeReview(
                            artifact.id,
                            file.name,
                            'pptx',
                          )
                        "
                      >
                        PPT draft</button
                      ><button
                        v-if="file.name === 'index.csv'"
                        class="text-tool"
                        :disabled="savingExport"
                        aria-label="Save Excel index workbook"
                        @click="
                          saveOfficeReview(
                            artifact.id,
                            file.name,
                            'xlsx',
                          )
                        "
                      >
                        Excel index</button
                      ><button
                        v-if="
                          file.name.endsWith('.pdf') &&
                          !selectedProfile?.disallow_pdf_encryption
                        "
                        class="text-tool"
                        :disabled="savingExport"
                        :aria-label="'Save password-protected ' + file.name"
                        @click="
                          beginProtectedExport(artifact.id, file.name)
                        "
                      >
                        Protect PDF…
                      </button>
                    </div>
<p>Office files are editable text review drafts; layout, images and verified signatures are not reproduced.</p></template>
            </BuildWorkspace>

            <section
              v-else-if="activeView === 'storage'"
              class="matter-storage"
              aria-label="Matter storage"
            >
              <BuildWorkspace :key="currentMatterId!" storage :matter-id="currentMatterId!" :documents="docs" :artifacts="exportsList" :job="activeJob" :busy="isJobActive" :blocked="engineBlocked" :cancel-pending="cancelPending" :max-pages="matterEditor.max_pages_per_volume" :keep-groups="matterEditor.keep_groups_intact" :refresh-key="previewRefreshKey" :dirty="settingsDirty" checks-text="" @review="reviewGeneratedPdf" @finding="openFinding" @export="exportDialogArtifact=$event" @build="showView('build')" @storage="loadMatterResources" @backup="backupCurrentMatter"/>
              <details class="storage-maintenance"><summary>Storage usage, originals, revisions and backup tools</summary><div class="panel">
                <div class="panel-header">
                  <div>
                    <h2>Local matter storage</h2>
                    <span
                      >Files are stored in this device’s app workspace. Packaged
                      desktop storage uses the local vault.</span
                    >
                  </div>
                  <button
                    class="btn secondary small"
                    @click="loadMatterResources"
                  >
                    <RefreshCw :size="15" /> Refresh
                  </button>
                </div>
                <div class="storage-metrics">
                  <div>
                    <span>Total</span
                    ><strong>{{ bytesLabel(storageUsage.bytes) }}</strong>
                  </div>
                  <div>
                    <span>Imported originals</span
                    ><strong>{{
                      bytesLabel(storageUsage.by_category.originals)
                    }}</strong>
                  </div>
                  <div>
                    <span>Working revisions</span
                    ><strong>{{
                      bytesLabel(storageUsage.by_category.working)
                    }}</strong>
                  </div>
                  <div>
                    <span>Generated exports</span
                    ><strong>{{
                      bytesLabel(storageUsage.by_category.exports)
                    }}</strong>
                  </div>
                  <div>
                    <span>Other data</span
                    ><strong>{{
                      bytesLabel(storageUsage.by_category.other)
                    }}</strong>
                  </div>
                </div>
                <p class="hint">
                  Archiving keeps originals, working revisions and earlier
                  exports. Back up the matter to create a portable copy.
                </p>
                <button
                  class="btn secondary"
                  :disabled="busy"
                  @click="backupCurrentMatter"
                >
                  Back up matter
                </button>
              </div>
              <div class="panel">
                <div class="panel-header">
                  <div>
                    <h2>Documents</h2>
                    <span
                      >{{ docs.length }} active ·
                      {{ archivedDocs.length }} archived</span
                    >
                  </div>
                </div>
                <div v-for="doc in docs" :key="doc.id" class="storage-item">
                  <FileText :size="18" />
                  <div>
                    <strong>{{ doc.title }}</strong
                    ><small
                      >Original: {{ doc.original_name }} · working revision
                      {{ doc.revision }} · {{ doc.page_count }} pages</small
                    >
                  </div>
                  <button class="text-tool" @click="openDocumentTab(doc.id)">
                    Open
                  </button>
                </div>
                <div
                  v-for="doc in archivedDocs"
                  :key="doc.id"
                  class="storage-item"
                >
                  <Archive :size="18" />
                  <div>
                    <strong>{{ doc.title }}</strong
                    ><small
                      >Archived · original retained:
                      {{ doc.original_name }}</small
                    >
                  </div>
                  <button
                    class="text-tool"
                    @click="restoreArchivedDocument(doc.id)"
                  >
                    Restore
                  </button>
                </div>
                <p v-if="!docs.length && !archivedDocs.length" class="hint">
                  No documents in this matter.
                </p>
              </div>
              <div class="panel">
                <div class="panel-header">
                  <div>
                    <h2>Previous builds</h2>
                    <span
                      >Immutable exports remain available even when inputs
                      change.</span
                    >
                  </div>
                </div>
                <div
                  v-for="item in exportsList"
                  :key="item.id"
                  class="storage-item"
                >
                  <FileCheck2 :size="18" />
                  <div>
                    <strong
                      >{{ prettyDate(item.created_at) }} ·
                      {{ item.id.slice(0, 8) }}</strong
                    ><small
                      >{{ item.files.length }} files ·
                      {{
                        item.stale
                          ? "Outdated; review before use"
                          : "Current inputs"
                      }}</small
                    >
                  </div>
                  <button
                    class="text-tool"
                    @click="
                      comparisonExportId = item.id;
                      openExports();
                    "
                  >
                    View files
                  </button>
                </div>
                <p v-if="!exportsList.length" class="hint">
                  No completed builds yet.
                </p>
              </div>
            </details></section>

            <PreflightWorkspace v-else-if="activeView === 'validation'" :checks="checks" :loading="checksLoading" :error="checksError" :pending="documentDirty || settingsDirty || formattingDirty || busy || isJobActive" :blocked="engineBlocked" v-model:filter="preflightFilter" @run="validate" @finding="openFinding($event)" @build="showView('build')">
              <FilingReview :matter-id="currentMatterId!" :refresh-key="previewRefreshKey" :disabled="engineBlocked || busy || isJobActive" @modified="loadMatterResources"/>
            </PreflightWorkspace>
          </Transition>
        </main>
      </Transition>
    </div>
    <footer class="application-status"><button v-if="currentMatter && !membershipOpen" class="membership-status-entry" @click="openMembership" :title="inclusionSummary">Bundle membership</button><span v-if="membershipOpen || activeView==='index'" class="membership-status-step">03 / 06 &nbsp; Prepare index</span><span><HardDrive :size="13"/> {{engineBlocked ? 'Engine disconnected' : membershipOpen ? 'On this computer' : 'Processing locally'}}</span><span v-if="selectedDoc && activeView==='documents' && !membershipOpen"><template v-if="reviewingPdf">{{reviewingPdf.originalId ? 'Imported original preserved · Read only' : 'Generated output · Read only'}}</template><template v-else>Working revision {{selectedDoc.revision}} · {{selectedDoc.page_count}} source pages</template></span><button v-if="currentMatter" class="text-tool" @click="membershipOpen ? leaveMembership() : showPreparation()">{{membershipOpen ? 'Return to caller' : 'Back to preparation'}}</button></footer>
    <dialog
      v-if="selectedDoc && pageToolsOpen"
      ref="pageToolsDialog"
      class="page-tools-dialog"
      aria-labelledby="page-tools-title"
      @keydown.stop
      @close="pageToolsOpen = false"
      @click.self="pageToolsOpen = false"
    >
      <div class="page-tools-dialog-head">
        <div>
          <h2 id="page-tools-title">Page tools</h2>
          <p>
            {{ selectedDoc.title }} · {{ selectedDoc.page_count }} pages ·
            Choose what you want to do below
          </p>
        </div>
        <button
          class="icon-btn"
          aria-label="Close page tools"
          @click="pageToolsOpen = false"
        >
          <X :size="18" />
        </button>
      </div>
      <p v-if="documentDirty" class="notice amber">
        Save document details before editing pages.
      </p>
      <PageTools
        :document="selectedDoc"
        :documents="docs"
        :disabled="engineBlocked || busy || isJobActive || documentDirty"
        :job="activeJob"
        expanded
        @modified="refreshDocument"
        @job-started="watchJob"
        @cancel="cancelJob"
      />
    </dialog>
    <OutputChecksDialog v-if="outputChecksArtifact && currentMatterId" :key="outputChecksArtifact.id" :matter-id="currentMatterId" :artifact="outputChecksArtifact" @close="outputChecksArtifact=null" @changed="previewRefreshKey++" @review="reviewGeneratedPdf(outputChecksArtifact.id,'combined.pdf');outputChecksArtifact=null" @finding="openFinding($event,outputChecksArtifact.id);outputChecksArtifact=null"/>
    <BundleExportDialog v-if="exportDialogArtifact && currentMatterId" :matter-id="currentMatterId" :artifact="exportDialogArtifact" @close="exportDialogArtifact=null"/>
    <ExportPdfReview
      v-if="reviewingPdf && currentMatterId"
      :matter-id="currentMatterId"
      :export-id="reviewingPdf.exportId"
      :filename="reviewingPdf.filename"
      :original-id="reviewingPdf.originalId"
      :artifact-options="selectedDoc ? artifactOptions : undefined"
      :manifest="exportsList.find(item=>item.id===reviewingPdf!.exportId)"
      @source="openGeneratedSource"
      @checks="outputChecksArtifact=exportsList.find(item=>item.id===reviewingPdf!.exportId)||null"
      @export="exportDialogArtifact=exportsList.find(item=>item.id===reviewingPdf!.exportId)||null"
      @rebuild="reviewingPdf=null;showView('build');nextTick(()=>buildWorkspace?.showSettings())"
      @artifact="switchArtifact"
      @retry-engine="retryEngine"
      :stale="
        exportsList.find((item) => item.id === reviewingPdf!.exportId)?.stale
      "
      @open-external-link="openReviewedExternalLink"
      @close="reviewingPdf = null"
    />
    <div v-if="printRequest" class="vault-restore-overlay" role="presentation">
      <section
        class="vault-card print-card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="print-export-title"
      >
        <h2 id="print-export-title">Print {{ printRequest.filename }}</h2>
        <p>
          Choose PDF pages, paper orientation and scaling. The print copy is
          temporary; this build and its page map stay unchanged.
        </p>
        <form class="print-options" @submit.prevent="preparePrint">
          <label
            >Pages<input
              v-model="printPages"
              aria-label="Print pages"
              placeholder="all or 1,3-5"
          /></label>
          <label
            >Paper<select
              v-model="printOrientation"
              aria-label="Print paper orientation"
            >
              <option value="source">Source page size</option>
              <option value="portrait">A4 portrait</option>
              <option value="landscape">A4 landscape</option>
            </select></label
          >
          <label
            >Scaling<select v-model="printScale" aria-label="Print scaling">
              <option value="fit">Fit to paper</option>
              <option value="actual">Actual PDF size, centered</option>
            </select></label
          >
          <button class="btn secondary" type="submit" :disabled="printBusy">
            {{ printBusy ? "Preparing…" : "Prepare print copy" }}
          </button>
        </form>
        <p v-if="printError" class="notice amber" role="alert">
          {{ printError }}
        </p>
        <iframe
          v-if="printPreviewUrl"
          ref="printFrame"
          class="print-preview-frame"
          :src="printPreviewUrl"
          title="Print copy preview"
        ></iframe>
        <div class="print-actions">
          <button
            class="btn primary"
            :disabled="!printPreviewUrl || printBusy"
            @click="openPrintDialog"
          >
            Open print dialog</button
          ><button class="btn secondary" @click="closePrint">Close</button>
        </div>
      </section>
    </div>
    <div
      v-if="pdfImportRequest"
      class="vault-restore-overlay"
      role="presentation"
    >
      <section
        class="vault-card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="pdf-import-password-title"
      >
        <h2 id="pdf-import-password-title">Open protected PDF</h2>
        <p>
          {{ pdfImportRequest.name }} requires a password. The encrypted
          original will be preserved.
        </p>
        <p v-if="pdfImportRequest.error" class="notice amber" role="alert">
          {{ pdfImportRequest.error }}
        </p>
        <form @submit.prevent="finishPdfImportPassword(pdfImportPassword)">
          <PasswordInput
            v-model="pdfImportPassword"
            label="PDF password"
            autocomplete="off"
          />
          <button class="btn primary" type="submit">Open and import</button>
          <button
            class="btn secondary"
            type="button"
            @click="finishPdfImportPassword(null)"
          >
            Cancel import
          </button>
        </form>
      </section>
    </div>
    <div
      v-if="protectedExportRequest"
      class="vault-restore-overlay"
      role="presentation"
    >
      <section
        class="vault-card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="pdf-export-password-title"
      >
        <h2 id="pdf-export-password-title">Save protected PDF</h2>
        <p>
          Choose a password for {{ protectedExportRequest.filename }}. You will
          need it to reopen the saved PDF.
        </p>
        <p v-if="protectedExportError" class="notice amber" role="alert">
          {{ protectedExportError }}
        </p>
        <form @submit.prevent="saveProtectedExport">
          <PasswordInput
            v-model="protectedExportPassword"
            label="PDF password"
            autocomplete="new-password"
          />
          <PasswordInput
            v-model="protectedExportConfirm"
            label="Confirm PDF password"
            autocomplete="new-password"
          />
          <button class="btn primary" type="submit" :disabled="savingExport">
            {{ savingExport ? "Saving…" : "Save protected PDF" }}
          </button>
          <button
            class="btn secondary"
            type="button"
            :disabled="savingExport"
            @click="cancelProtectedExport"
          >
            Cancel
          </button>
        </form>
      </section>
    </div>
    <div
      v-if="protectedRestoreFile"
      class="vault-restore-overlay"
      role="presentation"
    >
      <section
        class="vault-card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="restore-vault-title"
      >
        <h2 id="restore-vault-title">Unlock protected backup</h2>
        <p>
          This backup uses a different vault key. Enter the password or recovery
          key that protected it.
        </p>
        <p v-if="protectedRestoreError" class="notice amber" role="alert">
          {{ protectedRestoreError }}
        </p>
        <form @submit.prevent="submitProtectedRestore">
          <label v-if="protectedRestoreRecovery"
            >Recovery key<input
              v-model="protectedRestoreCredential"
              type="text"
              autocomplete="off"
              required
          /></label>
          <PasswordInput
            v-else
            v-model="protectedRestoreCredential"
            label="Backup password"
            autocomplete="off"
          />
          <label class="vault-choice"
            ><input v-model="protectedRestoreRecovery" type="checkbox" /> Use
            recovery key</label
          >
          <button class="btn primary" type="submit" :disabled="busy">
            Restore backup
          </button>
          <button
            class="btn secondary"
            type="button"
            @click="
              protectedRestoreFile = null;
              protectedRestoreCredential = '';
              protectedRestoreError = '';
            "
          >
            Cancel
          </button>
        </form>
      </section>
    </div>
    <Transition name="modal"
      ><MatterForm
        v-if="modalOpen"
        :profiles="profileList"
        :working="creating"
        @close="modalOpen = false"
        @create="createMatter"
    /></Transition>
    <Transition name="toast"
      ><div
        v-if="toast"
        :class="['toast', toast.kind]"
        :role="toast.kind === 'error' ? 'alert' : 'status'"
      >
        <CheckCircle2 v-if="toast.kind === 'success'" :size="19" /><CircleAlert
          v-else-if="toast.kind === 'error'"
          :size="19"
        /><Info v-else :size="19" /><span>{{ toast.message }}</span
        ><button
          class="icon-btn small"
          aria-label="Dismiss notification"
          @click="toast = null"
        >
          <X :size="15" />
        </button></div
    ></Transition>
  </div>
</template>
