import { invoke } from '@tauri-apps/api/core'
import type { Matter } from './types'

type BackendConnection = { base_url: string, token: string }
let sessionPromise: Promise<BackendConnection> | undefined
const REQUEST_TIMEOUT_MS = 30_000

export function inTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

async function getConnection(): Promise<BackendConnection> {
  if (!sessionPromise) {
    sessionPromise = inTauri()
      ? invoke<BackendConnection>('get_backend_connection')
      : Promise.resolve({
          base_url: import.meta.env.VITE_API_BASE || 'http://127.0.0.1:8765',
          token: import.meta.env.VITE_DEV_TOKEN || 'development-token-change-me',
        })
  }
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      sessionPromise,
      new Promise<BackendConnection>((_resolve, reject) => {
        timer = setTimeout(() => reject(new ApiError(0, 'Cannot obtain the local engine session. Quit and reopen CourtBundle.')), 3_000)
      }),
    ])
  } catch (err) {
    sessionPromise = undefined
    throw err
  } finally {
    if (timer) clearTimeout(timer)
  }
}

export class ApiError extends Error {
  constructor(public status: number, message: string) { super(message) }
}

async function request(path: string, init?: RequestInit, timeoutMs = REQUEST_TIMEOUT_MS): Promise<Response> {
  const conn = await getConnection()
  const headers = new Headers(init?.headers)
  headers.set('Authorization', 'Bearer ' + conn.token)
  if (typeof init?.body === 'string') headers.set('Content-Type', 'application/json')
  let res: Response
  const controller = new AbortController()
  if (init?.signal?.aborted) controller.abort()
  const abortFromCaller = () => controller.abort()
  init?.signal?.addEventListener('abort', abortFromCaller, { once: true })
  const timeout = setTimeout(() => controller.abort(), timeoutMs)
  try {
    res = await fetch(conn.base_url + path, { ...init, headers, signal: controller.signal })
  } catch (err) {
    if (controller.signal.aborted) throw new ApiError(0, 'Local Python service timed out. Check the engine and retry.')
    throw new ApiError(0, 'Local Python service unavailable. Check it is running and retry. ' + String(err))
  } finally {
    clearTimeout(timeout)
    init?.signal?.removeEventListener('abort', abortFromCaller)
  }
  if (!res.ok) {
    let detail = res.statusText
    try {
      const body = await res.json()
      if (typeof body.detail === 'string') detail = body.detail
      else if (Array.isArray(body.detail)) {
        detail = body.detail.map((item: {loc?: unknown, msg?: unknown}) => {
          const field = Array.isArray(item.loc) ? item.loc.filter(part => part !== 'body').join('.') : ''
          const message = typeof item.msg === 'string' ? item.msg : 'Invalid value'
          return field ? `${field}: ${message}` : message
        }).join('; ') || 'Invalid request'
      } else if (body.detail != null) detail = String(body.detail)
    }
    catch { /* retain status */ }
    throw new ApiError(res.status, detail)
  }
  return res
}

export async function api<T>(path: string, method = 'GET', body?: unknown, timeoutMs?: number,
                             headers?: HeadersInit): Promise<T> {
  const options: RequestInit = { method, headers }
  if (body !== undefined) options.body = JSON.stringify(body)
  const response = await request(path, options, timeoutMs)
  return response.json() as Promise<T>
}

export async function saveReaderPosition(key:string, state:import('./pdfReader').ReadingState) {
  // Keep a final metadata-only save alive when the reader/window closes.
  await request('/reader-position', {method:'PUT',body:JSON.stringify({key,...state}),keepalive:true})
}

export async function uploadDocument(matterId: string, file: File, password?: string,
                                     acknowledgeSignatureRisk = false): Promise<void> {
  const headers: Record<string,string> = {
    'Content-Type': 'application/octet-stream',
    'X-CourtBundle-Filename': encodeURIComponent(file.name),
    'X-CourtBundle-File-Mtime': file.lastModified ? new Date(file.lastModified).toISOString() : '',
  }
  if (password) headers['X-CourtBundle-Pdf-Password'] = encodeURIComponent(password)
  if (acknowledgeSignatureRisk) headers['X-CourtBundle-Signature-Acknowledge'] = 'true'
  await request(`/matters/${matterId}/documents/raw`, { method: 'POST', body: file, headers }, 5 * 60_000)
}

export async function uploadDocuments(matterId: string, files: File[]): Promise<void> {
  for (const file of files) await uploadDocument(matterId, file)
}

export async function restoreMatter(file: File, credential?: string, recovery = false): Promise<Matter> {
  const headers: Record<string, string> = { 'Content-Type': 'application/octet-stream' }
  if (credential) headers['X-CourtBundle-Backup-Credential'] = credential
  if (recovery) headers['X-CourtBundle-Backup-Recovery'] = 'true'
  const response = await request('/matters/restore', { method: 'POST', body: file, headers }, 5 * 60_000)
  return response.json() as Promise<Matter>
}

export async function downloadMatter(matterId: string): Promise<boolean> {
  if (inTauri()) return invoke<boolean>('save_matter', { matterId })
  const response = await request(`/matters/${matterId}/portable`, undefined, 5 * 60_000)
  const url = URL.createObjectURL(await response.blob())
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = 'court-matter.cbmatter'
  document.body.appendChild(anchor)
  anchor.click()
  anchor.remove()
  setTimeout(() => URL.revokeObjectURL(url), 30_000)
  return true
}

export async function downloadPageOutput(documentId: string, operation: 'extract' | 'split', indexes: number[],
                                         acknowledgeSignatureRisk = false): Promise<boolean> {
  const selection = indexes.join(',')
  if (inTauri()) return invoke<boolean>('save_page_output', {
    documentId, operation, selection, acknowledgeSignatureRisk,
  })
  const query = operation === 'extract' ? 'selection' : 'after'
  const response = await request(`/documents/${documentId}/pages/${operation}?${query}=${selection}` +
    `&acknowledge_signature_risk=${acknowledgeSignatureRisk}`)
  const url = URL.createObjectURL(await response.blob())
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = operation === 'extract' ? 'extracted-pages.pdf' : 'split-pages.zip'
  document.body.appendChild(anchor)
  anchor.click()
  anchor.remove()
  setTimeout(() => URL.revokeObjectURL(url), 30_000)
  return true
}

export async function pdfBytes(docId: string, revision: number, signal?: AbortSignal): Promise<Uint8Array> {
  const response = await request(`/documents/${docId}/pdf?revision=${revision}`, { cache: 'no-store', signal })
  return new Uint8Array(await response.arrayBuffer())
}

/** Save a PDF snapshot with resolved editable marks flattened; private notes stay out. */
export async function downloadWorkingPdf(documentId: string, revision: number, title: string,
    acknowledgeSignatureRisk = false): Promise<boolean> {
  if (!Number.isSafeInteger(revision) || revision < 1) throw new ApiError(0, 'Choose a current working PDF revision to save.')
  const stem = title.replace(/\.pdf$/i, '').replace(/[^a-zA-Z0-9_-]+/g, '-')
    .replace(/^-+|-+$/g, '').slice(0, 80) || 'document'
  const filename = `${stem}-working-r${revision}.pdf`
  if (inTauri()) return invoke<boolean>('save_working_pdf', { documentId, expectedRevision: revision, filename,
    acknowledgeSignatureRisk })
  const response = await request(`/documents/${encodeURIComponent(documentId)}/pdf?revision=${revision}&editable=true&acknowledge_signature_risk=${acknowledgeSignatureRisk}`,
    { cache: 'no-store' }, 5 * 60_000)
  const url = URL.createObjectURL(await response.blob())
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = filename
  document.body.appendChild(anchor)
  anchor.click()
  anchor.remove()
  setTimeout(() => URL.revokeObjectURL(url), 30_000)
  return true
}

export async function documentThumbnailBlob(docId: string, revision: number,
                                            signal?: AbortSignal): Promise<Blob> {
  const response = await request(`/documents/${encodeURIComponent(docId)}/thumbnail?revision=${revision}`,
    { cache: 'no-store', signal })
  return response.blob()
}

export async function pdfDocumentSource(docId: string, revision: number) {
  const connection = await getConnection()
  return {
    url: `${connection.base_url}/documents/${encodeURIComponent(docId)}/pdf?revision=${revision}`,
    httpHeaders: {Authorization: `Bearer ${connection.token}`},
    rangeChunkSize: 256 * 1024,
    disableStream: true,
    disableAutoFetch: true,
  }
}

export async function uploadPageAnnotationImage(documentId: string, file: File,
  fields: {page_index:number,expected_revision:number,rect_x:number,rect_y:number,
    rect_width:number,rect_height:number}): Promise<import('./types').PageAnnotation[]> {
  const form = new FormData()
  form.append('file', file, file.name)
  for (const [key, value] of Object.entries(fields)) form.append(key, String(value))
  const response = await request(`/documents/${documentId}/annotations/image`, {
    method: 'POST', body: form,
  }, 2 * 60_000)
  return response.json() as Promise<import('./types').PageAnnotation[]>
}

export async function uploadWorkingPageImage(documentId: string, file: File,
  fields: {page_index:number,expected_revision:number,rect_x:number,rect_y:number,
    rect_width:number,rect_height:number,acknowledge_signature_risk:boolean}): Promise<import('./types').MatterDocument> {
  const form = new FormData()
  form.append('file', file, file.name)
  for (const [key, value] of Object.entries(fields)) form.append(key, String(value))
  const response = await request(`/documents/${documentId}/pages/image-overlay`, {
    method: 'POST', body: form,
  }, 2 * 60_000)
  return response.json() as Promise<import('./types').MatterDocument>
}

export async function pageAnnotationAsset(annotationId: string): Promise<Blob> {
  const response = await request(`/annotations/${annotationId}/asset`, {cache:'no-store'})
  return response.blob()
}

export async function exportPdfDocumentSource(matterId: string, exportId: string, filename: string) {
  const connection = await getConnection()
  return {
    url: `${connection.base_url}/matters/${encodeURIComponent(matterId)}/exports/${encodeURIComponent(exportId)}/${encodeURIComponent(filename)}`,
    httpHeaders: { Authorization: `Bearer ${connection.token}` },
    rangeChunkSize: 256 * 1024,
    // PDF.js requests only the byte ranges needed for reading. Disabling
    // auto-fetch requires disabling its eager complete-response stream too.
    disableStream: true,
    disableAutoFetch: true,
  }
}

export async function downloadExport(matterId: string, exportId: string, filename: string, saveAs = false): Promise<boolean> {
  if (inTauri()) {
    return invoke<boolean>('save_export', { matterId, exportId, filename, saveAs })
  }
  const url = `/matters/${matterId}/exports/${exportId}/${encodeURIComponent(filename)}`
  const response = await request(url)
  const blob = await response.blob()
  const fileUrl = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = fileUrl
  anchor.download = filename
  document.body.appendChild(anchor)
  anchor.click()
  anchor.remove()
  setTimeout(() => URL.revokeObjectURL(fileUrl), 30_000)
  return true
}

export async function downloadProtectedPdf(matterId: string, exportId: string,
                                           filename: string, password: string): Promise<boolean> {
  if (inTauri()) {
    return invoke<boolean>('save_protected_export', {matterId, exportId, filename, password})
  }
  const response = await request(
    `/matters/${matterId}/exports/${exportId}/${encodeURIComponent(filename)}/protected`,
    {method:'POST', body:JSON.stringify({password})}, 5 * 60_000)
  const fileUrl = URL.createObjectURL(await response.blob())
  const anchor = document.createElement('a')
  anchor.href = fileUrl
  anchor.download = filename.replace(/\.pdf$/i, '-protected.pdf')
  document.body.appendChild(anchor)
  anchor.click()
  anchor.remove()
  setTimeout(() => URL.revokeObjectURL(fileUrl), 30_000)
  return true
}
