<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import {
  Archive, ArrowDownToLine, ArrowRight, BookMarked, BookOpen, BriefcaseBusiness,
  Check, CheckCircle2, ChevronRight, CircleAlert, CircleHelp, CloudOff,
  Ellipsis, FileCheck2, FilePlus2, FileStack, FileText, FolderOpen, GripVertical, Info,
  Layers3, LoaderCircle, LockKeyhole, Menu, PanelLeftClose, PanelLeftOpen, Plus, RefreshCw,
  Redo2, ScanText, Search, Settings2, ShieldAlert, ShieldCheck,
  Undo2, UploadCloud, WandSparkles, X
} from 'lucide-vue-next'
import PdfViewer from './components/PdfViewer.vue'
import DocumentThumbnail from './components/DocumentThumbnail.vue'
import PasswordInput from './components/PasswordInput.vue'
import PageTools from './components/PageTools.vue'
import ManualBookmarks from './components/ManualBookmarks.vue'
import BundlePreview from './components/BundlePreview.vue'
import OutputPresets from './components/OutputPresets.vue'
import DocumentTable from './components/DocumentTable.vue'
import ExportPdfReview from './components/ExportPdfReview.vue'
import MatterForm from './components/MatterForm.vue'
import MatterHome from './components/MatterHome.vue'
import CourtProfileGuide from './components/CourtProfileGuide.vue'
import { openUrl } from '@tauri-apps/plugin-opener'
import { VueDraggable } from 'vue-draggable-plus'
import { ApiError, api, downloadExport, downloadProtectedPdf, downloadMatter, inTauri, restoreMatter, uploadDocument } from './lib/api'
import { bytesLabel, friendlyError, prettyDate } from './lib/format'
import { applyAppearance, readAppearance, type Theme, type UiScale } from './lib/appearance'
import { confirmSignatureTransformation, signatureRiskDocuments, signatureStatusLabel } from './lib/signatures'
import type { Check as PreflightCheck, CourtProfile, DocKind, ExportManifest, Job, ManualBookmark, Matter, MatterDocument, MatterNew, PageSearchResult } from './lib/types'

type View = 'matters' | 'overview' | 'documents' | 'index' | 'build' | 'validation' | 'rules' | 'about'
const matterList = ref<Matter[]>([])
const archivedMatters = ref<Matter[]>([])
const currentMatterId = ref<string | null>(null)
const currentMatter = computed(() => matterList.value.find(m => m.id === currentMatterId.value) || null)
const docs = ref<MatterDocument[]>([])
const archivedDocs = ref<MatterDocument[]>([])
const storageUsage = ref({bytes:0,by_category:{originals:0,working:0,exports:0,other:0}})
const profileList = ref<CourtProfile[]>([])
const exportsList = ref<ExportManifest[]>([])
const recentJobs = ref<Job[]>([])
const previewRefreshKey = ref(0)
const bookmarkPreset = ref<{documentId:string,sequence:number}>({documentId:'',sequence:0})
const history = ref({can_undo:false,can_redo:false,undo_count:0,redo_count:0})
const guideReloadKey = ref(0)
const filingGuide = ref<InstanceType<typeof CourtProfileGuide> | null>(null)
const checks = ref<PreflightCheck[]>([])
const manualBookmarks = ref<ManualBookmark[]>([])
const selectedDocId = ref<string | null>(null)
const selectedDoc = computed(() => docs.value.find(d => d.id === selectedDocId.value) || null)
type OcrRun = { language:string; provider:string; pymupdf_version:string;
  tesseract_cli_version:string|null; model_sha256:Record<string,string>;
  pages_ocr_applied:number; applied_at:string; applied_revision:number }
const selectedOcrRuns = computed<OcrRun[]>(() => {
  try {
    const runs = JSON.parse(selectedDoc.value?.ocr_provenance_json || '[]')
    return Array.isArray(runs) ? runs : []
  } catch { return [] }
})
const detailsOpen = ref(false)
const readerFocus = ref(false)
const editorExpanded = ref(false)
let previousReaderFocus = false
function toggleEditorExpanded() {
  if (editorExpanded.value) {
    editorExpanded.value = false
    readerFocus.value = selectedDocId.value ? previousReaderFocus : false
  } else {
    previousReaderFocus = readerFocus.value
    readerFocus.value = true
    editorExpanded.value = true
  }
}

const documentTableOpen = ref(false)
const pageToolsOpen = ref(false)
const pageToolsDialog = ref<HTMLDialogElement | null>(null)
const documentsWorkspace = ref<HTMLElement | null>(null)
const documentsWorkspaceHeight = ref<number | null>(null)
const activeView = ref<View>('documents')
async function focusMain() {
  await nextTick()
  const regions = document.querySelectorAll<HTMLElement>('#main-content')
  regions[regions.length - 1]?.focus()
}
watch([activeView, selectedDocId, documentTableOpen], () => {
  if (editorExpanded.value && (activeView.value !== 'documents' || !selectedDocId.value || documentTableOpen.value)) toggleEditorExpanded()
})
const sidebarCollapsed = ref(false)
const smallScreen = ref(window.matchMedia('(max-width: 900px)').matches)
const mobileNavOpen = ref(false)
const searching = ref('')
const pageQuery = ref('')
const pageSearch = ref<PageSearchResult | null>(null)
const pageSearchBusy = ref(false)
const pageSearchError = ref('')
const pageTarget = ref<{documentId:string, page:number, sequence:number}>({documentId:'', page:1, sequence:0})
let searchTimer: ReturnType<typeof setTimeout> | null = null
let searchSequence = 0
const creating = ref(false)
const modalOpen = ref(false)
const appearance = readAppearance()
const theme = ref<Theme>(appearance.theme)
const uiScale = ref<UiScale>(appearance.scale)
const busy = ref(false)
const hoverUpload = ref(false)
const apiStatus = ref<'connecting'|'online'|'offline'>('connecting')
const vaultState = ref<'checking'|'setup'|'locked'|'unlocked'|'show_recovery'|'change_password'>('checking')
const vaultEnabled = ref(false)
const vaultCredential = ref('')
const vaultConfirm = ref('')
const vaultRecoveryMode = ref(false)
const vaultRecoveryKey = ref('')
const vaultBusy = ref(false)
const vaultError = ref('')
const protectedRestoreFile = ref<File | null>(null)
const protectedRestoreCredential = ref('')
const protectedRestoreRecovery = ref(false)
const protectedRestoreError = ref('')
const toast = ref<{message:string, kind:'success'|'error'|'info'} | null>(null)
const activeJob = ref<Job | null>(null)
const retryOcrLanguage = ref<'eng' | 'hin' | 'eng+hin'>('eng')
const savingExport = ref(false)
const reviewingPdf = ref<{exportId:string,filename:string} | null>(null)
const pdfImportRequest = ref<{name:string,error:string} | null>(null)
const pdfImportPassword = ref('')
let pdfImportResolve: ((password:string|null)=>void) | null = null
const protectedExportRequest = ref<{exportId:string,filename:string} | null>(null)
const protectedExportPassword = ref('')
const protectedExportConfirm = ref('')
const protectedExportError = ref('')
const fileInput = ref<HTMLInputElement | null>(null)
const matterFileInput = ref<HTMLInputElement | null>(null)
const docEditor = reactive({ title: '', kind: 'other' as DocKind, part: 1, label: '', group_key: '',
  document_date: '', date_confirmed: false,
  visible_signature_status: 'unreviewed' as MatterDocument['visible_signature_status'] })
const matterEditor = reactive({max_pages_per_volume:300,keep_groups_intact:true})
const formattingDirty = ref(false)
const outputPresetsPanel = ref<InstanceType<typeof OutputPresets> | null>(null)
let jobTimer: ReturnType<typeof setInterval> | null = null
let toastTimer: ReturnType<typeof setTimeout> | null = null
let healthTimer: ReturnType<typeof setInterval> | null = null
let connecting = false

const filteredDocs = computed(() => docs.value.filter(d => (d.title + ' ' + d.label + ' ' + d.kind).toLowerCase().includes(searching.value.toLowerCase())))
const selectedProfile = computed(() => profileList.value.find(p => p.id === currentMatter.value?.court_profile) || profileList.value[0])
const delhiGuidance = computed(() => profileList.value.find(p => p.id === 'delhi-hc-illustrative')?.filing_guidance)
const isJobActive = computed(() => !!activeJob.value && (activeJob.value.status === 'running' || activeJob.value.status === 'queued'))
const activeExport = computed(() => exportsList.value[0] || null)
const interruptedJobs = computed(() => recentJobs.value.filter(j => j.status === 'interrupted' && !j.retry_job_id))
const errorChecks = computed(() => checks.value.filter(c => c.severity === 'error'))
const warningChecks = computed(() => checks.value.filter(c => c.severity === 'warning'))
const signatureRiskDocs = computed(() => signatureRiskDocuments(docs.value))
const totalPages = computed(() => docs.value.reduce((n,d) => n+d.page_count, 0))
const documentKindOptions: DocKind[] = ['petition','affidavit','order','annexure','application','evidence','other']
const documentDirty = computed(() => !!selectedDoc.value && (
  docEditor.title !== selectedDoc.value.title || docEditor.kind !== selectedDoc.value.kind ||
  docEditor.part !== selectedDoc.value.part || docEditor.label !== selectedDoc.value.label ||
  docEditor.group_key !== selectedDoc.value.group_key ||
  docEditor.visible_signature_status !== selectedDoc.value.visible_signature_status ||
  docEditor.document_date !== selectedDoc.value.document_date ||
  docEditor.date_confirmed !== (selectedDoc.value.date_status === 'confirmed')))
const settingsDirty = computed(() => !!currentMatter.value && (
  matterEditor.max_pages_per_volume !== currentMatter.value.max_pages_per_volume ||
  matterEditor.keep_groups_intact !== currentMatter.value.keep_groups_intact))

function resetDrafts() {
  const doc = selectedDoc.value
  docEditor.title = doc?.title || ''
  docEditor.kind = doc?.kind || 'other'
  docEditor.part = doc?.part || 1
  docEditor.label = doc?.label || ''
  docEditor.group_key = doc?.group_key || ''
  docEditor.document_date = doc?.document_date || ''
  docEditor.date_confirmed = doc?.date_status === 'confirmed'
  docEditor.visible_signature_status = doc?.visible_signature_status || 'unreviewed'
  const matter = currentMatter.value
  matterEditor.max_pages_per_volume = matter?.max_pages_per_volume || 300
  matterEditor.keep_groups_intact = matter?.keep_groups_intact ?? true
}
function confirmDiscardDrafts() {
  if (!documentDirty.value && !settingsDirty.value && !formattingDirty.value) return true
  if (!window.confirm('Discard unsaved document details, bundle settings or output policy?')) return false
  resetDrafts()
  outputPresetsPanel.value?.discard()
  formattingDirty.value = false
  return true
}
function selectDocument(id: string) {
  if (selectedDocId.value === id || !confirmDiscardDrafts()) return
  pageTarget.value = {documentId:'', page:1, sequence:pageTarget.value.sequence + 1}
  selectedDocId.value = id
}
function openPreviewDocument(id: string) {
  if (!confirmDiscardDrafts()) return
  selectedDocId.value = id
  activeView.value = 'documents'
}
function prepareIndexBookmark(id: string) {
  bookmarkPreset.value = {documentId:id,sequence:bookmarkPreset.value.sequence+1}
}
watch(selectedDocId, id => { pageToolsOpen.value = false; if (!id) readerFocus.value = false })
function fitDocumentsWorkspace() {
  const workspace = documentsWorkspace.value
  if (!workspace || (window.innerWidth <= 700 && !editorExpanded.value)) {
    documentsWorkspaceHeight.value = null
    return
  }
  const scale = uiScale.value / 100
  const minimum = window.innerWidth <= 1120 ? 240 : 320
  documentsWorkspaceHeight.value = Math.max(minimum, Math.floor((window.innerHeight - workspace.getBoundingClientRect().top - 8) / scale))
}
watch([uiScale, activeView, currentMatterId, editorExpanded], async () => {
  await nextTick()
  window.requestAnimationFrame(fitDocumentsWorkspace)
})
watch(pageToolsOpen, async open => {
  await nextTick()
  const dialog = pageToolsDialog.value
  if (open && dialog && !dialog.open) dialog.showModal()
  if (!open && dialog?.open) dialog.close()
})
function showView(view: View) {
  if (activeView.value === view) {
    mobileNavOpen.value = false
    return
  }
  if (!confirmDiscardDrafts()) return
  activeView.value = view
  mobileNavOpen.value = false
  if (view === 'validation') void validate()
}
function toggleNavigation() {
  if (smallScreen.value) mobileNavOpen.value = !mobileNavOpen.value
  else sidebarCollapsed.value = !sidebarCollapsed.value
  void nextTick(() => window.requestAnimationFrame(fitDocumentsWorkspace))
}
function onResize() {
  const narrow = window.matchMedia('(max-width: 900px)').matches
  if (smallScreen.value !== narrow) mobileNavOpen.value = false
  smallScreen.value = narrow
  fitDocumentsWorkspace()
}
function beforeUnload(event: BeforeUnloadEvent) {
  if (!documentDirty.value && !settingsDirty.value) return
  event.preventDefault()
  event.returnValue = ''
}
function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape' && reviewingPdf.value) { reviewingPdf.value = null; return }
  if (event.key === 'Escape' && mobileNavOpen.value) { mobileNavOpen.value = false; return }
  if (event.defaultPrevented || modalOpen.value || event.altKey) return
  const target = event.target as HTMLElement | null
  const editingField = target?.isContentEditable || !!target?.closest('input, textarea, select, [contenteditable="true"]')
  if (event.key === 'Escape' && editorExpanded.value && !pageToolsOpen.value && !editingField) { toggleEditorExpanded(); return }
  if (event.key === 'Escape' && toast.value) { toast.value = null; return }
  if (editingField) return
  if (event.key === '/' || ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k')) {
    event.preventDefault()
    readerFocus.value = false
    if (activeView.value !== 'documents') showView('documents')
    if (activeView.value === 'documents') void nextTick(() => document.querySelector<HTMLInputElement>('input[aria-label="Search documents"]')?.focus())
  }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'z' && currentMatter.value && !busy.value) {
    event.preventDefault()
    if (event.shiftKey && history.value.can_redo) void traverseHistory('redo')
    else if (!event.shiftKey && history.value.can_undo) void traverseHistory('undo')
  }
}
watch([theme, uiScale], () => applyAppearance(theme.value, uiScale.value), { immediate: true })

watch(selectedDoc, (doc, previous) => {
  if (doc && previous?.id === doc.id && (
    docEditor.title !== previous.title || docEditor.kind !== previous.kind ||
    docEditor.part !== previous.part || docEditor.label !== previous.label ||
    docEditor.group_key !== previous.group_key || docEditor.document_date !== previous.document_date ||
    docEditor.date_confirmed !== (previous.date_status === 'confirmed') ||
    docEditor.visible_signature_status !== previous.visible_signature_status)) return
  docEditor.title = doc?.title || ''
  docEditor.kind = doc?.kind || 'other'
  docEditor.part = doc?.part || 1
  docEditor.label = doc?.label || ''
  docEditor.group_key = doc?.group_key || ''
  docEditor.document_date = doc?.document_date || ''
  docEditor.date_confirmed = doc?.date_status === 'confirmed'
  docEditor.visible_signature_status = doc?.visible_signature_status || 'unreviewed'
}, {immediate:true})
watch(currentMatter, (item, previous) => {
  if (item && previous?.id === item.id && (
    matterEditor.max_pages_per_volume !== previous.max_pages_per_volume ||
    matterEditor.keep_groups_intact !== previous.keep_groups_intact)) return
  matterEditor.max_pages_per_volume = item?.max_pages_per_volume || 300
  matterEditor.keep_groups_intact = item?.keep_groups_intact ?? true
}, {immediate:true})
watch(currentMatterId, () => {
  detailsOpen.value = false
  documentTableOpen.value = false
  pageQuery.value = ''
  pageSearch.value = null
  searchSequence++
  if (currentMatterId.value) {
    void loadMatterResources()
  }
})
watch(pageQuery, () => {
  if (searchTimer) clearTimeout(searchTimer)
  if (pageQuery.value.trim().length < 2) {
    pageSearch.value = null
    pageSearchError.value = ''
    pageSearchBusy.value = false
    searchSequence++
    return
  }
  searchTimer = setTimeout(() => void searchPages(), 350)
})

async function searchPages() {
  const id = currentMatterId.value
  const q = pageQuery.value.trim()
  const sequence = ++searchSequence
  if (!id || q.length < 2) return
  pageSearchBusy.value = true
  pageSearchError.value = ''
  try {
    const result = await api<PageSearchResult>(`/matters/${id}/search?q=${encodeURIComponent(q)}`, 'GET', undefined, 5 * 60_000)
    if (sequence === searchSequence && id === currentMatterId.value) pageSearch.value = result
  } catch (err) {
    if (sequence === searchSequence) pageSearchError.value = friendlyError(err)
  } finally {
    if (sequence === searchSequence) pageSearchBusy.value = false
  }
}
function openSearchMatch(documentId: string, page: number) {
  if (!confirmDiscardDrafts()) return
  selectedDocId.value = documentId
  pageTarget.value = {documentId, page, sequence:pageTarget.value.sequence + 1}
}
function highlightedSnippet(snippet: string, query: string) {
  const index = snippet.toLocaleLowerCase().indexOf(query.toLocaleLowerCase())
  return index < 0 ? {before:snippet, match:'', after:''} : {
    before:snippet.slice(0, index), match:snippet.slice(index, index + query.length),
    after:snippet.slice(index + query.length),
  }
}

function notify(message:string, kind:'success'|'error'|'info'='info') {
  toast.value={message,kind}
  if (toastTimer) clearTimeout(toastTimer)
  toastTimer=setTimeout(()=>toast.value=null,kind === 'error' && message.length > 140 ? 12000 : 6500)
}
function issue(err: unknown) { notify(friendlyError(err), 'error') }

async function openOfficialReference(url: string) {
  const parsed = new URL(url)
  const allowedHosts = new Set(['delhihighcourt.nic.in', 'www.delhihighcourt.nic.in', 'dhcefiling.nic.in'])
  if (parsed.protocol !== 'https:' || !allowedHosts.has(parsed.hostname)) {
    notify('Blocked an unexpected external reference URL.', 'error')
    return
  }
  try {
    if (inTauri()) await openUrl(parsed.href)
    else window.open(parsed.href, '_blank', 'noopener,noreferrer')
  } catch (err) {
    issue(err)
  }
}
async function openReviewedExternalLink(url: string) {
  let parsed: URL
  try { parsed = new URL(url) }
  catch { notify('This reviewed external link is invalid.', 'error'); return }
  if (!['https:', 'http:', 'mailto:'].includes(parsed.protocol)) {
    notify('Blocked a reviewed link with an unsafe URL scheme.', 'error')
    return
  }
  try {
    if (inTauri()) await openUrl(parsed.href)
    else window.open(parsed.href, '_blank', 'noopener,noreferrer')
  } catch (err) { issue(err) }
}

async function init() {
  if (connecting) return
  connecting = true
  apiStatus.value = 'connecting'
  try {
    let connected = false
    const deadline = Date.now() + (inTauri() ? 60_000 : 12_000)
    while (Date.now() < deadline) {
      try {
        await api('/health', 'GET', undefined, 1_500)
        connected = true
        break
      } catch { await new Promise(resolve => setTimeout(resolve, 400)) }
    }
    if (!connected) throw new Error(inTauri()
      ? 'Local engine did not start. Quit and reopen CourtBundle. If this continues, check the packaged sidecar and local port.'
      : 'Local engine did not start. Run backend/dev.sh and retry.')
    const vault = await api<{state:'setup'|'locked'|'unlocked',enabled:boolean}>('/vault/status')
    vaultEnabled.value = vault.enabled
    if (vault.state !== 'unlocked') {
      vaultState.value = vault.state
      apiStatus.value = 'online'
      return
    }
    vaultState.value = 'unlocked'
    profileList.value = await api<CourtProfile[]>('/profiles')
    await refreshMatters()
    apiStatus.value = 'online'
  } catch (err) { apiStatus.value='offline'; issue(err) }
  finally { connecting = false }
}
async function setupVault() {
  if (vaultBusy.value) return
  if (vaultCredential.value !== vaultConfirm.value) { vaultError.value = 'Passwords do not match.'; return }
  vaultBusy.value = true; vaultError.value = ''
  try {
    const result = await api<{recovery_key:string}>('/vault/setup', 'POST', {password:vaultCredential.value}, 5 * 60_000)
    vaultCredential.value = ''; vaultConfirm.value = ''
    vaultRecoveryKey.value = result.recovery_key
    vaultState.value = 'show_recovery'
  } catch (err) { vaultError.value = friendlyError(err) }
  finally { vaultBusy.value = false }
}
async function unlockVault() {
  if (vaultBusy.value) return
  vaultBusy.value = true; vaultError.value = ''
  try {
    await api('/vault/unlock', 'POST', {credential:vaultCredential.value,recovery:vaultRecoveryMode.value}, 5 * 60_000)
    vaultCredential.value = ''
    vaultState.value = 'unlocked'
    await init()
  } catch (err) { vaultError.value = friendlyError(err) }
  finally { vaultBusy.value = false }
}
async function rotateVault() {
  if (vaultBusy.value) return
  if (vaultCredential.value !== vaultConfirm.value) { vaultError.value = 'Passwords do not match.'; return }
  vaultBusy.value = true; vaultError.value = ''
  try {
    const result = await api<{recovery_key:string}>('/vault/rotate', 'POST', {new_password:vaultCredential.value})
    vaultCredential.value = ''; vaultConfirm.value = ''
    vaultRecoveryKey.value = result.recovery_key
    vaultState.value = 'show_recovery'
  } catch (err) { vaultError.value = friendlyError(err) }
  finally { vaultBusy.value = false }
}
async function finishVaultSetup() {
  vaultRecoveryKey.value = ''
  vaultState.value = 'unlocked'
  await init()
}
async function checkHealth() {
  if (connecting || apiStatus.value === 'offline') return
  try {
    await api('/health', 'GET', undefined, 1_500)
  } catch {
    if (apiStatus.value === 'online') {
      apiStatus.value = 'offline'
      notify(inTauri()
        ? 'Local engine stopped. Quit and reopen CourtBundle to start a new session.'
        : 'Local engine stopped. Restart backend/dev.sh and retry.', 'error')
    }
  }
}
async function refreshMatters() {
  const [all, archived] = await Promise.all([
    api<Matter[]>('/matters'), api<Matter[]>('/matters/archived')])
  matterList.value = all
  archivedMatters.value = archived
  if (currentMatterId.value && !all.some(m => m.id === currentMatterId.value)) currentMatterId.value = null
  if (!currentMatterId.value && all.length) currentMatterId.value = all[0].id
}
async function loadMatterResources() {
  if (!currentMatterId.value) return
  const id = currentMatterId.value
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
    ])
    if (currentMatterId.value !== id) return
    docs.value=d; checks.value=c; exportsList.value=e; recentJobs.value=j; history.value=h
    manualBookmarks.value=bookmarks
    previewRefreshKey.value += 1
    if (pageQuery.value.trim().length >= 2) void searchPages()
    archivedDocs.value=archived; storageUsage.value=usage
    const ongoing = j.find(job => job.status === 'queued' || job.status === 'running')
    if (ongoing && (!activeJob.value || activeJob.value.id !== ongoing.id)) watchJob(ongoing)
    if (!selectedDocId.value || !d.some(doc=>doc.id===selectedDocId.value)) selectedDocId.value=d[0]?.id || null
  } catch(err) { issue(err) }
}
async function refreshHistory() {
  if (currentMatterId.value) history.value = await api<typeof history.value>(`/matters/${currentMatterId.value}/history`)
}
async function traverseHistory(direction:'undo'|'redo') {
  if (!currentMatterId.value || busy.value || !confirmDiscardDrafts()) return
  if (filingGuide.value && !(await filingGuide.value.flushPendingSave())) {
    notify('Save the Memo of Parties before using Undo or Redo.', 'error')
    return
  }
  busy.value = true
  try {
    await api(`/matters/${currentMatterId.value}/history/${direction}`,'POST')
    await refreshMatters()
    await loadMatterResources()
    guideReloadKey.value += 1
    notify(direction === 'undo' ? 'Last edit undone.' : 'Edit restored.', 'success')
  } catch (err) { issue(err) }
  finally { busy.value = false }
}
async function createMatter(fields: MatterNew) {
  if (!confirmDiscardDrafts()) return
  creating.value=true
  try {
    const value=await api<Matter>('/matters','POST',fields)
    await refreshMatters()
    currentMatterId.value=value.id
    activeView.value='documents'; selectedDocId.value=null
    modalOpen.value=false
    notify('Matter created. Import your first document to begin.', 'success')
  } catch(err) { issue(err) }
  finally { creating.value=false }
}
async function createSampleMatter() {
  if (creating.value || !confirmDiscardDrafts()) return
  creating.value = true
  try {
    const value = await api<Matter>('/matters','POST',{
      name:`Synthetic Sample Matter ${new Date().toLocaleDateString('en-CA')}`,
      court_profile:'general-demo', case_type:'Synthetic demonstration',
      petitioner:'Sample Applicant', respondent:'Sample Respondent', advocate:'', case_number:'',
      max_pages_per_volume:10, keep_groups_intact:true,
    })
    await api(`/matters/${value.id}/cover`,'POST')
    await refreshMatters()
    currentMatterId.value = value.id
    selectedDocId.value = null
    activeView.value = 'documents'
    await loadMatterResources()
    notify('Opened a separate synthetic sample matter. Its draft cover is not a court-approved form.', 'success')
  } catch (err) { issue(err) }
  finally { creating.value = false }
}
async function backupCurrentMatter() {
  if (!currentMatterId.value || busy.value || !confirmDiscardDrafts()) return
  busy.value = true
  try {
    if (await downloadMatter(currentMatterId.value)) notify('Portable matter backup saved.', 'success')
  } catch (err) { issue(err) }
  finally { busy.value = false }
}
async function importMatterPackage(files: FileList | null) {
  const file = files?.item(0)
  if (!file || busy.value || !confirmDiscardDrafts()) return
  busy.value = true
  try {
    const restored = await restoreMatter(file)
    await useRestoredMatter(restored)
  } catch (err) {
    if (err instanceof ApiError && err.status === 422 && err.message.includes('backup needs')) {
      protectedRestoreFile.value = file
      protectedRestoreError.value = ''
    } else issue(err)
  }
  finally { busy.value = false; if (matterFileInput.value) matterFileInput.value.value = '' }
}
async function useRestoredMatter(restored: Matter) {
  await refreshMatters()
  currentMatterId.value = restored.id
  selectedDocId.value = null
  activeView.value = 'documents'
  notify('Matter restored with originals and working revisions.', 'success')
}
async function submitProtectedRestore() {
  if (!protectedRestoreFile.value || busy.value) return
  busy.value = true; protectedRestoreError.value = ''
  try {
    const restored = await restoreMatter(protectedRestoreFile.value,
      protectedRestoreCredential.value, protectedRestoreRecovery.value)
    protectedRestoreFile.value = null
    protectedRestoreCredential.value = ''
    await useRestoredMatter(restored)
  } catch (err) { protectedRestoreError.value = friendlyError(err) }
  finally { busy.value = false }
}
async function onUpload(files: FileList | File[]) {
  if (!currentMatterId.value || busy.value) return
  const incoming=Array.from(files).filter(f => /\.(pdf|png|jpe?g)$/i.test(f.name))
  if (!incoming.length) { notify('Choose PDF, JPG or PNG files.', 'error'); return }
  busy.value=true
  let imported = 0
  try {
    for (const file of incoming) {
      let password: string | undefined
      let acknowledged = false
      while (true) {
        try {
          await uploadDocument(currentMatterId.value, file, password, acknowledged)
          imported++
          break
        } catch (err) {
          if (err instanceof ApiError && err.status === 422 &&
              /PDF password required|Incorrect PDF password/.test(err.message)) {
            const supplied = await askPdfImportPassword(file.name,
              err.message.includes('Incorrect') ? 'Incorrect PDF password. Try again or cancel.' : '')
            password = supplied === null ? undefined : supplied
            if (supplied === null) { notify(`${file.name} import cancelled.`, 'info'); break }
            continue
          }
          if (err instanceof ApiError && err.status === 409 &&
              err.message.includes('Protected PDF signature review required')) {
            acknowledged = window.confirm('This protected PDF may contain certificate signatures. Creating a decrypted working copy can invalidate them. Certificate validity is not verified. The encrypted original is preserved. Continue importing?')
            if (!acknowledged) { notify(`${file.name} import cancelled.`, 'info'); break }
            continue
          }
          throw err
        }
      }
      password = undefined
    }
    await loadMatterResources()
    if (imported) notify(`${imported} document(s) imported. Original files preserved.`, 'success')
  } catch(err) { issue(err); await loadMatterResources() }
  finally { busy.value=false; if(fileInput.value) fileInput.value.value='' }
}
function askPdfImportPassword(name:string, error:string): Promise<string|null> {
  pdfImportPassword.value = ''
  pdfImportRequest.value = {name,error}
  return new Promise(resolve => { pdfImportResolve = resolve })
}
function finishPdfImportPassword(password:string|null) {
  const resolve = pdfImportResolve
  pdfImportResolve = null
  pdfImportPassword.value = ''
  pdfImportRequest.value = null
  resolve?.(password)
}
function onDropUpload(e: DragEvent) {
  hoverUpload.value=false
  if (e.dataTransfer?.files) void onUpload(e.dataTransfer.files)
}
async function generateCover() {
  if (!currentMatterId.value) return
  busy.value=true
  try {
    await api(`/matters/${currentMatterId.value}/cover`,'POST')
    await loadMatterResources()
    notify('Draft cover sheet created from matter details.', 'success')
  } catch(err) { issue(err) }
  finally { busy.value=false }
}
async function saveDocument() {
  if (!selectedDoc.value) return
  busy.value=true
  try {
    const preserveAmbiguous = !docEditor.document_date && selectedDoc.value.date_status === 'ambiguous'
    const updated=await api<MatterDocument>(`/documents/${selectedDoc.value.id}`,'PATCH',{
      title:docEditor.title, kind:docEditor.kind, part:docEditor.part, label:docEditor.label,
      group_key:docEditor.group_key, document_date:docEditor.document_date,
      visible_signature_status:docEditor.visible_signature_status,
      date_status:docEditor.document_date ? (docEditor.date_confirmed ? 'confirmed' : selectedDoc.value.date_status) : (preserveAmbiguous ? 'ambiguous' : 'unknown'),
      date_source:docEditor.document_date ? (docEditor.date_confirmed ? 'user' : selectedDoc.value.date_source) : (preserveAmbiguous ? selectedDoc.value.date_source : 'none'),
      date_note:preserveAmbiguous ? selectedDoc.value.date_note : '',
    })
    docs.value=docs.value.map(x=>x.id===updated.id ? updated : x)
    await refreshHistory()
    await validate()
    await loadMatterResources()
    notify('Document details updated.', 'success')
  } catch(err){issue(err)} finally{busy.value=false}
}
async function documentTableModified() {
  await refreshMatters()
  await loadMatterResources()
  notify('Document table changes saved. Bundle order and freshness were refreshed.', 'success')
}
function openDocumentFromTable(id:string) {
  documentTableOpen.value = false
  selectedDocId.value = id
  detailsOpen.value = true
}
async function deleteDocument() {
  if (!selectedDoc.value || !confirmDiscardDrafts() || !window.confirm(`Archive ${selectedDoc.value.title}? Its records and original remain available for restore.`)) return
  busy.value=true
  try {
    await api(`/documents/${selectedDoc.value.id}`,'DELETE')
    selectedDocId.value=null
    await loadMatterResources()
    notify('Document archived. It can be restored from this matter.', 'success')
  } catch(err){ issue(err) } finally { busy.value=false }
}
async function persistDocumentOrder(ids: string[]) {
  if (!currentMatterId.value || busy.value) return
  busy.value = true
  try {
    docs.value=await api<MatterDocument[]>(`/matters/${currentMatterId.value}/documents/reorder`,'POST',{ids})
    await refreshHistory()
    await validate()
    await loadMatterResources()
    notify('Document order and index page ranges updated.', 'success')
  } catch(err){issue(err); await loadMatterResources()}
  finally { busy.value = false }
}
async function submitDocumentOrder(ids: string[]) {
  if (!currentMatterId.value || busy.value || !confirmDiscardDrafts()) return
  await persistDocumentOrder(ids)
}
function applyDocumentSort(reordered: MatterDocument[]) {
  const ids = reordered.map(doc => doc.id)
  const currentIds = docs.value.map(doc => doc.id)
  const sameDocuments = ids.length === currentIds.length && ids.every(id => currentIds.includes(id))
  if (!sameDocuments || busy.value || searching.value.trim() || !confirmDiscardDrafts()) {
    // A new array forces Vue to restore Sortable's moved DOM when a drop is rejected.
    docs.value = [...docs.value]
    return
  }
  if (ids.every((id,index) => id === currentIds[index])) return
  docs.value = [...reordered]
  void persistDocumentOrder(ids)
}
async function moveDocumentById(id: string, direction: number) {
  if (!currentMatterId.value) return
  const ids=docs.value.map(d=>d.id); const position=ids.indexOf(id)
  const next=position+direction
  if(position<0 || next<0 || next>=ids.length) return
  ;[ids[position],ids[next]]=[ids[next],ids[position]]
  await submitDocumentOrder(ids)
}
function moveDocumentFromHandle(id: string, direction: number, filtered = false) {
  if (busy.value || (filtered && searching.value.trim())) return
  void moveDocumentById(id, direction)
}
async function refreshDocument() { await loadMatterResources() }
async function startOcr(language: 'eng' | 'hin' | 'eng+hin') {
  if(!selectedDoc.value || isJobActive.value) return
  const signature = confirmSignatureTransformation([selectedDoc.value], 'OCR')
  if (!signature.proceed) return
  try {
    const job=await api<Job>(`/documents/${selectedDoc.value.id}/ocr`,'POST',{
      acknowledge_signature_risk:signature.acknowledged,
      language,
    })
    watchJob(job)
    notify('OCR started on scanned and mixed-content pages. Review recognized text.', 'info')
  } catch(err){issue(err)}
}
async function validate(): Promise<PreflightCheck[]> {
  if(!currentMatterId.value) return []
  try {
    checks.value=await api<PreflightCheck[]>(`/matters/${currentMatterId.value}/validate`)
    return checks.value
  }
  catch(err){issue(err); return [{severity:'error',code:'VALIDATION_UNAVAILABLE',message:'Technical checks could not be loaded.',document_id:null}]}
}
async function savePolicy() {
  if(!currentMatter.value) return false
  try {
    const updated=await api<Matter>(`/matters/${currentMatter.value.id}`,'PATCH',{...matterEditor})
    matterList.value=matterList.value.map(m=>m.id===updated.id?updated:m)
    await refreshHistory()
    await loadMatterResources()
    notify('Bundle settings saved.', 'success')
    return true
  } catch(err){issue(err);return false}
}
async function applyOutputPreset(presetId: string) {
  if (!currentMatterId.value || !confirmDiscardDrafts()) return
  try {
    await api(`/matters/${currentMatterId.value}/output-preset/${presetId}`, 'POST')
    await refreshMatters()
    resetDrafts()
    await loadMatterResources()
    notify('Preset version applied. Preview marks on actual pages before building.', 'success')
  } catch (err) { issue(err) }
}
async function startBuild(): Promise<boolean> {
  if (!currentMatterId.value || isJobActive.value) return false
  try {
    if (formattingDirty.value) {
      notify('Save numbering and annexure policy before building.', 'error')
      return false
    }
    if (!await savePolicy()) return false
    const signature = confirmSignatureTransformation(docs.value, 'Bundle generation')
    if (!signature.proceed) return false
    const job=await api<Job>(`/matters/${currentMatterId.value}/build`,'POST',{
      acknowledge_signature_risk:signature.acknowledged,
    })
    watchJob(job)
    return true
  } catch(err){issue(err); return false}
}
function watchJob(job:Job) {
  activeJob.value=job
  if(jobTimer)clearInterval(jobTimer)
  let fetching=false
  jobTimer=setInterval(async()=>{
    if(fetching)return
    fetching=true
    try{
      const next=await api<Job>(`/jobs/${job.id}`)
      activeJob.value=next
      if(['completed','failed','cancelled'].includes(next.status)) {
        if(jobTimer)clearInterval(jobTimer)
        jobTimer=null
        if(next.status==='failed')notify(next.error || 'Job failed', 'error')
        if(next.status==='completed')notify(next.kind==='ocr'?'OCR processing finished.':next.kind==='compress'?'PDF compression finished. Review the size and quality.':'Filing bundle generated successfully.','success')
        if(next.status==='cancelled')notify('Processing cancelled.','info')
        await loadMatterResources()
      }
    } catch(err){if(jobTimer) clearInterval(jobTimer);jobTimer=null;issue(err)}
    finally{fetching=false}
  },650)
}
async function cancelJob(){if(activeJob.value)await api(`/jobs/${activeJob.value.id}/cancel`,'POST').catch(issue)}
async function retryInterrupted(job: Job) {
  try {
    const relevant = job.kind === 'ocr' ? docs.value.filter(document => document.id === job.target_id) : docs.value
    const signature = confirmSignatureTransformation(relevant,
      job.kind === 'ocr' ? 'Retrying OCR' : 'Retrying bundle generation')
    if (!signature.proceed) return
    const retry = await api<Job>(`/jobs/${job.id}/retry`, 'POST', {
      acknowledge_signature_risk: signature.acknowledged,
      ...(job.kind === 'ocr' ? { language: retryOcrLanguage.value } : {}),
    })
    watchJob(retry)
    await loadMatterResources()
    notify(`Restarted ${job.kind} as a new job.`, 'info')
  } catch (err) { issue(err) }
}
async function download(exportId:string, filename:string, saveAs=false){
  if(!currentMatterId.value || savingExport.value)return
  savingExport.value = true
  try{
    const saved = await downloadExport(currentMatterId.value,exportId,filename,saveAs)
    if(saved && inTauri()) notify(`${filename} saved.`, 'success')
  }catch(err){issue(err)}
  finally{savingExport.value = false}
}
function beginProtectedExport(exportId:string, filename:string) {
  protectedExportPassword.value = ''
  protectedExportConfirm.value = ''
  protectedExportError.value = ''
  protectedExportRequest.value = {exportId,filename}
}
function cancelProtectedExport() {
  protectedExportPassword.value = ''
  protectedExportConfirm.value = ''
  protectedExportError.value = ''
  protectedExportRequest.value = null
}
async function saveProtectedExport() {
  const request = protectedExportRequest.value
  if (!request || !currentMatterId.value || savingExport.value) return
  if (protectedExportPassword.value !== protectedExportConfirm.value) {
    protectedExportError.value = 'Passwords do not match.'
    return
  }
  if (!protectedExportPassword.value || new TextEncoder().encode(protectedExportPassword.value).length > 40) {
    protectedExportError.value = 'Enter a password of 1 to 40 UTF-8 bytes.'
    return
  }
  savingExport.value = true
  protectedExportError.value = ''
  try {
    const saved = await downloadProtectedPdf(currentMatterId.value, request.exportId,
      request.filename, protectedExportPassword.value)
    if (saved) { cancelProtectedExport(); notify('Password-protected PDF saved. Reopen it with the password before use.', 'success') }
  } catch (err) { protectedExportError.value = friendlyError(err) }
  finally { savingExport.value = false }
}
function reviewGeneratedPdf(exportId:string, filename:string, bytes:number) {
  if (bytes > 25 * 1024 * 1024) {
    notify('This PDF exceeds the 25 MB in-app review limit. Save it and inspect it in a PDF viewer.', 'info')
    return
  }
  reviewingPdf.value = {exportId,filename}
}
function selectMatter(id:string, view:View='documents'){
  if (currentMatterId.value === id) { showView(view); return }
  if (!confirmDiscardDrafts()) return
  currentMatterId.value=id;selectedDocId.value=null;activeView.value=view;readerFocus.value=false
  mobileNavOpen.value = false
}
async function removeMatter(){
  if(!currentMatter.value || !confirmDiscardDrafts() || !window.confirm('Archive this matter? Its records, originals and working files remain available for restore.'))return
  try{await api(`/matters/${currentMatter.value.id}`,'DELETE');currentMatterId.value=null;docs.value=[];await refreshMatters();notify('Matter archived.','info')}
  catch(err){issue(err)}
}
async function restoreArchivedMatter(id: string) {
  if (!confirmDiscardDrafts()) return
  try {
    await api(`/matters/${id}/restore`, 'POST')
    await refreshMatters()
    selectMatter(id)
    notify('Archived matter restored.', 'success')
  } catch (err) { issue(err) }
}
async function restoreArchivedDocument(id: string) {
  if (!currentMatterId.value) return
  try {
    await api(`/matters/${currentMatterId.value}/archived-documents/${id}/restore`, 'POST')
    await loadMatterResources()
    notify('Archived document restored.', 'success')
  } catch (err) { issue(err) }
}
const iconsBySeverity = {error:CircleAlert,warning:ShieldAlert,info:Info}
onMounted(()=>{try { sidebarCollapsed.value = localStorage.getItem('courtbundle.sidebarCollapsed.v1') === 'true' } catch { /* Storage may be unavailable. */ } void init(); healthTimer=setInterval(()=>void checkHealth(),5_000);window.addEventListener('beforeunload',beforeUnload);window.addEventListener('keydown',onKeydown);window.addEventListener('resize',onResize)})
onUnmounted(()=>{if(jobTimer)clearInterval(jobTimer);if(toastTimer)clearTimeout(toastTimer);if(healthTimer)clearInterval(healthTimer);window.removeEventListener('beforeunload',beforeUnload);window.removeEventListener('keydown',onKeydown);window.removeEventListener('resize',onResize)})
watch(sidebarCollapsed, collapsed => { try { localStorage.setItem('courtbundle.sidebarCollapsed.v1', String(collapsed)) } catch { /* Storage may be unavailable. */ } })
</script>

<template>
  <main v-if="vaultState!=='unlocked' && apiStatus!=='offline'" class="vault-screen">
    <section :class="['vault-card','vault-state-'+vaultState]" aria-labelledby="vault-title">
      <div :class="['vault-mark',{loading:vaultState==='checking'}]" aria-hidden="true">
        <LoaderCircle v-if="vaultState==='checking'" :size="27"/>
        <LockKeyhole v-else :size="27"/>
      </div>
      <div class="vault-heading">
        <span>{{vaultState==='checking'?'LOCAL ENGINE':vaultState==='show_recovery'?'RECOVERY KEY':'PROTECTED WORKSPACE'}}</span>
        <h1 id="vault-title">{{vaultState==='setup'?'Protect local matters':vaultState==='locked'?'Unlock CourtBundle':vaultState==='show_recovery'?'Save your recovery key':vaultState==='change_password'?'Change vault password':'Starting local engine'}}</h1>
      </div>
      <p v-if="vaultState==='checking'">Opening the encrypted local workspace. CourtBundle stays on this computer and does not upload matter files.</p>
      <p v-if="vaultState==='setup'">Create a password for the local matter store. The existing local data will be migrated before the workspace opens. Keep the separate recovery key outside this computer.</p>
      <p v-else-if="vaultState==='locked'">Enter your password or recovery key. If both are lost, encrypted matter data cannot be recovered.</p>
      <p v-else-if="vaultState==='change_password'">Changing the password creates a new recovery key. Previous recovery keys stop unlocking this local store.</p>
      <p v-else-if="vaultState==='show_recovery'">Write down this key and keep it separately. It is shown only now and is not stored in the app.</p>
      <div v-if="vaultState==='checking'" class="vault-progress" role="progressbar" aria-label="Starting local engine"><span></span></div>
      <p v-if="vaultError" class="notice amber" role="alert">{{vaultError}}</p>
      <form v-if="vaultState==='setup' || vaultState==='change_password'" @submit.prevent="vaultState==='setup'?setupVault():rotateVault()">
        <PasswordInput v-model="vaultCredential" label="New password" autocomplete="new-password" :minlength="6"/>
        <PasswordInput v-model="vaultConfirm" label="Confirm password" autocomplete="new-password" :minlength="6"/>
        <small>Minimum 6 characters. A longer password is safer; keep your recovery key separately.</small>
        <button class="btn primary" type="submit" :disabled="vaultBusy">{{vaultBusy?'Protecting local files…':vaultState==='setup'?'Protect and open':'Change password'}}</button>
        <button v-if="vaultState==='change_password'" class="btn secondary" type="button" @click="vaultState='unlocked';vaultCredential='';vaultConfirm='';vaultError=''">Cancel</button>
      </form>
      <form v-else-if="vaultState==='locked'" @submit.prevent="unlockVault()">
        <label v-if="vaultRecoveryMode">Recovery key<input v-model="vaultCredential" type="text" autocomplete="off" required/></label>
        <PasswordInput v-else v-model="vaultCredential" label="Password" autocomplete="current-password"/>
        <label class="vault-choice"><input v-model="vaultRecoveryMode" type="checkbox"/> Use recovery key</label>
        <button class="btn primary" type="submit" :disabled="vaultBusy">{{vaultBusy?'Unlocking…':'Unlock'}}</button>
      </form>
      <div v-else-if="vaultState==='show_recovery'" class="vault-recovery">
        <div class="vault-recovery-key"><span>RECOVERY KEY</span><code>{{vaultRecoveryKey}}</code></div>
        <div class="vault-recovery-warning"><ShieldAlert :size="18"/><span>Store this separately from your password. Anyone with this key can unlock the local matter store.</span></div>
        <button class="btn primary" @click="finishVaultSetup">I saved the recovery key</button>
      </div>
    </section>
  </main>
  <div v-else class="app-shell" :data-editor-expanded="editorExpanded">
    <a class="skip-link" href="#main-content" @click.prevent="focusMain">Skip to main content</a>
    <button v-if="smallScreen && mobileNavOpen" class="sidebar-backdrop" aria-label="Close navigation" @click="mobileNavOpen=false"></button>
    <aside class="sidebar" :class="{collapsed:sidebarCollapsed && !smallScreen, 'mobile-open':mobileNavOpen}" :inert="smallScreen && !mobileNavOpen">
      <div class="brand-area">
        <div class="brand-mark"><Layers3 :size="22" :stroke-width="2.3"/></div>
        <div class="brand-copy"><div class="brand-name">CourtBundle<span class="brand-plus">.</span></div><div class="brand-sub">LEGAL WORKSPACE / POC</div></div>
      </div>
      <div class="side-label">WORKSPACE</div>
      <nav class="side-nav">
        <button class="side-link" aria-label="Matters" title="Matters" :class="{active:activeView!=='rules'&&activeView!=='about'}" @click="showView('matters')"><BriefcaseBusiness :size="18"/><span class="nav-label">Matters</span><span class="side-count">{{matterList.length}}</span></button>
        <button class="side-link" aria-label="Court rules" title="Court rules" :class="{active:activeView==='rules'}" @click="showView('rules')"><BookOpen :size="18"/><span class="nav-label">Court rules</span></button>
        <button class="side-link" aria-label="About POC" title="About POC" :class="{active:activeView==='about'}" @click="showView('about')"><CircleHelp :size="18"/><span class="nav-label">About POC</span></button>
      </nav>
      <input ref="matterFileInput" type="file" accept=".cbmatter" hidden @change="e=>importMatterPackage((e.target as HTMLInputElement).files)"/>
      <button class="side-link" aria-label="Restore matter" title="Restore matter" :disabled="busy" @click="matterFileInput?.click()"><FolderOpen :size="18"/><span class="nav-label">Restore matter</span></button>
      <div class="side-label matter-section-label"><span class="nav-label">YOUR MATTERS</span><button aria-label="New matter" title="New matter" class="side-plus" @click="modalOpen=true"><Plus :size="17"/></button></div>
      <div class="matter-list">
        <button v-for="m in matterList" :key="m.id" class="matter-link" :aria-label="m.name" :title="m.name" :class="{active:currentMatterId===m.id&&activeView!=='rules'&&activeView!=='about'}" @click="selectMatter(m.id)">
          <div class="matter-initial">{{m.name.slice(0,1).toUpperCase()}}</div>
          <div class="matter-link-label"><span>{{m.name}}</span><small>{{m.case_type}}</small></div>
        </button>
        <div v-if="!matterList.length" class="sidebar-empty">No matters yet</div>
      </div>
      <div v-if="archivedMatters.length" class="side-label">ARCHIVED MATTERS</div>
      <div v-if="archivedMatters.length" class="matter-list">
        <button v-for="m in archivedMatters" :key="m.id" class="matter-link" :aria-label="'Restore '+m.name" :title="'Restore '+m.name" @click="restoreArchivedMatter(m.id)"><Archive :size="17"/><span>{{m.name}}</span></button>
      </div>
      <div class="sidebar-bottom">
        <button v-if="vaultEnabled" class="side-link" @click="vaultError='';vaultState='change_password'"><LockKeyhole :size="17"/><span>Change vault password</span></button>
        <div class="appearance-controls">
          <label>Theme<select v-model="theme" aria-label="Theme"><option value="system">System</option><option value="light">Light</option><option value="dark">Dark</option></select></label>
          <label>Interface size<select v-model.number="uiScale" aria-label="Interface size"><option :value="90">90%</option><option :value="100">100%</option><option :value="110">110%</option><option :value="125">125%</option></select></label>
          <small>/ search · ⌘/Ctrl+Z undo · Shift+⌘/Ctrl+Z redo</small>
        </div>
        <div class="offline-status"><CloudOff :size="17"/><div><strong>Offline-first</strong><small>Files stay on this computer</small></div></div>
        <div class="version-chip"><span :class="['status-dot',apiStatus]"></span> {{apiStatus==='online'?'Local engine connected':apiStatus==='offline'?'Engine disconnected':'Connecting…'}} <span class="version-text">v0.1</span></div>
      </div>
    </aside>

    <div class="app-main" :class="{'workspace-main':currentMatter && activeView!=='rules' && activeView!=='about'}">
      <header class="topbar">
        <div class="topbar-left"><button class="icon-btn nav-toggle" :aria-label="smallScreen ? (mobileNavOpen ? 'Close navigation' : 'Open navigation') : (sidebarCollapsed ? 'Expand sidebar' : 'Collapse sidebar')" :aria-expanded="smallScreen ? mobileNavOpen : !sidebarCollapsed" @click="toggleNavigation"><Menu v-if="smallScreen" :size="20"/><PanelLeftOpen v-else-if="sidebarCollapsed" :size="20"/><PanelLeftClose v-else :size="20"/></button><div class="breadcrumb"><span>Workspace</span><ChevronRight :size="14"/><strong>{{activeView==='rules'?'Court rules':activeView==='about'?'About POC':(activeView==='matters'?'Matters':currentMatter?.name||'Matters')}}</strong></div></div>
        <div class="topbar-right"><span class="desktop-tag">DESKTOP POC</span><span class="user-badge">CB</span></div>
      </header>

      <div v-if="apiStatus==='offline'" class="backend-error"><CircleAlert :size="19"/><div><strong>Python engine unavailable</strong><p v-if="inTauri()">Quit and reopen CourtBundle to start a new local engine session. If it still fails, inspect the packaged sidecar and local port.</p><p v-else>Run <code>cd backend && ./dev.sh</code>, then retry. The browser development token is for local testing only.</p></div><button class="btn secondary" @click="init">Retry connection</button></div>

      <Transition name="view" @after-enter="fitDocumentsWorkspace">
      <main v-if="activeView==='rules'" id="main-content" tabindex="-1" class="page-content rules-page">
        <div class="page-eyebrow">LEGAL KNOWLEDGE</div><h1>Court rules & technical profiles</h1><p class="page-description">Source-linked preparation checks. These do not replace registry instructions or an advocate's review.</p>
        <div class="notice subtle profile-scope"><Info :size="18"/><span><strong v-if="currentMatter">{{selectedProfile?.name}} is selected for {{currentMatter.name}}. </strong>Profiles currently affect searchable-page warnings and the export file size cap. The filing guidance and linked forms below are references for advocate review; they are not applied automatically.</span></div>
        <div class="rules-grid"><div v-for="profile in profileList" :key="profile.id" class="card rule-card">
          <div class="rule-icon"><BookMarked :size="24" /></div><div class="badge muted">{{profile.status==='DEMO_ONLY'?'DEMO ONLY':'NOT LEGALLY APPROVED'}}</div><div v-if="currentMatter?.court_profile===profile.id" class="badge profile-selected">SELECTED FOR THIS MATTER</div>
          <h2>{{profile.name}}</h2><p>{{profile.description}}</p>
          <div class="rule-facts"><div><span>File size setting</span><strong>{{profile.max_file_mb ? profile.max_file_mb+' MB' : 'None'}}</strong></div><div><span>Searchable text check</span><strong>{{profile.require_searchable_text?'Enabled':'Off'}}</strong></div><div><span>Source review date</span><strong>{{profile.last_checked}}</strong></div></div>
          <div class="rule-sources" v-if="profile.sources.length"><span>OFFICIAL REFERENCE LINKS</span><a v-for="source in profile.sources" :key="source.url" :href="source.url" target="_blank" rel="noopener noreferrer" @click.prevent="openOfficialReference(source.url)">{{source.title}} <ArrowRight :size="13"/></a></div>
        </div></div>
        <CourtProfileGuide v-if="delhiGuidance" :key="guideReloadKey" ref="filingGuide" :guidance="delhiGuidance" :matter-id="currentMatterId" :can-undo="history.can_undo" :can-redo="history.can_redo" @open-url="openOfficialReference" @modified="refreshHistory" @undo="traverseHistory('undo')" @redo="traverseHistory('redo')"/>
        <div class="notice amber"><ShieldAlert :size="19"/> These rules are only a selected technical subset. Case-type documents, notarisation, signatures, fees, limitation and court acceptance are outside the POC's validation scope.</div>
      </main>

      <main v-else-if="activeView==='about'" id="main-content" tabindex="-1" class="page-content about-page">
        <div class="page-eyebrow">PRODUCT DEMONSTRATION</div><h1>Built for structured legal filing preparation.</h1>
        <p class="page-description">A local-first proof of concept connecting Tauri, Vue and Python to convert source documents into indexed legal bundles.</p>
        <div class="feature-strip"><div><FileStack :size="23"/><h3>Preserve originals</h3><p>Working copies are edited separately from imported source documents.</p></div><div><WandSparkles :size="23"/><h3>Deterministic assembly</h3><p>Volume splitting, bookmarks and indexes share the same page map.</p></div><div><ShieldCheck :size="23"/><h3>Visible limitations</h3><p>Illustrative rules and a validation report, not a filing approval.</p></div></div>
        <div class="card about-card"><h2>What is included?</h2><p>Matters, PDF/JPEG/PNG import, working-copy page operations, English and Hindi OCR, drag-drop ordering, annexure/part metadata, auto index, volume splitting, linked bookmarks, technical validation and ZIP export.</p><h2>What is not included?</h2><p>Actual court submission, legally approved templates, digital signing, Word/Excel/PPT conversion, comprehensive PDF text editing, cloud sharing or guaranteed legal compliance.</p></div>
      </main>

      <MatterHome v-else-if="activeView==='matters'" :matters="matterList" :creating="creating" @open="id=>selectMatter(id,'overview')" @create="modalOpen=true" @sample="createSampleMatter"/>

      <main v-else-if="!currentMatter" id="main-content" tabindex="-1" class="page-content welcome-page">
        <div class="welcome-icon"><FolderOpen :size="32"/></div><div class="page-eyebrow">YOUR FIRST MATTER</div><h1>From scattered files to<br/>a structured court bundle.</h1>
        <p>Create a matter to import documents, organise annexures, generate indexes and prepare a complete PDF package—offline.</p>
        <div class="welcome-actions"><button class="btn primary large" @click="modalOpen=true"><Plus :size="18"/> Create your first matter</button><button class="btn secondary large" :disabled="creating" @click="createSampleMatter"><WandSparkles :size="18"/> {{creating?'Opening sample…':'Open sample matter'}}</button></div>
        <div class="welcome-steps"><span>01 &nbsp; Add documents</span><ArrowRight :size="18"/><span>02 &nbsp; Collate & OCR</span><ArrowRight :size="18"/><span>03 &nbsp; Build bundle</span></div>
      </main>

      <main v-else id="main-content" tabindex="-1" class="page-content workspace-page">
        <div class="matter-heading">
          <div class="matter-heading-copy"><h1>{{currentMatter.name}}</h1><p class="matter-description"><span>{{currentMatter.case_number||'Draft'}}</span><span class="dot-sep">·</span>{{currentMatter.case_type}} <span class="dot-sep">·</span> {{selectedProfile?.name}}</p></div>
          <div class="matter-heading-actions">
            <button class="btn secondary small matter-icon-action" aria-label="Undo" title="Undo" :disabled="busy || !history.can_undo" @click="traverseHistory('undo')"><Undo2 :size="16"/></button>
            <button class="btn secondary small matter-icon-action" aria-label="Redo" title="Redo" :disabled="busy || !history.can_redo" @click="traverseHistory('redo')"><Redo2 :size="16"/></button>
            <details class="matter-more"><summary class="btn secondary small matter-icon-action" aria-label="More actions" title="More actions"><Ellipsis :size="17"/></summary><div class="matter-more-menu">
              <button :disabled="busy" @click="backupCurrentMatter">Back up matter</button>
              <button @click="confirmDiscardDrafts() && void loadMatterResources()"><RefreshCw :size="15"/> Refresh</button>
              <button :disabled="busy" @click="removeMatter">Archive matter</button>
            </div></details>
            <button class="btn primary" @click="showView('build')"><FileStack :size="17"/> Build filing bundle</button>
          </div>
        </div>
        <div class="matter-summary" aria-label="Matter summary"><span><strong>{{docs.length}}</strong> documents</span><span><strong>{{totalPages}}</strong> source pages</span><span><strong>{{warningChecks.length+errorChecks.length}}</strong> review flags</span><span><strong>{{exportsList.length}}</strong> prepared packages</span></div>
        <nav class="workspace-tabs" aria-label="Matter preparation"><button :class="{active:activeView==='overview'}" @click="showView('overview')"><BriefcaseBusiness :size="16"/> Overview</button><button :class="{active:activeView==='documents'}" @click="showView('documents')"><FileStack :size="16"/> Documents</button><button :class="{active:activeView==='index'}" @click="showView('index')"><BookMarked :size="16"/> Index & bookmarks</button><button :class="{active:activeView==='build'}" @click="showView('build')"><Layers3 :size="16"/> Build bundle</button><button :class="{active:activeView==='validation'}" @click="showView('validation')"><ShieldCheck :size="16"/> Preflight checks <span v-if="warningChecks.length+errorChecks.length" class="count-bubble">{{warningChecks.length+errorChecks.length}}</span></button></nav>

        <Transition name="section" mode="out-in" @after-enter="fitDocumentsWorkspace">
        <section v-if="activeView==='overview'" class="matter-overview" aria-label="Matter overview">
          <div class="overview-lead panel"><div><div class="eyebrow">PREPARE YOUR BUNDLE</div><h2>{{docs.length ? 'Continue preparing this matter' : 'Begin with your source documents'}}</h2><p>Review working copies, arrange the index, check the result and build your filing package.</p></div><button class="btn primary" @click="showView('documents')"><FolderOpen :size="17"/> {{docs.length ? 'Open documents' : 'Add documents'}}</button></div>
          <div class="overview-steps">
            <button class="panel overview-step" @click="showView('documents')"><span class="step-number">01</span><h3>Documents</h3><p>Import, read and edit working copies.</p><strong>{{docs.length}} documents · {{totalPages}} pages <ArrowRight :size="16"/></strong></button>
            <button class="panel overview-step" @click="showView('index')"><span class="step-number">02</span><h3>Index & bookmarks</h3><p>Arrange bundle order and review page destinations.</p><strong>Review page map <ArrowRight :size="16"/></strong></button>
            <button class="panel overview-step" @click="showView('validation')"><span class="step-number">03</span><h3>Preflight checks</h3><p>Run technical checks and inspect flagged items.</p><strong>Run checks <ArrowRight :size="16"/></strong></button>
            <button class="panel overview-step" @click="showView('build')"><span class="step-number">04</span><h3>Build & export</h3><p>Generate, review and save the finished PDFs.</p><strong>{{exportsList.length}} prepared packages <ArrowRight :size="16"/></strong></button>
          </div>
          <div class="overview-bottom"><section class="panel overview-context"><h2>Case details</h2><dl><dt>Petitioner</dt><dd>{{currentMatter.petitioner || 'Not entered'}}</dd><dt>Respondent</dt><dd>{{currentMatter.respondent || 'Not entered'}}</dd><dt>Advocate</dt><dd>{{currentMatter.advocate || 'Not entered'}}</dd><dt>Court profile</dt><dd>{{selectedProfile?.name}}</dd></dl></section><section class="panel overview-context"><h2>Latest package</h2><template v-if="activeExport"><p>{{prettyDate(activeExport.created_at)}} · {{activeExport.volumes}} volume(s) · {{activeExport.pages}} pages</p><p>{{activeExport.stale ? 'Inputs changed. Rebuild before using this package.' : 'Review the generated PDF before use.'}}</p><button class="btn secondary" @click="showView('build')">Review exports</button></template><p v-else>No package built yet. Start with documents and review the page map before building.</p></section></div>
        </section>
        <DocumentTable v-else-if="activeView==='documents' && documentTableOpen" :matter-id="currentMatterId!" :documents="docs" :busy="busy || isJobActive" :keep-groups="currentMatter.keep_groups_intact" @modified="documentTableModified" @open-document="openDocumentFromTable" @error="issue" @close="documentTableOpen=false"/>
        <div v-else-if="activeView==='documents'" ref="documentsWorkspace" class="documents-workspace" :class="{'details-open':detailsOpen && !!selectedDoc,'reader-focus':readerFocus}" :style="documentsWorkspaceHeight ? {height:documentsWorkspaceHeight+'px'} : undefined">
          <section v-show="!readerFocus" class="panel docs-panel" :class="{'has-documents':docs.length>0}">
            <div class="panel-header"><div><h2>Source documents</h2><span>Drag or use ↑/↓ to reorder</span></div><div class="panel-header-actions"><button class="text-tool" :disabled="!docs.length" @click="documentTableOpen=true">Open table</button><button class="icon-btn" title="Generate draft cover sheet" :disabled="busy" @click="generateCover"><FilePlus2 :size="19"/></button></div></div>
            <div class="docs-search"><Search :size="16"/><input v-model="searching" placeholder="Search documents..." aria-label="Search documents"/></div>
            <div class="docs-search"><Search :size="16"/><input v-model="pageQuery" placeholder="Find text on pages..." aria-label="Find text on pages"/></div>
            <div v-if="pageQuery.trim().length >= 2" class="page-search-results" aria-live="polite">
              <small v-if="pageSearchBusy">Searching working PDFs…</small>
              <small v-else-if="pageSearchError" class="error-msg">{{pageSearchError}}</small>
              <template v-else-if="pageSearch">
                <small>{{pageSearch.matches.length}} matching page(s){{pageSearch.truncated ? ' · first 100 shown' : ''}}</small>
                <button v-for="match in pageSearch.matches" :key="match.document_id+'-'+match.page" class="page-search-hit" @click="openSearchMatch(match.document_id, match.page)"><strong>{{match.title}} · page {{match.page}}</strong><span>{{highlightedSnippet(match.snippet, pageQuery.trim()).before}}<mark>{{highlightedSnippet(match.snippet, pageQuery.trim()).match}}</mark>{{highlightedSnippet(match.snippet, pageQuery.trim()).after}}</span></button>
              </template>
            </div>
            <VueDraggable v-if="filteredDocs.length" :model-value="filteredDocs" class="docs-list" item-key="id" handle=".drag-handle" direction="vertical" :disabled="busy || !!searching.trim()" :animation="220" easing="cubic-bezier(.2,.7,.2,1)" :force-fallback="true" :fallback-tolerance="5" :fallback-on-body="true" ghost-class="sortable-ghost" chosen-class="sortable-chosen" drag-class="sortable-drag" fallback-class="sortable-fallback" @update:model-value="applyDocumentSort">
              <div v-for="doc in filteredDocs" :key="doc.id" class="doc-item" :data-document-id="doc.id" :class="{selected:selectedDocId===doc.id}" role="button" tabindex="0" :aria-label="`Open ${doc.title}, ${doc.page_count} ${doc.page_count===1?'page':'pages'}`" @click="selectDocument(doc.id)" @keydown.enter.prevent="selectDocument(doc.id)" @keydown.space.prevent="selectDocument(doc.id)">
                <span class="grip drag-handle" role="button" :tabindex="busy || !!searching.trim() ? -1 : 0" :aria-label="'Reorder document '+doc.title+'. Press Up or Down arrow keys.'" @click.stop @keydown.up.prevent="moveDocumentFromHandle(doc.id,-1,true)" @keydown.down.prevent="moveDocumentFromHandle(doc.id,1,true)"><GripVertical :size="16"/></span>
                <DocumentThumbnail :document-id="doc.id" :revision="doc.revision"/>
                <div class="doc-info"><strong :title="doc.title">{{doc.title}}</strong><span><span v-if="doc.label" class="doc-label">{{doc.label}} · </span>Part {{doc.part}} · {{doc.page_count}} {{doc.page_count===1?'page':'pages'}}</span></div>
              </div>
            </VueDraggable>
            <div v-else-if="docs.length" class="no-matching">No matching documents</div>
            <div v-else class="no-documents"><FileStack :size="30"/><strong>No documents added</strong><small>Import a PDF or scanned image to begin.</small></div>
            <div v-if="archivedDocs.length" class="docs-help"><Archive :size="14"/> Archived documents</div>
            <div v-for="doc in archivedDocs" :key="doc.id" class="export-file"><FileText :size="16"/><div><strong>{{doc.title}}</strong><small>{{doc.original_name}}</small></div><button class="text-tool" @click="restoreArchivedDocument(doc.id)">Restore</button></div>
            <div class="upload-zone" :class="{over:hoverUpload}" @dragover.prevent="hoverUpload=true" @dragleave="hoverUpload=false" @drop.prevent="onDropUpload"><input ref="fileInput" type="file" accept=".pdf,.jpg,.jpeg,.png" multiple hidden @change="e=>onUpload((e.target as HTMLInputElement).files || [])"/><UploadCloud :size="25"/><strong>Drop files here</strong><span>PDF, JPG, PNG · up to 80 MB/file</span><button class="btn secondary small" :disabled="busy" @click="fileInput?.click()">{{busy?'Importing…':'Browse files'}}</button></div>
            <div class="docs-help"><LockKeyhole :size="14"/> Original files are never overwritten.</div>
          </section>

          <section class="panel preview-panel"><PdfViewer :expanded="editorExpanded" @toggle-expanded="toggleEditorExpanded" :focus-mode="readerFocus" @toggle-focus="readerFocus=!readerFocus" :document="selectedDoc" :documents="docs" :disabled="busy || isJobActive || documentDirty" :can-undo="history.can_undo" :can-redo="history.can_redo" :details-open="detailsOpen" :details-dirty="documentDirty" :bookmark-refresh-key="previewRefreshKey" :page-target="pageTarget" @modified="refreshDocument" @undo="traverseHistory('undo')" @redo="traverseHistory('redo')" @error="issue" @page-tools="pageToolsOpen=true" @toggle-details="detailsOpen=!detailsOpen" @ocr="startOcr" @archive-document="deleteDocument" @open-internal-link="openSearchMatch" @open-external-link="openReviewedExternalLink"/></section>

          <Transition name="inspector">
          <section v-if="detailsOpen && selectedDoc" id="document-details-panel" class="panel inspector-panel" aria-label="Document details">
            <div class="panel-header"><div><h2>Document details</h2><span>Metadata for collation</span></div><button class="icon-btn" aria-label="Close document details" @click="detailsOpen=false"><X :size="18"/></button></div>
              <div class="inspector-content">
                <div class="inspector-heading"><div class="doc-icon big"><FileText :size="22"/></div><div><strong>{{selectedDoc.original_name}}</strong><small>{{selectedDoc.page_count}} pages · {{selectedDoc.ocr_status.split('_').join(' ')}}</small></div></div>
                <div v-if="selectedOcrRuns.length" class="notice subtle ocr-provenance" role="note">
                  <ScanText :size="18"/><span><strong>OCR provenance · unverified text</strong>
                    <small v-for="run in selectedOcrRuns" :key="run.applied_revision">
                      Revision {{run.applied_revision}}: {{run.language.split('+').join(' + ')}} · {{run.pages_ocr_applied}} {{run.pages_ocr_applied === 1 ? 'page' : 'pages'}} · PyMuPDF {{run.pymupdf_version}}<template v-if="run.tesseract_cli_version"> · {{run.tesseract_cli_version}} (CLI)</template><br/>
                      <span v-for="(hash, language) in run.model_sha256" :key="language" :title="hash">{{language}} model SHA-256: {{hash}}</span>
                    </small>
                    <small>These identifiers describe the OCR software and model files. Recognized text still needs review against the page.</small>
                  </span>
                </div>
                <label class="field">Display title<input v-model="docEditor.title" maxlength="250"/></label>
                <label class="field">Document type<select v-model="docEditor.kind"><option v-for="kind in documentKindOptions" :key="kind" :value="kind">{{kind[0].toUpperCase()+kind.slice(1)}}</option></select></label>
                <div class="field-row"><label class="field">Part<select v-model.number="docEditor.part"><option v-for="part in 4" :value="part">Part {{part}}</option></select></label><label class="field">Label<input v-model="docEditor.label" placeholder="P-1" maxlength="60"/></label></div>
                <label class="field">Document date<input v-model="docEditor.document_date" type="date" @input="docEditor.date_confirmed=true"/><span v-if="selectedDoc.date_status==='ambiguous' && !docEditor.document_date" class="hint">Ambiguous {{selectedDoc.date_source}} text: {{selectedDoc.date_note}}. Enter and confirm a date only after checking the document.</span><span v-else class="hint">Unknown is preserved when blank. A file timestamp is never treated as the document date.</span></label>
                <label v-if="docEditor.document_date" class="checkbox-line"><input v-model="docEditor.date_confirmed" type="checkbox"/><span><strong>Confirmed from the document</strong><small v-if="selectedDoc.date_status==='suggested' && !docEditor.date_confirmed">Current value is an unreviewed {{selectedDoc.date_source}} suggestion.</small></span></label>
                <div :class="['notice', ['certificate_signature_detected','inspection_inconclusive','visible_signature_only'].includes(selectedDoc.signature_status) ? 'amber' : 'subtle']">
                  <ShieldAlert :size="18"/><span><strong>{{signatureStatusLabel(selectedDoc.signature_status)}}</strong><small>{{selectedDoc.signature_inspection_note}} This does not validate a certificate, signer, scanned mark or current signature coverage.</small></span>
                </div>
                <label class="field">Visible/scanned signature review
                  <select v-model="docEditor.visible_signature_status">
                    <option value="unreviewed">Not reviewed</option>
                    <option value="present">Visible or scanned signature appears present</option>
                    <option value="absent">No visible or scanned signature found in review</option>
                  </select>
                  <span class="hint">This is a reviewer classification only. It is separate from PDF certificate indicators and does not verify a signature.</span>
                </label>
                <label class="field">Keep-together group<input v-model="docEditor.group_key" placeholder="Optional: annexures-a"/><span class="hint">Adjacent documents with the same group stay together when enabled.</span></label>
                <p v-if="documentDirty" role="status" class="hint">Unsaved document changes</p>
                <button class="btn primary block" :disabled="busy || !docEditor.title.trim()" @click="saveDocument"><Check :size="16"/> Save document details</button>
              </div>
          </section>
          </Transition>
        </div>

        <div v-else-if="activeView==='index'" class="index-preparation">
          <div class="notice subtle"><Info :size="17"/> Prepare the index and manual bookmarks as you arrange documents. Page ranges update from current matter order and the saved volume settings; review all destinations before export.</div>
          <section v-if="docs.length" class="panel index-order-panel" aria-label="Index document order"><div class="panel-header"><div><h2>Index order</h2><span>Drag or focus a handle and press ↑/↓ to change order and page ranges.</span></div></div>
            <VueDraggable :model-value="docs" class="index-order-list" item-key="id" handle=".drag-handle" :disabled="busy" :animation="160" :force-fallback="true" :fallback-tolerance="5" :fallback-on-body="true" ghost-class="sortable-ghost" chosen-class="sortable-chosen" drag-class="sortable-drag" fallback-class="sortable-fallback" @update:model-value="applyDocumentSort"><div v-for="(doc,index) in docs" :key="doc.id" class="index-order-row" :data-document-id="doc.id"><span class="grip drag-handle" role="button" :tabindex="busy ? -1 : 0" :aria-label="'Reorder document '+doc.title+'. Press Up or Down arrow keys.'" @click.stop @keydown.up.prevent="moveDocumentFromHandle(doc.id,-1)" @keydown.down.prevent="moveDocumentFromHandle(doc.id,1)"><GripVertical :size="16"/></span><strong>{{index+1}}. {{doc.label ? doc.label+' · ' : ''}}{{doc.title}}</strong><span>{{doc.page_count}} page(s)</span></div></VueDraggable>
          </section>
          <BundlePreview :matter-id="currentMatterId!" :max-pages="matterEditor.max_pages_per_volume" :keep-groups="matterEditor.keep_groups_intact" :refresh-key="previewRefreshKey" @open-document="openPreviewDocument" @bookmark-document="prepareIndexBookmark"/>
          <ManualBookmarks :key="currentMatterId!" :matter-id="currentMatterId!" :documents="docs" :refresh-key="previewRefreshKey" :preset="bookmarkPreset" :disabled="busy || isJobActive" @modified="loadMatterResources"/>
          <button class="btn primary" :disabled="!docs.length" @click="showView('build')"><Layers3 :size="17"/> Continue to Build bundle</button>
        </div>

        <div v-else-if="activeView==='build'" class="build-layout">
          <div class="build-main">
            <div class="panel build-panel"><div class="panel-header"><div><h2>Collation & export settings</h2><span>Configured per matter—not universal legal rules</span></div><Settings2 :size="18" class="muted-icon"/></div>
              <div class="build-body"><label class="field">Maximum pages per volume <span class="help-inline">(including index pages)</span><input v-model.number="matterEditor.max_pages_per_volume" type="number" min="2" max="2000"/></label><p class="setting-description">Client-configured document packaging limit. It is not a nationwide Indian court requirement.</p>
                <label class="checkbox-line"><input v-model="matterEditor.keep_groups_intact" type="checkbox"/><div><strong>Keep document groups together</strong><span>Grouped adjacent documents must fit in the same volume.</span></div></label>
                <button class="btn secondary" :disabled="!settingsDirty || isJobActive" @click="savePolicy"><Check :size="16"/> Save bundle settings</button>
                <div class="checklist"><div><CheckCircle2 :size="17"/> Generate index pages in every volume</div><div><CheckCircle2 :size="17"/> Include clickable index links and PDF bookmarks</div><div><CheckCircle2 :size="17"/> Generate combined PDF + separate volumes</div><div><CheckCircle2 :size="17"/> Archive in ZIP with validation report</div></div>
                <div class="notice amber"><ShieldAlert :size="18"/><span>Working-copy PDF annotation objects and imported/source link actions are omitted from volume and combined PDFs; flattened marks remain. Generated index links, bookmarks and explicitly reviewed CourtBundle links are retained only when their revision-anchored targets resolve. Cross-volume reviewed links work in the combined PDF and are omitted from standalone volumes. Review all warnings; signature integrity and filing acceptance are not certified.</span></div>
                <div v-if="signatureRiskDocs.length" class="notice amber"><ShieldAlert :size="18"/><span><strong>{{signatureRiskDocs.length}} signature-risk document(s) require acknowledgment before build.</strong><small>Bundle generation rewrites pages and cannot preserve certificate protections. Imported originals remain unchanged; no signature validity is asserted.</small></span></div>
                <p v-if="settingsDirty" role="status" class="hint">Unsaved bundle settings. Building saves these settings first.</p>
                <button class="btn primary large" :disabled="!docs.length || isJobActive" @click="startBuild"><Layers3 :size="19"/> {{isJobActive?'Processing…':'Build filing bundle'}}</button>
                <div v-if="isJobActive && activeJob" class="job-progress"><div class="job-progress-line"><span>{{activeJob.message}}</span><strong>{{activeJob.progress}}%</strong></div><div class="progress-track"><div :style="{width:activeJob.progress+'%'}"></div></div><button class="text-tool danger" @click="cancelJob">Cancel operation</button></div>
                <div v-else-if="activeJob?.status==='failed'" class="notice error"><CircleAlert :size="18"/> {{activeJob.error}}</div>
                <div v-for="job in interruptedJobs" :key="job.id" class="notice amber"><CircleAlert :size="18"/><span>{{job.kind === 'ocr' ? 'OCR' : job.kind === 'compress' ? 'Compression' : 'Bundle build'}} was interrupted by an engine restart. Review the current revision and exports before retrying.</span><span v-if="job.kind === 'compress'">Choose compression again in page tools.</span><label v-if="job.kind === 'ocr'">OCR language <select v-model="retryOcrLanguage"><option value="eng">English</option><option value="hin">Hindi</option><option value="eng+hin">English + Hindi</option></select></label><button v-if="job.kind !== 'compress'" class="text-tool" :disabled="isJobActive" @click="retryInterrupted(job)">Retry</button></div>
              </div>
            </div>
            <OutputPresets :key="currentMatterId!" ref="outputPresetsPanel" :matter-id="currentMatterId!" :max-pages="matterEditor.max_pages_per_volume" :keep-groups="matterEditor.keep_groups_intact" :settings-dirty="settingsDirty" :disabled="isJobActive" :refresh-key="previewRefreshKey" :documents="docs" @apply-requested="applyOutputPreset" @dirty-changed="formattingDirty=$event" @modified="loadMatterResources" @open-document="openPreviewDocument"/>
            <BundlePreview :matter-id="currentMatterId!" :max-pages="matterEditor.max_pages_per_volume" :keep-groups="matterEditor.keep_groups_intact" :refresh-key="previewRefreshKey" @open-document="openPreviewDocument"/>
            <div class="panel exports-panel"><div class="panel-header"><div><h2>Generated exports</h2><span>Stored locally for repeat downloads</span></div><Archive :size="18" class="muted-icon"/></div>
              <div v-if="activeExport" class="export-body"><div v-if="activeExport.stale" class="notice amber"><ShieldAlert :size="18"/> Matter inputs changed since this export. Rebuild before using it.</div><div v-if="activeJob?.kind==='build' && activeJob.status==='failed'" class="notice amber"><ShieldAlert :size="18"/> The latest build failed. These files are from an earlier completed export; review them before use.</div><div class="export-summary"><div class="export-icon"><FileCheck2 :size="25"/></div><div><strong>{{activeJob?.kind==='build' && activeJob.status==='failed' ? 'Previous filing package' : 'Latest filing package'}}</strong><span>{{prettyDate(activeExport.created_at)}} · {{activeExport.volumes}} volume(s) · {{activeExport.pages}} pages</span></div></div><div v-if="activeExport.checks.some(check => check.code==='SOURCE_ANNOTATIONS_OMITTED' || check.code==='SOURCE_LINKS_OMITTED')" class="notice amber"><ShieldAlert :size="18"/> This export omitted source annotations or links. See validation-report.json for affected documents; generated index links remain.</div>
                <div v-for="file in activeExport.files" :key="file.name" class="export-file"><FileText :size="18"/><div><strong>{{file.name}}</strong><small>{{bytesLabel(file.bytes)}}</small></div><button v-if="file.name.endsWith('.pdf')" class="text-tool" :aria-label="'Review '+file.name" @click="reviewGeneratedPdf(activeExport.id,file.name,file.bytes)">Review PDF</button><button class="icon-btn" :disabled="savingExport" :title="inTauri() ? 'Save '+file.name : 'Download '+file.name" :aria-label="inTauri() ? 'Save '+file.name : 'Download '+file.name" @click="download(activeExport.id,file.name)"><ArrowDownToLine :size="18"/></button><button v-if="inTauri()" class="text-tool" :disabled="savingExport" :title="'Save '+file.name+' as'" @click="download(activeExport.id,file.name,true)">Save As</button><button v-if="file.name.endsWith('.pdf') && !selectedProfile?.disallow_pdf_encryption" class="text-tool" :disabled="savingExport" :aria-label="'Save password-protected '+file.name" @click="beginProtectedExport(activeExport.id,file.name)">Protect PDF…</button></div>
                <p v-if="selectedProfile?.disallow_pdf_encryption" class="export-footnote">Encrypted PDF output is disabled by this illustrative profile as an unapproved technical setting. This is not a reviewed court rule.</p>
                <p class="export-footnote">The ZIP contains PDFs, an index CSV, and a technical validation report. It is not submitted to a court.</p>
              </div><div v-else class="empty-export"><Archive :size="28"/><strong>No exports yet</strong><span>Build a bundle to see generated files.</span></div>
            </div>
          </div>
          <div class="build-side"><div class="panel snapshot-panel"><h3>Matter snapshot</h3><div><span>Documents</span><strong>{{docs.length}}</strong></div><div><span>Local storage</span><strong>{{bytesLabel(storageUsage.bytes)}}</strong></div><div><span>Originals</span><strong>{{bytesLabel(storageUsage.by_category.originals)}}</strong></div><div><span>Working revisions</span><strong>{{bytesLabel(storageUsage.by_category.working)}}</strong></div><div><span>Exports</span><strong>{{bytesLabel(storageUsage.by_category.exports)}}</strong></div><div><span>Source pages</span><strong>{{totalPages}}</strong></div><div><span>Volume page cap</span><strong>{{matterEditor.max_pages_per_volume}}</strong></div><div><span>Court profile</span><strong>{{selectedProfile?.id}}</strong></div><div><span>Review flags</span><strong>{{checks.filter(c=>c.severity!=='info').length}}</strong></div></div><div class="notice subtle"><Info :size="17"/> Review every output against the required court directions. Grouping and page policy are configurable.</div></div>
        </div>

        <div v-else-if="activeView==='validation'" class="validation-layout"><div class="panel validation-panel"><div class="panel-header"><div><h2>Technical preflight</h2><span>Checks that are automated in this POC</span></div><button class="btn secondary small" @click="validate"><RefreshCw :size="15"/> Run checks</button></div>
          <div class="validation-intro"><div class="validation-icon"><ShieldCheck :size="25"/></div><div><strong>{{errorChecks.length}} errors · {{warningChecks.length}} warnings</strong><p>No legal judgment is made about your documents. Fix flagged items and review all other filing requirements with the advocate.</p></div></div>
          <div v-for="(check,index) in checks" :key="index" :class="['check-entry',check.severity]"><component :is="iconsBySeverity[check.severity]" :size="18"/><div><strong>{{check.code.split('_').join(' ')}}</strong><p>{{check.message}}</p></div><span class="check-severity">{{check.severity}}</span></div>
          <div v-if="!checks.length" class="empty-export">No checks reported.</div>
        </div><div class="panel review-panel"><h3>Human review remains essential</h3><p>These items are not certified by this application:</p><div><Check :size="16"/> Court-specific case-type requirements</div><div><Check :size="16"/> Signature and attestation validity</div><div><Check :size="16"/> Evidence authenticity and translations</div><div><Check :size="16"/> Limitation, fees and registry acceptance</div></div></div>
        </Transition>
      </main>
      </Transition>
    </div>
    <dialog v-if="selectedDoc" ref="pageToolsDialog" class="page-tools-dialog" aria-labelledby="page-tools-title" @close="pageToolsOpen=false" @click.self="pageToolsOpen=false">
      <div class="page-tools-dialog-head"><div><h2 id="page-tools-title">Page tools</h2><p>{{selectedDoc.title}} · {{selectedDoc.page_count}} pages · Choose what you want to do below</p></div><button class="icon-btn" aria-label="Close page tools" @click="pageToolsOpen=false"><X :size="18"/></button></div>
      <p v-if="documentDirty" class="notice amber">Save document details before editing pages.</p>
      <PageTools :document="selectedDoc" :documents="docs" :disabled="busy || isJobActive || documentDirty" :job="activeJob" expanded @modified="refreshDocument" @job-started="watchJob" @cancel="cancelJob"/>
    </dialog>
    <ExportPdfReview v-if="reviewingPdf && currentMatterId" :matter-id="currentMatterId" :export-id="reviewingPdf.exportId" :filename="reviewingPdf.filename" @close="reviewingPdf=null"/>
    <div v-if="pdfImportRequest" class="vault-restore-overlay" role="presentation">
      <section class="vault-card" role="dialog" aria-modal="true" aria-labelledby="pdf-import-password-title">
        <h2 id="pdf-import-password-title">Open protected PDF</h2>
        <p>{{pdfImportRequest.name}} requires a password. The encrypted original will be preserved.</p>
        <p v-if="pdfImportRequest.error" class="notice amber" role="alert">{{pdfImportRequest.error}}</p>
        <form @submit.prevent="finishPdfImportPassword(pdfImportPassword)">
          <PasswordInput v-model="pdfImportPassword" label="PDF password" autocomplete="off"/>
          <button class="btn primary" type="submit">Open and import</button>
          <button class="btn secondary" type="button" @click="finishPdfImportPassword(null)">Cancel import</button>
        </form>
      </section>
    </div>
    <div v-if="protectedExportRequest" class="vault-restore-overlay" role="presentation">
      <section class="vault-card" role="dialog" aria-modal="true" aria-labelledby="pdf-export-password-title">
        <h2 id="pdf-export-password-title">Save protected PDF</h2>
        <p>Choose a password for {{protectedExportRequest.filename}}. You will need it to reopen the saved PDF.</p>
        <p v-if="protectedExportError" class="notice amber" role="alert">{{protectedExportError}}</p>
        <form @submit.prevent="saveProtectedExport">
          <PasswordInput v-model="protectedExportPassword" label="PDF password" autocomplete="new-password"/>
          <PasswordInput v-model="protectedExportConfirm" label="Confirm PDF password" autocomplete="new-password"/>
          <button class="btn primary" type="submit" :disabled="savingExport">{{savingExport?'Saving…':'Save protected PDF'}}</button>
          <button class="btn secondary" type="button" :disabled="savingExport" @click="cancelProtectedExport">Cancel</button>
        </form>
      </section>
    </div>
    <div v-if="protectedRestoreFile" class="vault-restore-overlay" role="presentation">
      <section class="vault-card" role="dialog" aria-modal="true" aria-labelledby="restore-vault-title">
        <h2 id="restore-vault-title">Unlock protected backup</h2>
        <p>This backup uses a different vault key. Enter the password or recovery key that protected it.</p>
        <p v-if="protectedRestoreError" class="notice amber" role="alert">{{protectedRestoreError}}</p>
        <form @submit.prevent="submitProtectedRestore">
          <label v-if="protectedRestoreRecovery">Recovery key<input v-model="protectedRestoreCredential" type="text" autocomplete="off" required/></label>
          <PasswordInput v-else v-model="protectedRestoreCredential" label="Backup password" autocomplete="off"/>
          <label class="vault-choice"><input v-model="protectedRestoreRecovery" type="checkbox"/> Use recovery key</label>
          <button class="btn primary" type="submit" :disabled="busy">Restore backup</button>
          <button class="btn secondary" type="button" @click="protectedRestoreFile=null;protectedRestoreCredential='';protectedRestoreError=''">Cancel</button>
        </form>
      </section>
    </div>
    <Transition name="modal"><MatterForm v-if="modalOpen" :profiles="profileList" :working="creating" @close="modalOpen=false" @create="createMatter"/></Transition>
    <Transition name="toast"><div v-if="toast" :class="['toast',toast.kind]" :role="toast.kind==='error'?'alert':'status'"><CheckCircle2 v-if="toast.kind==='success'" :size="19"/><CircleAlert v-else-if="toast.kind==='error'" :size="19"/><Info v-else :size="19"/><span>{{toast.message}}</span><button class="icon-btn small" aria-label="Dismiss notification" @click="toast=null"><X :size="15"/></button></div></Transition>
  </div>
</template>
