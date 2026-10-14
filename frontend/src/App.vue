<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import {
  Archive, ArrowDownToLine, ArrowLeft, ArrowRight, BookMarked, BookOpen, BriefcaseBusiness,
  Check, CheckCircle2, ChevronRight, CircleAlert, CircleHelp, CloudOff,
  Ellipsis, FileCheck2, FilePenLine, FilePlus2, FileStack, FileText, FolderOpen, GripVertical, Info,
  Layers3, LockKeyhole, Menu, PanelLeftClose, PanelLeftOpen, Plus, RefreshCw,
  Redo2, ScanText, Search, Settings2, ShieldAlert, ShieldCheck,
  Undo2, UploadCloud, WandSparkles, X
} from 'lucide-vue-next'
import PdfViewer from './components/PdfViewer.vue'
import PageTools from './components/PageTools.vue'
import ManualBookmarks from './components/ManualBookmarks.vue'
import MatterForm from './components/MatterForm.vue'
import DelhiFilingGuide from './components/DelhiFilingGuide.vue'
import { openUrl } from '@tauri-apps/plugin-opener'
import { api, downloadExport, downloadMatter, inTauri, restoreMatter, uploadDocuments } from './lib/api'
import { bytesLabel, friendlyError, prettyDate } from './lib/format'
import { applyAppearance, readAppearance, type Theme, type UiScale } from './lib/appearance'
import type { Check as PreflightCheck, CourtProfile, DocKind, ExportManifest, Job, Matter, MatterDocument, MatterNew, PageSearchResult } from './lib/types'

type View = 'documents' | 'build' | 'validation' | 'rules' | 'about'
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
const history = ref({can_undo:false,can_redo:false,undo_count:0,redo_count:0})
const checks = ref<PreflightCheck[]>([])
const selectedDocId = ref<string | null>(null)
const selectedDoc = computed(() => docs.value.find(d => d.id === selectedDocId.value) || null)
const inspectorTab = ref<'details' | 'tools'>('details')
const detailsTabButton = ref<HTMLButtonElement | null>(null)
const toolsTabButton = ref<HTMLButtonElement | null>(null)
const pageToolsOpen = ref(false)
const pageToolsDialog = ref<HTMLDialogElement | null>(null)
const documentsWorkspace = ref<HTMLElement | null>(null)
const documentsWorkspaceHeight = ref<number | null>(null)
const activeView = ref<View>('documents')
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
const draggingDocId = ref<string | null>(null)
const hoverUpload = ref(false)
const apiStatus = ref<'connecting'|'online'|'offline'>('connecting')
const toast = ref<{message:string, kind:'success'|'error'|'info'} | null>(null)
const activeJob = ref<Job | null>(null)
const savingExport = ref(false)
const fileInput = ref<HTMLInputElement | null>(null)
const matterFileInput = ref<HTMLInputElement | null>(null)
const docEditor = reactive({ title: '', kind: 'other' as DocKind, part: 1, label: '', group_key: '' })
const matterEditor = reactive({max_pages_per_volume:300,keep_groups_intact:true})
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
const totalPages = computed(() => docs.value.reduce((n,d) => n+d.page_count, 0))
const documentKindOptions: DocKind[] = ['petition','affidavit','order','annexure','application','evidence','other']
const documentDirty = computed(() => !!selectedDoc.value && (
  docEditor.title !== selectedDoc.value.title || docEditor.kind !== selectedDoc.value.kind ||
  docEditor.part !== selectedDoc.value.part || docEditor.label !== selectedDoc.value.label ||
  docEditor.group_key !== selectedDoc.value.group_key))
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
  const matter = currentMatter.value
  matterEditor.max_pages_per_volume = matter?.max_pages_per_volume || 300
  matterEditor.keep_groups_intact = matter?.keep_groups_intact ?? true
}
function confirmDiscardDrafts() {
  if (!documentDirty.value && !settingsDirty.value) return true
  if (!window.confirm('Discard unsaved document details or bundle settings?')) return false
  resetDrafts()
  return true
}
function selectDocument(id: string) {
  if (selectedDocId.value === id || !confirmDiscardDrafts()) return
  pageTarget.value = {documentId:'', page:1, sequence:pageTarget.value.sequence + 1}
  selectedDocId.value = id
}
watch(selectedDocId, () => { inspectorTab.value = 'details'; pageToolsOpen.value = false })
function fitDocumentsWorkspace() {
  const workspace = documentsWorkspace.value
  if (!workspace || window.innerWidth <= 1120) {
    documentsWorkspaceHeight.value = null
    return
  }
  const scale = uiScale.value / 100
  documentsWorkspaceHeight.value = Math.max(320, Math.floor((window.innerHeight - workspace.getBoundingClientRect().top - 8) / scale))
}
watch([uiScale, activeView, currentMatterId], async () => {
  await nextTick()
  window.requestAnimationFrame(fitDocumentsWorkspace)
})
watch(pageToolsOpen, async open => {
  await nextTick()
  const dialog = pageToolsDialog.value
  if (open && dialog && !dialog.open) dialog.showModal()
  if (!open && dialog?.open) dialog.close()
})
function onInspectorTabKeydown(event: KeyboardEvent) {
  if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return
  event.preventDefault()
  inspectorTab.value = event.key === 'ArrowLeft' || event.key === 'Home' ? 'details' : 'tools'
  nextTick(() => (inspectorTab.value === 'details' ? detailsTabButton.value : toolsTabButton.value)?.focus())
}
function showView(view: View) {
  if (activeView.value === view) { mobileNavOpen.value = false; return }
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
  if (event.key === 'Escape' && mobileNavOpen.value) { mobileNavOpen.value = false; return }
  if (event.defaultPrevented || modalOpen.value || event.altKey) return
  const target = event.target as HTMLElement | null
  const editingField = target?.isContentEditable || !!target?.closest('input, textarea, select, [contenteditable="true"]')
  if (event.key === 'Escape' && toast.value) { toast.value = null; return }
  if (editingField) return
  if (event.key === '/' || ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k')) {
    event.preventDefault()
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
    docEditor.group_key !== previous.group_key)) return
  docEditor.title = doc?.title || ''
  docEditor.kind = doc?.kind || 'other'
  docEditor.part = doc?.part || 1
  docEditor.label = doc?.label || ''
  docEditor.group_key = doc?.group_key || ''
}, {immediate:true})
watch(currentMatter, (item, previous) => {
  if (item && previous?.id === item.id && (
    matterEditor.max_pages_per_volume !== previous.max_pages_per_volume ||
    matterEditor.keep_groups_intact !== previous.keep_groups_intact)) return
  matterEditor.max_pages_per_volume = item?.max_pages_per_volume || 300
  matterEditor.keep_groups_intact = item?.keep_groups_intact ?? true
}, {immediate:true})
watch(currentMatterId, () => {
  pageQuery.value = ''
  pageSearch.value = null
  searchSequence++
  if (currentMatterId.value) void loadMatterResources()
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
  toastTimer=setTimeout(()=>toast.value=null,6500)
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

async function init() {
  if (connecting) return
  connecting = true
  apiStatus.value = 'connecting'
  try {
    let connected = false
    const deadline = Date.now() + (inTauri() ? 30_000 : 12_000)
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
    profileList.value = await api<CourtProfile[]>('/profiles')
    await refreshMatters()
    apiStatus.value = 'online'
  } catch (err) { apiStatus.value='offline'; issue(err) }
  finally { connecting = false }
}
async function checkHealth() {
  if (connecting) return
  try {
    await api('/health', 'GET', undefined, 1_500)
    if (apiStatus.value === 'offline') await init()
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
    const [d, c, e, j, h, archived, usage] = await Promise.all([
      api<MatterDocument[]>(`/matters/${id}/documents`),
      api<PreflightCheck[]>(`/matters/${id}/validate`),
      api<ExportManifest[]>(`/matters/${id}/exports`),
      api<Job[]>(`/matters/${id}/jobs`),
      api<typeof history.value>(`/matters/${id}/history`),
      api<MatterDocument[]>(`/matters/${id}/archived-documents`),
      api<typeof storageUsage.value>(`/matters/${id}/storage`),
    ])
    if (currentMatterId.value !== id) return
    docs.value=d; checks.value=c; exportsList.value=e; recentJobs.value=j; history.value=h
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
  busy.value = true
  try {
    await api(`/matters/${currentMatterId.value}/history/${direction}`,'POST')
    await refreshMatters()
    await loadMatterResources()
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
    notify('Matter created. Import your first document.', 'success')
  } catch(err) { issue(err) }
  finally { creating.value=false }
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
    await refreshMatters()
    currentMatterId.value = restored.id
    selectedDocId.value = null
    activeView.value = 'documents'
    notify('Matter restored with originals and working revisions.', 'success')
  } catch (err) { issue(err) }
  finally { busy.value = false; if (matterFileInput.value) matterFileInput.value.value = '' }
}
async function onUpload(files: FileList | File[]) {
  if (!currentMatterId.value || busy.value) return
  const incoming=Array.from(files).filter(f => /\.(pdf|png|jpe?g)$/i.test(f.name))
  if (!incoming.length) { notify('Choose PDF, JPG or PNG files.', 'error'); return }
  busy.value=true
  try {
    await uploadDocuments(currentMatterId.value, incoming)
    await loadMatterResources()
    notify(`${incoming.length} document(s) imported. Original files preserved.`, 'success')
  } catch(err) { issue(err); await loadMatterResources() }
  finally { busy.value=false; if(fileInput.value) fileInput.value.value='' }
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
    const updated=await api<MatterDocument>(`/documents/${selectedDoc.value.id}`,'PATCH',{...docEditor})
    docs.value=docs.value.map(x=>x.id===updated.id ? updated : x)
    await refreshHistory()
    await validate()
    await loadMatterResources()
    notify('Document details updated.', 'success')
  } catch(err){issue(err)} finally{busy.value=false}
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
function onDragStart(docId:string){ draggingDocId.value=docId }
async function onDropOn(targetId:string) {
  const sourceId=draggingDocId.value
  draggingDocId.value=null
  if(!currentMatterId.value || !sourceId || sourceId===targetId) return
  const ids=docs.value.map(d=>d.id)
  ids.splice(ids.indexOf(sourceId),1)
  ids.splice(ids.indexOf(targetId),0,sourceId)
  try {
    docs.value=await api<MatterDocument[]>(`/matters/${currentMatterId.value}/documents/reorder`,'POST',{ids})
    await refreshHistory()
    await validate()
    await loadMatterResources()
  } catch(err){issue(err); await loadMatterResources()}
}
async function moveDocument(direction: number) {
  if (!selectedDoc.value || !currentMatterId.value) return
  const ids=docs.value.map(d=>d.id); const position=ids.indexOf(selectedDoc.value.id)
  const next=position+direction
  if(next<0 || next>=ids.length) return
  ;[ids[position],ids[next]]=[ids[next],ids[position]]
  try {
    docs.value=await api<MatterDocument[]>(`/matters/${currentMatterId.value}/documents/reorder`,'POST',{ids})
    await refreshHistory()
    await validate()
    await loadMatterResources()
  } catch(err){issue(err)}
}
async function refreshDocument() { await loadMatterResources() }
async function startOcr() {
  if(!selectedDoc.value || isJobActive.value) return
  try {
    const job=await api<Job>(`/documents/${selectedDoc.value.id}/ocr`,'POST')
    watchJob(job)
    notify('OCR started on scanned and mixed-content pages.', 'info')
  } catch(err){issue(err)}
}
async function validate() {
  if(!currentMatterId.value) return
  try { checks.value=await api<PreflightCheck[]>(`/matters/${currentMatterId.value}/validate`) }
  catch(err){issue(err)}
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
async function startBuild() {
  if (!currentMatterId.value || isJobActive.value) return
  try {
    if (!await savePolicy()) return
    const job=await api<Job>(`/matters/${currentMatterId.value}/build`,'POST')
    watchJob(job)
  } catch(err){issue(err)}
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
    const retry = await api<Job>(`/jobs/${job.id}/retry`, 'POST')
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
function selectMatter(id:string){
  if (currentMatterId.value === id) { mobileNavOpen.value = false; return }
  if (!confirmDiscardDrafts()) return
  currentMatterId.value=id;selectedDocId.value=null;activeView.value='documents'
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
  <div class="app-shell">
    <a class="skip-link" href="#main-content">Skip to main content</a>
    <button v-if="smallScreen && mobileNavOpen" class="sidebar-backdrop" aria-label="Close navigation" @click="mobileNavOpen=false"></button>
    <aside class="sidebar" :class="{collapsed:sidebarCollapsed && !smallScreen, 'mobile-open':mobileNavOpen}" :inert="smallScreen && !mobileNavOpen">
      <div class="brand-area">
        <div class="brand-mark"><Layers3 :size="22" :stroke-width="2.3"/></div>
        <div class="brand-copy"><div class="brand-name">CourtBundle<span class="brand-plus">.</span></div><div class="brand-sub">LEGAL WORKSPACE / POC</div></div>
      </div>
      <div class="side-label">WORKSPACE</div>
      <nav class="side-nav">
        <button class="side-link" aria-label="Matters" title="Matters" :class="{active:activeView!=='rules'&&activeView!=='about'}" @click="showView('documents')"><BriefcaseBusiness :size="18"/><span class="nav-label">Matters</span><span class="side-count">{{matterList.length}}</span></button>
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
        <div class="topbar-left"><button class="icon-btn nav-toggle" :aria-label="smallScreen ? (mobileNavOpen ? 'Close navigation' : 'Open navigation') : (sidebarCollapsed ? 'Expand sidebar' : 'Collapse sidebar')" :aria-expanded="smallScreen ? mobileNavOpen : !sidebarCollapsed" @click="toggleNavigation"><Menu v-if="smallScreen" :size="20"/><PanelLeftOpen v-else-if="sidebarCollapsed" :size="20"/><PanelLeftClose v-else :size="20"/></button><div class="breadcrumb"><span>Workspace</span><ChevronRight :size="14"/><strong>{{activeView==='rules'?'Court rules':activeView==='about'?'About POC':(currentMatter?.name||'Matters')}}</strong></div></div>
        <div class="topbar-right"><span class="desktop-tag">DESKTOP POC</span><span class="user-badge">CB</span></div>
      </header>

      <div v-if="apiStatus==='offline'" class="backend-error"><CircleAlert :size="19"/><div><strong>Python engine unavailable</strong><p v-if="inTauri()">Quit and reopen CourtBundle to start a new local engine session. If it still fails, inspect the packaged sidecar and local port.</p><p v-else>Run <code>cd backend && ./dev.sh</code>, then retry. The browser development token is for local testing only.</p></div><button class="btn secondary" @click="init">Retry connection</button></div>

      <main v-if="activeView==='rules'" id="main-content" tabindex="-1" class="page-content rules-page">
        <div class="page-eyebrow">LEGAL KNOWLEDGE</div><h1>Court rules & technical profiles</h1><p class="page-description">Source-linked preparation checks. These do not replace registry instructions or an advocate's review.</p>
        <div class="rules-grid"><div v-for="profile in profileList" :key="profile.id" class="card rule-card">
          <div class="rule-icon"><BookMarked :size="24" /></div><div class="badge muted">{{profile.status==='DEMO_ONLY'?'DEMO ONLY':'NOT LEGALLY APPROVED'}}</div>
          <h2>{{profile.name}}</h2><p>{{profile.description}}</p>
          <div class="rule-facts"><div><span>File size setting</span><strong>{{profile.max_file_mb ? profile.max_file_mb+' MB' : 'None'}}</strong></div><div><span>Searchable text check</span><strong>{{profile.require_searchable_text?'Enabled':'Off'}}</strong></div><div><span>Source review date</span><strong>{{profile.last_checked}}</strong></div></div>
          <div class="rule-sources" v-if="profile.sources.length"><span>OFFICIAL REFERENCE LINKS</span><a v-for="source in profile.sources" :key="source.url" :href="source.url" target="_blank" rel="noopener noreferrer" @click.prevent="openOfficialReference(source.url)">{{source.title}} <ArrowRight :size="13"/></a></div>
        </div></div>
        <DelhiFilingGuide v-if="delhiGuidance" :guidance="delhiGuidance" :matter-id="currentMatterId" @open-url="openOfficialReference"/>
        <div class="notice amber"><ShieldAlert :size="19"/> These rules are only a selected technical subset. Case-type documents, notarisation, signatures, fees, limitation and court acceptance are outside the POC's validation scope.</div>
      </main>

      <main v-else-if="activeView==='about'" id="main-content" tabindex="-1" class="page-content about-page">
        <div class="page-eyebrow">PRODUCT DEMONSTRATION</div><h1>Built for structured legal filing preparation.</h1>
        <p class="page-description">A local-first proof of concept connecting Tauri, Vue and Python to convert source documents into indexed legal bundles.</p>
        <div class="feature-strip"><div><FileStack :size="23"/><h3>Preserve originals</h3><p>Working copies are edited separately from imported source documents.</p></div><div><WandSparkles :size="23"/><h3>Deterministic assembly</h3><p>Volume splitting, bookmarks and indexes share the same page map.</p></div><div><ShieldCheck :size="23"/><h3>Visible limitations</h3><p>Illustrative rules and a validation report, not a filing approval.</p></div></div>
        <div class="card about-card"><h2>What is included?</h2><p>Matters, PDF/JPEG/PNG import, working-copy page operations, English OCR, drag-drop ordering, annexure/part metadata, auto index, volume splitting, linked bookmarks, technical validation and ZIP export.</p><h2>What is not included?</h2><p>Actual court submission, legally approved templates, digital signing, Word/Excel/PPT conversion, comprehensive PDF text editing, cloud sharing or guaranteed legal compliance.</p></div>
      </main>

      <main v-else-if="!currentMatter" id="main-content" tabindex="-1" class="page-content welcome-page">
        <div class="welcome-icon"><FolderOpen :size="32"/></div><div class="page-eyebrow">YOUR FIRST MATTER</div><h1>From scattered files to<br/>a structured court bundle.</h1>
        <p>Create a matter to import documents, organise annexures, generate indexes and prepare a complete PDF package—offline.</p>
        <button class="btn primary large" @click="modalOpen=true"><Plus :size="18"/> Create your first matter</button>
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
        <div class="workspace-tabs"><button :class="{active:activeView==='documents'}" @click="showView('documents')"><FileStack :size="16"/> Documents</button><button :class="{active:activeView==='build'}" @click="showView('build')"><Layers3 :size="16"/> Build bundle</button><button :class="{active:activeView==='validation'}" @click="showView('validation')"><ShieldCheck :size="16"/> Preflight checks <span v-if="warningChecks.length+errorChecks.length" class="count-bubble">{{warningChecks.length+errorChecks.length}}</span></button></div>

        <div v-if="activeView==='documents'" ref="documentsWorkspace" class="documents-workspace" :style="documentsWorkspaceHeight ? {height:documentsWorkspaceHeight+'px'} : undefined">
          <section class="panel docs-panel">
            <div class="panel-header"><div><h2>Source documents</h2><span>Reorder documents by dragging</span></div><button class="icon-btn" title="Generate draft cover sheet" :disabled="busy" @click="generateCover"><FilePlus2 :size="19"/></button></div>
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
            <div v-if="docs.length" class="docs-list">
              <div v-for="doc in filteredDocs" :key="doc.id" class="doc-item" :class="{selected:selectedDocId===doc.id,dragging:draggingDocId===doc.id}" :draggable="!busy" @dragstart="onDragStart(doc.id)" @dragend="draggingDocId=null" @dragover.prevent @drop.prevent="onDropOn(doc.id)" @click="selectDocument(doc.id)">
                <GripVertical :size="16" class="grip"/>
                <div class="doc-icon"><FileText :size="20"/></div>
                <div class="doc-info"><strong :title="doc.title">{{doc.title}}</strong><span><span v-if="doc.label" class="doc-label">{{doc.label}} · </span>Part {{doc.part}} · {{doc.page_count}} {{doc.page_count===1?'page':'pages'}}</span></div>
                <ChevronRight :size="16" class="doc-arrow"/>
              </div>
            </div>
            <div v-else-if="docs.length" class="no-matching">No matching documents</div>
            <div v-else class="no-documents"><FileStack :size="30"/><strong>No documents added</strong><small>Import a PDF or scanned image to begin.</small></div>
            <div v-if="archivedDocs.length" class="docs-help"><Archive :size="14"/> Archived documents</div>
            <div v-for="doc in archivedDocs" :key="doc.id" class="export-file"><FileText :size="16"/><div><strong>{{doc.title}}</strong><small>{{doc.original_name}}</small></div><button class="text-tool" @click="restoreArchivedDocument(doc.id)">Restore</button></div>
            <div class="upload-zone" :class="{over:hoverUpload}" @dragover.prevent="hoverUpload=true" @dragleave="hoverUpload=false" @drop.prevent="onDropUpload"><input ref="fileInput" type="file" accept=".pdf,.jpg,.jpeg,.png" multiple hidden @change="e=>onUpload((e.target as HTMLInputElement).files || [])"/><UploadCloud :size="25"/><strong>Drop files here</strong><span>PDF, JPG, PNG · up to 80 MB/file</span><button class="btn secondary small" :disabled="busy" @click="fileInput?.click()">{{busy?'Importing…':'Browse files'}}</button></div>
            <div class="docs-help"><LockKeyhole :size="14"/> Original files are never overwritten.</div>
          </section>

          <section class="panel preview-panel"><PdfViewer :document="selectedDoc" :disabled="busy || isJobActive || documentDirty" :page-target="pageTarget" @modified="refreshDocument" @error="issue" @page-tools="pageToolsOpen=true"/></section>

          <section class="panel inspector-panel">
            <div class="panel-header"><div><h2>{{inspectorTab==='tools'?'Document tools':'Document details'}}</h2><span>{{inspectorTab==='tools'?'OCR and page editing':'Metadata for collation'}}</span></div><Settings2 :size="18" class="muted-icon"/></div>
            <template v-if="selectedDoc">
              <div class="inspector-tabs" role="tablist" aria-label="Document panel" @keydown="onInspectorTabKeydown">
                <button ref="detailsTabButton" id="document-details-tab" role="tab" aria-controls="document-details-panel" :aria-selected="inspectorTab==='details'" :tabindex="inspectorTab==='details'?0:-1" :class="{active:inspectorTab==='details'}" @click="inspectorTab='details'">Details</button>
                <button ref="toolsTabButton" id="document-tools-tab" role="tab" aria-controls="document-tools-panel" :aria-selected="inspectorTab==='tools'" :tabindex="inspectorTab==='tools'?0:-1" :class="{active:inspectorTab==='tools'}" @click="inspectorTab='tools'">Tools</button>
              </div>
              <div v-if="inspectorTab==='details'" id="document-details-panel" class="inspector-content" role="tabpanel" aria-labelledby="document-details-tab">
                <div class="inspector-heading"><div class="doc-icon big"><FileText :size="22"/></div><div><strong>{{selectedDoc.original_name}}</strong><small>{{selectedDoc.page_count}} pages · {{selectedDoc.ocr_status.split('_').join(' ')}}</small></div></div>
                <label class="field">Display title<input v-model="docEditor.title" maxlength="250"/></label>
                <label class="field">Document type<select v-model="docEditor.kind"><option v-for="kind in documentKindOptions" :key="kind" :value="kind">{{kind[0].toUpperCase()+kind.slice(1)}}</option></select></label>
                <div class="field-row"><label class="field">Part<select v-model.number="docEditor.part"><option v-for="part in 4" :value="part">Part {{part}}</option></select></label><label class="field">Label<input v-model="docEditor.label" placeholder="P-1" maxlength="60"/></label></div>
                <label class="field">Keep-together group<input v-model="docEditor.group_key" placeholder="Optional: annexures-a"/><span class="hint">Adjacent documents with the same group stay together when enabled.</span></label>
                <p v-if="documentDirty" role="status" class="hint">Unsaved document changes</p>
                <button class="btn primary block" :disabled="busy || !docEditor.title.trim()" @click="saveDocument"><Check :size="16"/> Save document details</button>
              </div>
              <div v-else id="document-tools-panel" class="inspector-content" role="tabpanel" aria-labelledby="document-tools-tab">
                <p v-if="documentDirty" class="hint">Save document details before editing pages.</p>
                <div class="action-caption">DOCUMENT TOOLS</div>
                <button class="action-row" :disabled="isJobActive" @click="startOcr"><ScanText :size="17"/><div><strong>Run English OCR</strong><small>Make scanned pages searchable</small></div><ChevronRight :size="16"/></button>
                <button class="action-row" :disabled="busy" @click="moveDocument(-1)"><ArrowLeft :size="17"/><div><strong>Move document up</strong><small>Adjust bundle order</small></div></button>
                <button class="action-row" :disabled="busy" @click="moveDocument(1)"><ArrowRight :size="17"/><div><strong>Move document down</strong><small>Adjust bundle order</small></div></button>
                <button class="action-row danger" :disabled="busy" @click="deleteDocument"><Archive :size="17"/><div><strong>Archive document</strong><small>Restore it from this matter</small></div></button>
                <button class="action-row" :disabled="busy || isJobActive" @click="pageToolsOpen=true"><FilePenLine :size="17"/><div><strong>Insert, merge, replace or crop pages</strong><small>Opens tools for this document</small></div><ChevronRight :size="16"/></button>
              </div>
            </template>
            <div v-else class="inspector-empty">Select a document to manage its details and OCR.</div>
          </section>
        </div>

        <div v-else-if="activeView==='build'" class="build-layout">
          <div class="build-main">
            <div class="panel build-panel"><div class="panel-header"><div><h2>Collation & export settings</h2><span>Configured per matter—not universal legal rules</span></div><Settings2 :size="18" class="muted-icon"/></div>
              <div class="build-body"><label class="field">Maximum pages per volume <span class="help-inline">(including index pages)</span><input v-model.number="matterEditor.max_pages_per_volume" type="number" min="2" max="2000"/></label><p class="setting-description">Client-configured document packaging limit. It is not a nationwide Indian court requirement.</p>
                <label class="checkbox-line"><input v-model="matterEditor.keep_groups_intact" type="checkbox"/><div><strong>Keep document groups together</strong><span>Grouped adjacent documents must fit in the same volume.</span></div></label>
                <button class="btn secondary" :disabled="!settingsDirty || isJobActive" @click="savePolicy"><Check :size="16"/> Save bundle settings</button>
                <div class="checklist"><div><CheckCircle2 :size="17"/> Generate index pages in every volume</div><div><CheckCircle2 :size="17"/> Include clickable index links and PDF bookmarks</div><div><CheckCircle2 :size="17"/> Generate combined PDF + separate volumes</div><div><CheckCircle2 :size="17"/> Archive in ZIP with validation report</div></div>
                <div class="notice amber"><ShieldAlert :size="18"/><span>Working-copy PDF annotation objects and link actions are omitted from volume and combined PDFs; flattened marks in page content remain. Generated index links and bookmarks remain. Review validation warnings for affected documents; signature integrity and filing acceptance are not certified.</span></div>
                <p v-if="settingsDirty" role="status" class="hint">Unsaved bundle settings. Building saves these settings first.</p>
                <button class="btn primary large" :disabled="!docs.length || isJobActive" @click="startBuild"><Layers3 :size="19"/> {{isJobActive?'Processing…':'Build filing bundle'}}</button>
                <div v-if="isJobActive && activeJob" class="job-progress"><div class="job-progress-line"><span>{{activeJob.message}}</span><strong>{{activeJob.progress}}%</strong></div><div class="progress-track"><div :style="{width:activeJob.progress+'%'}"></div></div><button class="text-tool danger" @click="cancelJob">Cancel operation</button></div>
                <div v-else-if="activeJob?.status==='failed'" class="notice error"><CircleAlert :size="18"/> {{activeJob.error}}</div>
                <div v-for="job in interruptedJobs" :key="job.id" class="notice amber"><CircleAlert :size="18"/><span>{{job.kind === 'ocr' ? 'OCR' : job.kind === 'compress' ? 'Compression' : 'Bundle build'}} was interrupted by an engine restart. Review the current revision and exports before retrying.</span><span v-if="job.kind === 'compress'">Choose compression again in page tools.</span><button v-else class="text-tool" :disabled="isJobActive" @click="retryInterrupted(job)">Retry</button></div>
              </div>
            </div>
            <ManualBookmarks :key="`${currentMatterId}-${history.undo_count}-${history.redo_count}`" :matter-id="currentMatterId!" :documents="docs" :disabled="busy || isJobActive" @modified="loadMatterResources"/>
            <div class="panel exports-panel"><div class="panel-header"><div><h2>Generated exports</h2><span>Stored locally for repeat downloads</span></div><Archive :size="18" class="muted-icon"/></div>
              <div v-if="activeExport" class="export-body"><div v-if="activeExport.stale" class="notice amber"><ShieldAlert :size="18"/> Matter inputs changed since this export. Rebuild before using it.</div><div v-if="activeJob?.kind==='build' && activeJob.status==='failed'" class="notice amber"><ShieldAlert :size="18"/> The latest build failed. These files are from an earlier completed export; review them before use.</div><div class="export-summary"><div class="export-icon"><FileCheck2 :size="25"/></div><div><strong>{{activeJob?.kind==='build' && activeJob.status==='failed' ? 'Previous filing package' : 'Latest filing package'}}</strong><span>{{prettyDate(activeExport.created_at)}} · {{activeExport.volumes}} volume(s) · {{activeExport.pages}} pages</span></div></div><div v-if="activeExport.checks.some(check => check.code==='SOURCE_ANNOTATIONS_OMITTED' || check.code==='SOURCE_LINKS_OMITTED')" class="notice amber"><ShieldAlert :size="18"/> This export omitted source annotations or links. See validation-report.json for affected documents; generated index links remain.</div>
                <div v-for="file in activeExport.files" :key="file.name" class="export-file"><FileText :size="18"/><div><strong>{{file.name}}</strong><small>{{bytesLabel(file.bytes)}}</small></div><button class="icon-btn" :disabled="savingExport" :title="inTauri() ? 'Save '+file.name : 'Download '+file.name" :aria-label="inTauri() ? 'Save '+file.name : 'Download '+file.name" @click="download(activeExport.id,file.name)"><ArrowDownToLine :size="18"/></button><button v-if="inTauri()" class="text-tool" :disabled="savingExport" :title="'Save '+file.name+' as'" @click="download(activeExport.id,file.name,true)">Save As</button></div>
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
      </main>
    </div>
    <dialog v-if="selectedDoc" ref="pageToolsDialog" class="page-tools-dialog" aria-labelledby="page-tools-title" @close="pageToolsOpen=false" @click.self="pageToolsOpen=false">
      <div class="page-tools-dialog-head"><div><h2 id="page-tools-title">Page tools</h2><p>{{selectedDoc.title}} · Working copy</p></div><button class="icon-btn" aria-label="Close page tools" @click="pageToolsOpen=false"><X :size="18"/></button></div>
      <p v-if="documentDirty" class="notice amber">Save document details before editing pages.</p>
      <PageTools :document="selectedDoc" :documents="docs" :disabled="busy || isJobActive || documentDirty" :job="activeJob" expanded @modified="refreshDocument" @job-started="watchJob" @cancel="cancelJob"/>
    </dialog>
    <MatterForm v-if="modalOpen" :profiles="profileList" :working="creating" @close="modalOpen=false" @create="createMatter"/>
    <div v-if="toast" :class="['toast',toast.kind]"><CheckCircle2 v-if="toast.kind==='success'" :size="19"/><CircleAlert v-else-if="toast.kind==='error'" :size="19"/><Info v-else :size="19"/><span>{{toast.message}}</span><button class="icon-btn small" @click="toast=null"><X :size="15"/></button></div>
  </div>
</template>
