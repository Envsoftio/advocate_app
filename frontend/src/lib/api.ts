import { invoke } from '@tauri-apps/api/core';
import { rustResponse } from './rustTransport';
import { disconnectEngine, requireEngine, lastEngineResponse } from './engineConnection';
import type { Job, Matter, MatterDocument } from './types';

type BackendConnection = { base_url: string; token: string; rust_protected?: boolean };
let sessionPromise: Promise<BackendConnection> | undefined;
const REQUEST_TIMEOUT_MS = 30_000;

export function inTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

async function getConnection(): Promise<BackendConnection> {
  if (!sessionPromise) {
    sessionPromise = inTauri()
      ? invoke<BackendConnection>('get_backend_connection')
      : Promise.resolve({
          base_url: import.meta.env.VITE_API_BASE || 'http://127.0.0.1:8765',
          token: import.meta.env.VITE_DEV_TOKEN || 'development-token-change-me',
        });
  }
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      sessionPromise,
      new Promise<BackendConnection>((_resolve, reject) => {
        timer = setTimeout(
          () =>
            reject(
              new ApiError(
                0,
                'Cannot obtain the local engine session. Quit and reopen MatterDraft.',
              ),
            ),
          3_000,
        );
      }),
    ]);
  } catch (err) {
    sessionPromise = undefined;
    if (err instanceof ApiError) throw err;
    const detail = err instanceof Error ? err.message : typeof err === 'string' ? err : '';
    throw new ApiError(0, detail || 'Cannot obtain the local engine session.');
  } finally {
    if (timer) clearTimeout(timer);
  }
}

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}

async function request(
  path: string,
  init?: RequestInit,
  timeoutMs = REQUEST_TIMEOUT_MS,
): Promise<Response> {
  if (!['GET', 'HEAD'].includes(init?.method || 'GET') && !path.startsWith('/vault/'))
    requireEngine();
  const conn = await getConnection();
  const rust = conn.base_url === 'rust-ipc:';
  const headers = new Headers(init?.headers);
  if (!rust) headers.set('Authorization', 'Bearer ' + conn.token);
  if (typeof init?.body === 'string') headers.set('Content-Type', 'application/json');
  let res: Response;
  const controller = new AbortController();
  if (init?.signal?.aborted) controller.abort();
  const abortFromCaller = () => controller.abort();
  init?.signal?.addEventListener('abort', abortFromCaller, { once: true });
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  try {
    res = rust
      ? await rustResponse(
          path,
          init?.method || 'GET',
          controller.signal,
          init?.body,
          init?.headers,
          conn.rust_protected,
        )
      : await fetch(conn.base_url + path, { ...init, headers, signal: controller.signal });
  } catch (err) {
    if (!init?.signal?.aborted) disconnectEngine();
    if (controller.signal.aborted && rust)
      throw new ApiError(
        0,
        'Rust request timed out. Reconnect and check saved state before retrying.',
      );
    if (controller.signal.aborted)
      throw new ApiError(0, 'Local processing service timed out. Check the engine and retry.');
    if (rust) throw new ApiError(0, 'Rust engine request failed. ' + String(err));
    throw new ApiError(
      0,
      'Local processing service unavailable. Check it is running and retry. ' + String(err),
    );
  } finally {
    clearTimeout(timeout);
    init?.signal?.removeEventListener('abort', abortFromCaller);
  }
  if (!res.ok) {
    let detail = res.statusText;
    try {
      const body = await res.json();
      if (typeof body.detail === 'string') detail = body.detail;
      else if (Array.isArray(body.detail)) {
        detail =
          body.detail
            .map((item: { loc?: unknown; msg?: unknown }) => {
              const field = Array.isArray(item.loc)
                ? item.loc.filter((part) => part !== 'body').join('.')
                : '';
              const message = typeof item.msg === 'string' ? item.msg : 'Invalid value';
              return field ? `${field}: ${message}` : message;
            })
            .join('; ') || 'Invalid request';
      } else if (body.detail != null) detail = String(body.detail);
    } catch {
      /* retain status */
    }
    throw new ApiError(res.status, detail);
  }
  lastEngineResponse.value = Date.now();
  return res;
}

export async function api<T>(
  path: string,
  method = 'GET',
  body?: unknown,
  timeoutMs?: number,
  headers?: HeadersInit,
): Promise<T> {
  const options: RequestInit = { method, headers };
  if (body !== undefined) options.body = JSON.stringify(body);
  const response = await request(path, options, timeoutMs);
  return response.json() as Promise<T>;
}

export async function readProcessingJob(id: string, signal: AbortSignal): Promise<Job> {
  const response = await request(`/jobs/${encodeURIComponent(id)}`, { signal });
  return response.json() as Promise<Job>;
}
export async function saveReaderPosition(key: string, state: import('./pdfReader').ReadingState) {
  // Keep a final metadata-only save alive when the reader/window closes.
  await request('/reader-position', {
    method: 'PUT',
    body: JSON.stringify({ key, ...state }),
    keepalive: true,
  });
}

export async function uploadDocument(
  matterId: string,
  file: File,
  password?: string,
  acknowledgeSignatureRisk = false,
): Promise<MatterDocument[]> {
  const headers: Record<string, string> = {
    'Content-Type': 'application/octet-stream',
    'X-CourtBundle-Filename': encodeURIComponent(file.name),
    'X-CourtBundle-File-Mtime': file.lastModified ? new Date(file.lastModified).toISOString() : '',
  };
  if (password) headers['X-CourtBundle-Pdf-Password'] = encodeURIComponent(password);
  if (acknowledgeSignatureRisk) headers['X-CourtBundle-Signature-Acknowledge'] = 'true';
  const response = await request(
    `/matters/${matterId}/documents/raw`,
    { method: 'POST', body: file, headers },
    5 * 60_000,
  );
  return response.json() as Promise<MatterDocument[]>;
}

export async function uploadDocuments(matterId: string, files: File[]): Promise<void> {
  for (const file of files) await uploadDocument(matterId, file);
}

export async function restoreMatter(
  file: File,
  credential?: string,
  recovery = false,
): Promise<Matter> {
  const headers: Record<string, string> = { 'Content-Type': 'application/octet-stream' };
  if (credential) headers['X-CourtBundle-Backup-Credential'] = credential;
  if (recovery) headers['X-CourtBundle-Backup-Recovery'] = 'true';
  const response = await request(
    '/matters/restore',
    { method: 'POST', body: file, headers },
    5 * 60_000,
  );
  return response.json() as Promise<Matter>;
}

export async function downloadMatter(matterId: string): Promise<boolean> {
  requireEngine();
  if (inTauri()) return invoke<boolean>('save_matter', { matterId });
  const response = await request(`/matters/${matterId}/portable`, undefined, 5 * 60_000);
  const url = URL.createObjectURL(await response.blob());
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = 'court-matter.cbmatter';
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 30_000);
  return true;
}

export async function downloadPageOutput(
  documentId: string,
  operation: 'extract' | 'split',
  indexes: number[],
  acknowledgeSignatureRisk = false,
): Promise<boolean> {
  const selection = indexes.join(',');
  requireEngine();
  if (inTauri())
    return invoke<boolean>('save_page_output', {
      documentId,
      operation,
      selection,
      acknowledgeSignatureRisk,
    });
  const query = operation === 'extract' ? 'selection' : 'after';
  const response = await request(
    `/documents/${documentId}/pages/${operation}?${query}=${selection}` +
      `&acknowledge_signature_risk=${acknowledgeSignatureRisk}`,
  );
  const url = URL.createObjectURL(await response.blob());
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = operation === 'extract' ? 'extracted-pages.pdf' : 'split-pages.zip';
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 30_000);
  return true;
}

export async function pdfBytes(
  docId: string,
  revision: number,
  signal?: AbortSignal,
): Promise<Uint8Array> {
  if (await isRustBackend()) {
    const { rustPdfBytes } = await import('./rustPdf');
    return rustPdfBytes(docId, revision, signal);
  }
  const response = await request(`/documents/${docId}/pdf?revision=${revision}`, {
    cache: 'no-store',
    signal,
  });
  return new Uint8Array(await response.arrayBuffer());
}

/** Save a PDF snapshot with resolved editable marks flattened; private notes stay out. */
export async function downloadWorkingPdf(
  documentId: string,
  revision: number,
  title: string,
  acknowledgeSignatureRisk = false,
): Promise<boolean> {
  if (!Number.isSafeInteger(revision) || revision < 1)
    throw new ApiError(0, 'Choose a current working PDF revision to save.');
  const stem =
    title
      .replace(/\.pdf$/i, '')
      .replace(/[^a-zA-Z0-9_-]+/g, '-')
      .replace(/^-+|-+$/g, '')
      .slice(0, 80) || 'document';
  const filename = `${stem}-working-r${revision}.pdf`;
  requireEngine();
  if (inTauri())
    return invoke<boolean>('save_working_pdf', {
      documentId,
      expectedRevision: revision,
      filename,
      acknowledgeSignatureRisk,
    });
  const response = await request(
    `/documents/${encodeURIComponent(documentId)}/pdf?revision=${revision}&editable=true&acknowledge_signature_risk=${acknowledgeSignatureRisk}`,
    { cache: 'no-store' },
    5 * 60_000,
  );
  const url = URL.createObjectURL(await response.blob());
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = filename;
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 30_000);
  return true;
}

export async function documentThumbnailBlob(
  docId: string,
  revision: number,
  signal?: AbortSignal,
): Promise<Blob> {
  const response = await request(
    `/documents/${encodeURIComponent(docId)}/thumbnail?revision=${revision}`,
    { cache: 'no-store', signal },
  );
  return response.blob();
}

export async function pdfDocumentSource(docId: string, revision: number) {
  const connection = await getConnection();
  if (connection.base_url === 'rust-ipc:') {
    const { rustPdfSource } = await import('./rustPdf');
    return rustPdfSource(docId, { kind: 'working', revision });
  }
  return {
    url: `${connection.base_url}/documents/${encodeURIComponent(docId)}/pdf?revision=${revision}`,
    httpHeaders: { Authorization: `Bearer ${connection.token}` },
    rangeChunkSize: 256 * 1024,
    disableStream: true,
    disableAutoFetch: true,
  };
}

export async function uploadPageAnnotationImage(
  documentId: string,
  file: File,
  fields: {
    page_index: number;
    expected_revision: number;
    rect_x: number;
    rect_y: number;
    rect_width: number;
    rect_height: number;
  },
): Promise<import('./types').PageAnnotation[]> {
  const form = new FormData();
  form.append('file', file, file.name);
  for (const [key, value] of Object.entries(fields)) form.append(key, String(value));
  const response = await request(
    `/documents/${documentId}/annotations/image`,
    {
      method: 'POST',
      body: form,
    },
    2 * 60_000,
  );
  return response.json() as Promise<import('./types').PageAnnotation[]>;
}

export async function uploadWorkingPageImage(
  documentId: string,
  file: File,
  fields: {
    page_index: number;
    expected_revision: number;
    rect_x: number;
    rect_y: number;
    rect_width: number;
    rect_height: number;
    acknowledge_signature_risk: boolean;
  },
): Promise<import('./types').MatterDocument> {
  const form = new FormData();
  form.append('file', file, file.name);
  for (const [key, value] of Object.entries(fields)) form.append(key, String(value));
  const response = await request(
    `/documents/${documentId}/pages/image-overlay`,
    {
      method: 'POST',
      body: form,
    },
    2 * 60_000,
  );
  return response.json() as Promise<import('./types').MatterDocument>;
}

export async function replaceWorkingPagesFromFile(
  documentId: string,
  file: File,
  fields: {
    target_pages: number[];
    expected_revision: number;
    acknowledge_signature_risk: boolean;
  },
): Promise<MatterDocument> {
  const headers: Record<string, string> = {
    'Content-Type': 'application/octet-stream',
    'X-CourtBundle-Filename': encodeURIComponent(file.name),
    'X-CourtBundle-Target-Pages': JSON.stringify(fields.target_pages),
    'X-CourtBundle-Expected-Revision': String(fields.expected_revision),
    'X-CourtBundle-Signature-Acknowledge': String(fields.acknowledge_signature_risk),
  };
  const response = await request(
    `/documents/${documentId}/pages/replace-file`,
    {
      method: 'POST',
      body: file,
      headers,
    },
    5 * 60_000,
  );
  return response.json() as Promise<MatterDocument>;
}

export async function pageAnnotationAsset(annotationId: string): Promise<Blob> {
  const response = await request(`/annotations/${annotationId}/asset`, { cache: 'no-store' });
  return response.blob();
}

export async function originalDocumentSource(documentId: string) {
  const connection = await getConnection();
  if (connection.base_url === 'rust-ipc:') {
    const { rustPdfSource } = await import('./rustPdf');
    return rustPdfSource(documentId, { kind: 'original' });
  }
  return {
    url: `${connection.base_url}/documents/${encodeURIComponent(documentId)}/original`,
    httpHeaders: { Authorization: `Bearer ${connection.token}` },
    rangeChunkSize: 256 * 1024,
    disableStream: true,
    disableAutoFetch: true,
  };
}
export async function originalImage(documentId: string) {
  return (
    await request(`/documents/${encodeURIComponent(documentId)}/original`, { cache: 'no-store' })
  ).blob();
}

export async function exportPdfDocumentSource(
  matterId: string,
  exportId: string,
  filename: string,
) {
  const connection = await getConnection();
  if (connection.base_url === 'rust-ipc:') {
    const { rustExportPdfSource } = await import('./rustPdf');
    return rustExportPdfSource(matterId, exportId, filename);
  }
  return {
    url: `${connection.base_url}/matters/${encodeURIComponent(matterId)}/exports/${encodeURIComponent(exportId)}/inspection/${encodeURIComponent(filename)}`,
    httpHeaders: { Authorization: `Bearer ${connection.token}` },
    rangeChunkSize: 256 * 1024,
    // PDF.js requests only the byte ranges needed for reading. Disabling
    // auto-fetch requires disabling its eager complete-response stream too.
    disableStream: true,
    disableAutoFetch: true,
  };
}

export async function downloadExport(
  matterId: string,
  exportId: string,
  filename: string,
  saveAs = false,
  release = false,
): Promise<boolean> {
  requireEngine();
  if (inTauri()) {
    return invoke<boolean>('save_export', { matterId, exportId, filename, saveAs, release });
  }
  const url = `/matters/${matterId}/exports/${exportId}/${encodeURIComponent(filename)}${release ? '?release=true' : ''}`;
  const response = await request(url);
  const blob = await response.blob();
  const fileUrl = URL.createObjectURL(blob);
  const anchor = document.createElement('a');
  anchor.href = fileUrl;
  anchor.download = filename;
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(fileUrl), 30_000);
  return true;
}

export async function downloadOfficeReview(
  matterId: string,
  exportId: string,
  filename: string,
  kind: 'docx' | 'xlsx' | 'pptx',
): Promise<boolean> {
  requireEngine();
  if (inTauri())
    return invoke<boolean>('save_office_export', { matterId, exportId, filename, kind });
  const response = await request(
    `/matters/${encodeURIComponent(matterId)}/exports/${encodeURIComponent(exportId)}/${encodeURIComponent(filename)}/office/${kind}`,
    { cache: 'no-store' },
    5 * 60_000,
  );
  const outputName = filename.replace(/\.[^.]+$/, '') + `-review.${kind}`;
  const url = URL.createObjectURL(await response.blob());
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = outputName;
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 30_000);
  return true;
}

export async function printableExport(
  matterId: string,
  exportId: string,
  filename: string,
  options: {
    pages: string;
    orientation: 'source' | 'portrait' | 'landscape';
    scale: 'fit' | 'actual';
  },
): Promise<Blob> {
  const response = await request(
    `/matters/${encodeURIComponent(matterId)}/exports/${encodeURIComponent(exportId)}/${encodeURIComponent(filename)}/print`,
    { method: 'POST', body: JSON.stringify(options), cache: 'no-store' },
    5 * 60_000,
  );
  return response.blob();
}

type PrintOptions = {
  pages: string;
  orientation: 'source' | 'portrait' | 'landscape';
  scale: 'fit' | 'actual';
};

type NativePrintSource =
  | { kind: 'working'; documentId: string; revision: number; acknowledged: boolean }
  | { kind: 'original'; documentId: string }
  | { kind: 'export'; matterId: string; exportId: string; filename: string };

export async function prepareNativePrint(
  source: NativePrintSource,
  options: PrintOptions,
  signal: AbortSignal,
) {
  if (!inTauri() || !/^mac/i.test(navigator.platform) || !(await isRustBackend())) return null;
  signal.throwIfAborted();
  const route =
    source.kind === 'export'
      ? `/matters/${encodeURIComponent(source.matterId)}/exports/${encodeURIComponent(source.exportId)}/${encodeURIComponent(source.filename)}/print`
      : `/documents/${encodeURIComponent(source.documentId)}${source.kind === 'original' ? '/original' : ''}/print`;
  const body =
    source.kind === 'working'
      ? {
          ...options,
          expected_revision: source.revision,
          acknowledge_signature_risk: source.acknowledged,
        }
      : options;
  const { rustPrintPreview } = await import('./rustPdf');
  return rustPrintPreview(route, body, signal);
}

export async function printableWorkingPdf(
  documentId: string,
  revision: number,
  options: {
    pages: string;
    orientation: 'source' | 'portrait' | 'landscape';
    scale: 'fit' | 'actual';
  },
  acknowledgeSignatureRisk = false,
): Promise<Blob> {
  const response = await request(
    `/documents/${encodeURIComponent(documentId)}/print`,
    {
      method: 'POST',
      body: JSON.stringify({
        ...options,
        expected_revision: revision,
        acknowledge_signature_risk: acknowledgeSignatureRisk,
      }),
      cache: 'no-store',
    },
    5 * 60_000,
  );
  return response.blob();
}

export async function printableOriginalPdf(
  documentId: string,
  options: {
    pages: string;
    orientation: 'source' | 'portrait' | 'landscape';
    scale: 'fit' | 'actual';
  },
): Promise<Blob> {
  const response = await request(
    `/documents/${encodeURIComponent(documentId)}/original/print`,
    { method: 'POST', body: JSON.stringify(options), cache: 'no-store' },
    5 * 60_000,
  );
  return response.blob();
}

export async function downloadProtectedPdf(
  matterId: string,
  exportId: string,
  filename: string,
  password: string,
): Promise<boolean> {
  requireEngine();
  if (inTauri()) {
    return invoke<boolean>('save_protected_export', { matterId, exportId, filename, password });
  }
  const response = await request(
    `/matters/${matterId}/exports/${exportId}/${encodeURIComponent(filename)}/protected`,
    { method: 'POST', body: JSON.stringify({ password }) },
    5 * 60_000,
  );
  const fileUrl = URL.createObjectURL(await response.blob());
  const anchor = document.createElement('a');
  anchor.href = fileUrl;
  anchor.download = filename.replace(/\.pdf$/i, '-protected.pdf');
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(fileUrl), 30_000);
  return true;
}

export async function revealSavedExport(matterId: string, exportId: string): Promise<void> {
  if (!inTauri()) throw new Error('Saved-file reveal is available in the desktop app.');
  await invoke('reveal_saved_export', { matterId, exportId });
}

export async function retainedPdfSource(documentId: string, revision: number, sha256: string) {
  const connection = await getConnection();
  if (connection.base_url === 'rust-ipc:') {
    const { rustPdfSource } = await import('./rustPdf');
    return rustPdfSource(documentId, { kind: 'retained', revision, sha256 });
  }
  return {
    url: `${connection.base_url}/documents/${encodeURIComponent(documentId)}/revisions/${revision}/pdf?sha256=${encodeURIComponent(sha256)}`,
    httpHeaders: { Authorization: `Bearer ${connection.token}` },
    rangeChunkSize: 256 * 1024,
    disableStream: true,
    disableAutoFetch: true,
  };
}

/** Use the connection's explicit backend selection. */
export async function isRustBackend(): Promise<boolean> {
  return (await getConnection()).base_url === 'rust-ipc:';
}

export async function rustWorkspaceMode(): Promise<'synthetic' | 'protected' | null> {
  const connection = await getConnection();
  if (connection.base_url !== 'rust-ipc:') return null;
  return connection.rust_protected ? 'protected' : 'synthetic';
}
