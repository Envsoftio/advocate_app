import { nextTick, onBeforeUnmount, ref, shallowRef } from 'vue'
import * as pdfjs from 'pdfjs-dist'
import workerSrc from './pdfWorker?worker&url'
import type { PDFDocumentProxy } from 'pdfjs-dist'
import { api, ApiError, saveReaderPosition } from './api'

pdfjs.GlobalWorkerOptions.workerSrc = workerSrc
export type ReaderMode = 'single' | 'continuous'
export type ReaderPosition = {page:number, y:number, x:number}
export type ReadingState = ReaderPosition & {zoom:string, mode:ReaderMode}
// Only navigation metadata is persisted, through the existing protected local store.
const positions = new Map<string, ReadingState>()
const pending = new Map<string, ReadingState>()
let saveTimer:ReturnType<typeof setTimeout>|undefined
let saving:Promise<void>|undefined
export const readingPositionError = ref('')
const persistedKey=/^(?:[0-9a-f]{32}:[1-9][0-9]*|export:[0-9a-f]{32}:[0-9a-f]{32}:[A-Za-z0-9_-]+\.pdf)$/
function validState(state:ReadingState):boolean {
  return Number.isInteger(state?.page) && state.page>0 && state.page<=100000 &&
    Number.isFinite(state.x) && Number.isFinite(state.y) && state.x>=0 && state.x<=1 && state.y>=0 && state.y<=1 &&
    ['single','continuous'].includes(state.mode) && ['fit-width','fit-page','25','50','75','100','125','150','200','300','400'].includes(state.zoom)
}
export function rememberReadingPosition(key:string, state:ReadingState) {
  if (!key) return
  state={...state,x:Math.max(0,Math.min(1,state.x)),y:Math.max(0,Math.min(1,state.y))}
  if(!validState(state))return
  const prior=positions.get(key)
  if(prior && JSON.stringify(prior)===JSON.stringify(state))return
  positions.delete(key)
  positions.set(key, {...state})
  if (positions.size > 100) positions.delete(positions.keys().next().value!)
  if(persistedKey.test(key)){
    pending.set(key,{...state})
    if(saveTimer)clearTimeout(saveTimer)
    saveTimer=setTimeout(()=>void flushReadingPositions(),750)
  }
}
export function recalledReadingPosition(key:string) { const state=positions.get(key);return state?{...state}:undefined }
export async function loadReadingPosition(key:string):Promise<ReadingState|undefined> {
  const cached=recalledReadingPosition(key)
  if(cached)return cached
  if(!persistedKey.test(key))return undefined
  try {
    const state=await api<ReadingState|null>(`/reader-position?key=${encodeURIComponent(key)}`)
    const recent=recalledReadingPosition(key)
    if(recent)return recent
    if(state && validState(state)){
      positions.set(key,{...state})
      if(positions.size>100)positions.delete(positions.keys().next().value!)
      return {...state}
    }
  }catch(error){
    if(!(error instanceof ApiError && [404,409].includes(error.status)))
      readingPositionError.value='Reading position could not be restored. The PDF is still available.'
  }
  return undefined
}
export function flushReadingPositions():Promise<void> {
  if(saveTimer){clearTimeout(saveTimer);saveTimer=undefined}
  if(saving)return saving.then(()=>flushReadingPositions())
  if(!pending.size)return Promise.resolve()
  const batch=[...pending];pending.clear()
  saving=(async()=>{
    for(const [key,state] of batch){
      try {await saveReaderPosition(key,state);readingPositionError.value=''}
      catch(error){if(!(error instanceof ApiError && [404,409].includes(error.status))){
        // Keep a failed save available for the next flush, without replacing a
        // newer position collected while this request was in flight.
        if(!pending.has(key))pending.set(key,state)
        readingPositionError.value='Reading position could not be saved. It will be remembered for this session.'
      }}
    }
  })().finally(()=>{saving=undefined})
  return saving
}
if(typeof window!=='undefined')window.addEventListener('pagehide',()=>void flushReadingPositions())

export function readerScale(zoom:string, width:number, height:number, pageWidth:number, pageHeight:number) {
  const fitWidth = Math.max(80, width - 40) / pageWidth
  if (zoom === 'fit-width') return Math.max(.05, Math.min(8, fitWidth))
  if (zoom === 'fit-page') return Math.max(.05, Math.min(8, fitWidth, Math.max(80, height - 40) / pageHeight))
  return Math.max(.1, Math.min(8, (Number(zoom) || 100) / 100 * 96 / 72))
}
export function pageAtOffset(offsets:number[], offset:number) {
  let lo = 0, hi = Math.max(0, offsets.length - 2)
  while (lo < hi) { const mid = Math.ceil((lo + hi) / 2); if (offsets[mid] <= offset) lo = mid; else hi = mid - 1 }
  return lo
}
export function nextZoom(zoom:string, scale:number, step:number) {
  const levels = [25, 50, 75, 100, 125, 150, 200, 300, 400]
  const pct = zoom.startsWith('fit-') ? scale * 72 / 96 * 100 : Number(zoom)
  return String(step > 0 ? levels.find(n => n > pct + .5) || 400 : [...levels].reverse().find(n => n < pct - .5) || 25)
}

export function usePdfDocument() {
  const pdf = shallowRef<PDFDocumentProxy | null>(null)
  const loading = ref(false)
  const error = ref('')
  let generation = 0
  let task: ReturnType<typeof pdfjs.getDocument> | null = null
  let teardown:Promise<void> = Promise.resolve()
  function close() {
    generation++
    const oldTask = task; task = null
    const old = pdf.value; pdf.value = null
    loading.value = false
    // Let page/text/search consumers cancel before terminating their worker.
    teardown=teardown.then(async()=>{
      await nextTick()
      if (oldTask) await oldTask.destroy().catch(() => {})
      else if (old) await old.destroy().catch(() => {})
    })
    return teardown
  }
  async function open(source:() => Promise<Parameters<typeof pdfjs.getDocument>[0]>) {
    const closing=close()
    const call = generation
    loading.value = true; error.value = ''
    try {
      await closing
      if(call!==generation)return null
      const input = await source()
      if (call !== generation) return null
      const pending = pdfjs.getDocument(input); task = pending
      const loaded = await pending.promise
      if (call !== generation) { void loaded.destroy().catch(() => {}); return null }
      pdf.value = loaded
      return loaded
    } catch (err) {
      if (call === generation) error.value = err instanceof Error ? err.message : String(err)
      return null
    } finally { if (call === generation) loading.value = false }
  }
  onBeforeUnmount(()=>{void close()})
  return {pdf, loading, error, open, close}
}
