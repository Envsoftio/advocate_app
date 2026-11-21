import { invoke } from '@tauri-apps/api/core';
import { PDFDataRangeTransport } from 'pdfjs-dist';
import type { DocumentInitParameters } from 'pdfjs-dist/types/src/display/api';
import { ApiError } from './api';

const CHUNK_BYTES = 256 * 1024;

type Artifact =
  | { kind: 'original' }
  | { kind: 'working'; revision: number }
  | { kind: 'retained'; revision: number; sha256: string };

export type PdfSource = DocumentInitParameters & { rangeFailure?: Promise<never> };

type OpenReply = {
  status: number;
  body: { token?: string; length?: number; detail?: string };
};

async function open(documentId: string, source: Artifact) {
  const result = await invoke<OpenReply>('rust_open_pdf', { documentId, source });
  return openReply(result);
}

function openReply(result: OpenReply) {
  const { token, length, detail } = result.body;
  if (result.status !== 200 || !token || !length || !Number.isSafeInteger(length))
    throw new ApiError(result.status || 500, detail || 'Rust PDF reader is unavailable.');
  return { token, length };
}

function close(token: string) {
  return invoke('rust_close_pdf', { token }).catch(() => {});
}

async function read(token: string, begin: number, end: number) {
  const bytes = await invoke<ArrayBuffer>('rust_read_pdf', { token, begin, end });
  if (bytes.byteLength !== end - begin) throw new Error('Rust PDF range ended early.');
  return new Uint8Array(bytes);
}

function errorText(error: unknown): string {
  if (typeof error === 'object' && error && 'body' in error) {
    const value = error.body as { detail?: string } | undefined;
    if (value?.detail) return value.detail;
  }
  return error instanceof Error ? error.message : String(error);
}

export async function rustPdfSource(documentId: string, source: Artifact): Promise<PdfSource> {
  return sourceFromToken(await open(documentId, source));
}

export async function rustExportPdfSource(matterId: string, exportId: string, filename: string) {
  const result = await invoke<OpenReply>('rust_open_export_pdf', { matterId, exportId, filename });
  return sourceFromToken(openReply(result));
}

export async function rustPrintPreview(route: string, options: unknown, signal: AbortSignal) {
  const result = await invoke<OpenReply>('rust_prepare_print_pdf', { route, options });
  const { token, length } = openReply(result);
  let disposed = false;
  const dispose = () => {
    if (disposed) return;
    disposed = true;
    void close(token);
  };
  try {
    const chunks: ArrayBuffer[] = [];
    for (let offset = 0; offset < length; offset += CHUNK_BYTES) {
      signal.throwIfAborted();
      const chunk = await read(token, offset, Math.min(length, offset + CHUNK_BYTES));
      chunks.push(chunk.buffer);
    }
    signal.throwIfAborted();
    return {
      file: new Blob(chunks, { type: 'application/pdf' }),
      print: () => invoke<void>('print_rust_pdf', { token }),
      dispose,
    };
  } catch (error) {
    dispose();
    if (error instanceof Error) throw error;
    throw new Error(errorText(error));
  }
}

function sourceFromToken({ token, length }: { token: string; length: number }): PdfSource {
  let fail!: (error: Error) => void;
  const rangeFailure = new Promise<never>((_, reject) => (fail = reject));
  // The reader can be closed while its source is being opened.
  void rangeFailure.catch(() => {});
  class Range extends PDFDataRangeTransport {
    private closed = false;

    requestDataRange(begin: number, end: number) {
      if (this.closed) return;
      void this.deliver(begin, end).catch((error: unknown) => {
        if (!this.closed) {
          this.abort();
          fail(new Error(errorText(error)));
        }
      });
    }

    private async deliver(begin: number, end: number) {
      // PDF.js may coalesce adjacent chunks into one request. Deliver the requested
      // span together after reading bounded IPC chunks, without sending a JSON byte array.
      const result = new Uint8Array(end - begin);
      for (let offset = begin; offset < end && !this.closed; offset += CHUNK_BYTES) {
        result.set(await read(token, offset, Math.min(end, offset + CHUNK_BYTES)), offset - begin);
      }
      if (!this.closed) this.onDataRange(begin, result);
    }

    abort() {
      if (this.closed) return;
      this.closed = true;
      void close(token);
    }
  }
  return {
    range: new Range(length, new Uint8Array(0), true),
    rangeChunkSize: CHUNK_BYTES,
    disableStream: true,
    disableAutoFetch: true,
    rangeFailure,
  };
}

export async function rustPdfBytes(documentId: string, revision: number, signal?: AbortSignal) {
  const { token, length } = await open(documentId, { kind: 'working', revision });
  try {
    const result = new Uint8Array(length);
    for (let offset = 0; offset < length; offset += CHUNK_BYTES) {
      signal?.throwIfAborted();
      result.set(await read(token, offset, Math.min(length, offset + CHUNK_BYTES)), offset);
    }
    signal?.throwIfAborted();
    return result;
  } finally {
    await close(token);
  }
}
