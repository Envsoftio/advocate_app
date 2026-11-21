import { invoke } from '@tauri-apps/api/core';

export const RUST_PILOT_PDF_SHA256 =
  '76da5e34c1799a5aff3d3e9bec8cc689cb6cf8430dfc163f93bfc3ec286bf17a';
export const RUST_PILOT_PDF_SIZE = 6004;

export function rustPilotImportUnavailableOnWindows(): boolean {
  return /^win/i.test(navigator.platform) || /windows/i.test(navigator.userAgent);
}

async function encodeFile(file: File): Promise<string> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  const parts: string[] = [];
  for (let offset = 0; offset < bytes.length; offset += 32_768) {
    parts.push(String.fromCharCode(...bytes.subarray(offset, offset + 32_768)));
  }
  return btoa(parts.join(''));
}

function unquoteHeader(value: string): string {
  return (
    new URLSearchParams(`value=${value.replace(/\+/g, '%2B').replace(/&/g, '%26')}`).get('value') ??
    ''
  );
}

type RustReply = { status: number; body: unknown };

/** Direct IPC adapter for JSON operations and bounded document uploads. */
export async function rustResponse(
  path: string,
  method: string,
  signal: AbortSignal,
  body?: BodyInit | null,
  headers?: HeadersInit,
  protectedMode = false,
): Promise<Response> {
  let upload:
    | {
        command: 'rust_import_raw_document' | 'rust_replace_raw_document';
        headers: Record<string, string>;
        bytes: ArrayBuffer;
      }
    | undefined;
  if (
    body instanceof FormData &&
    method === 'POST' &&
    /^\/documents\/[a-f0-9]{32}\/(annotations\/image|pages\/image-overlay)$/.test(path)
  ) {
    const file = body.get('file');
    if (!(file instanceof File)) {
      return Response.json({ detail: 'Image file is required' }, { status: 422 });
    }
    const editable = path.endsWith('/annotations/image');
    const oversized = file.size > 10 * 1024 * 1024;
    if (oversized && !editable) {
      return Response.json({ detail: 'Image exceeds 10 MiB' }, { status: 413 });
    }
    const fields: Record<string, string | number | boolean> = {};
    for (const [key, value] of body.entries()) {
      if (key === 'file') continue;
      if (editable) {
        if (typeof value === 'string') fields[key] = value;
      } else {
        fields[key] = key === 'acknowledge_signature_risk' ? value === 'true' : Number(value);
      }
    }
    if (signal.aborted) throw new DOMException('Aborted', 'AbortError');
    body = JSON.stringify({
      ...fields,
      filename: file.name,
      ...(oversized ? { file_size: file.size } : { data: await encodeFile(file) }),
    });
  }
  const replacement = /^\/documents\/([a-f0-9]{32})\/pages\/replace-file$/.exec(path);
  if (body instanceof File && method === 'POST' && replacement) {
    if (!protectedMode)
      return Response.json(
        { detail: 'Open the protected Rust vault before replacing pages' },
        { status: 423 },
      );
    if (body.size > 80 * 1024 * 1024)
      return Response.json({ detail: 'Replacement file limit is 80 MB' }, { status: 413 });
    if (signal.aborted) throw new DOMException('Aborted', 'AbortError');
    const fields = new Headers(headers);
    upload = {
      command: 'rust_replace_raw_document',
      headers: {
        'X-MatterDraft-Document-Id': replacement[1],
        'X-MatterDraft-Replacement': new URLSearchParams({
          content_type: fields.get('Content-Type') || '',
          filename: unquoteHeader(fields.get('X-CourtBundle-Filename') || ''),
          expected_revision: fields.get('X-CourtBundle-Expected-Revision') || '',
          target_pages: fields.get('X-CourtBundle-Target-Pages') || '',
          acknowledge_signature_risk: fields.get('X-CourtBundle-Signature-Acknowledge') || '',
        }).toString(),
      },
      bytes: await body.arrayBuffer(),
    };
  }
  if (body instanceof File && method === 'POST' && path === '/matters/restore') {
    if (body.size > 160 * 1024 * 1024)
      return Response.json(
        { detail: 'Portable package exceeds the Rust restore limit' },
        { status: 413 },
      );
    const fields = new Headers(headers);
    body = JSON.stringify({
      data: await encodeFile(body),
      credential: fields.get('X-CourtBundle-Backup-Credential'),
      recovery: fields.get('X-CourtBundle-Backup-Recovery') === 'true',
    });
  }
  if (!upload && body != null && typeof body !== 'string') {
    const [route, query] = path.split('?', 2);
    const match = /^\/matters\/([a-f0-9]{32})\/documents\/raw$/.exec(route);
    if (method !== 'POST' || !match || !(body instanceof File))
      return Response.json(
        { detail: 'Unsupported binary request for the Rust engine' },
        { status: 501 },
      );
    if (!protectedMode && rustPilotImportUnavailableOnWindows())
      return Response.json(
        { detail: 'Rust PDF import is unavailable on Windows in this pilot' },
        { status: 501 },
      );
    if (!protectedMode) {
      if (body.size > 4 * 1024 * 1024)
        return new Response(JSON.stringify({ detail: 'Rust pilot PDF limit is 4 MiB' }), {
          status: 413,
        });
      if (!body.name.toLowerCase().endsWith('.pdf'))
        return new Response(JSON.stringify({ detail: 'Rust pilot accepts one PDF only' }), {
          status: 415,
        });
      if (body.size !== RUST_PILOT_PDF_SIZE)
        return new Response(
          JSON.stringify({ detail: 'Only the exact samples/01_petition.pdf fixture is allowed' }),
          { status: 422, headers: { 'Content-Type': 'application/json' } },
        );
      return new Response(
        JSON.stringify({ detail: 'Choose PDFs through the native Rust picker' }),
        {
          status: 501,
          headers: { 'Content-Type': 'application/json' },
        },
      );
    }
    if (body.size > 80 * 1024 * 1024)
      return Response.json({ detail: 'Import limit is 80 MB per file' }, { status: 413 });
    if (signal.aborted) throw new DOMException('Aborted', 'AbortError');
    const fields = new Headers(headers);
    const metadata = new URLSearchParams({
      content_type: fields.get('Content-Type') || '',
      filename: unquoteHeader(fields.get('X-CourtBundle-Filename') || ''),
      source_file_mtime: fields.get('X-CourtBundle-File-Mtime') || '',
      acknowledge_signature_risk: fields.get('X-CourtBundle-Signature-Acknowledge') || '',
      included: new URLSearchParams(query).get('included') || '',
    });
    const password = fields.get('X-CourtBundle-Pdf-Password');
    if (password !== null) metadata.set('password', unquoteHeader(password));
    upload = {
      command: 'rust_import_raw_document',
      headers: {
        'X-MatterDraft-Matter-Id': match[1],
        'X-MatterDraft-Import': metadata.toString(),
      },
      bytes: await body.arrayBuffer(),
    };
  }
  const payload = upload || body == null ? null : JSON.parse(body as string);
  // Keep field spelling and order for these schema validators. Tauri's JSON
  // argument maps are sorted before the Rust command receives them.
  const orderedValidation =
    typeof body === 'string' &&
    ((method === 'PATCH' &&
      /^\/(?:documents\/[^/]+|matters\/[^/]+(?:\/documents\/batch)?|annotations\/[^/]+)$/.test(
        path,
      )) ||
      (method === 'POST' &&
        (/^\/matters\/[^/]+\/documents\/sort-preview$/.test(path) ||
          /^\/documents\/[^/]+\/(?:annotations|pages\/(?:batch|text-overlay|redact|redactions\/[^/]+\/remove))$/.test(
            path,
          ))) ||
      (method === 'PUT' && /^\/matters\/[^/]+\/metadata-review$/.test(path)));
  if (signal.aborted) throw new DOMException('Aborted', 'AbortError');
  let onAbort: () => void = () => {};
  try {
    const result = await Promise.race([
      upload
        ? invoke<RustReply>(upload.command, upload.bytes, { headers: upload.headers })
        : invoke<RustReply>('rust_request', {
            path,
            method,
            body: payload,
            ...(orderedValidation ? { keySource: body } : {}),
          }),
      new Promise<never>((_, reject) => {
        onAbort = () => reject(new DOMException('Aborted', 'AbortError'));
        signal.addEventListener('abort', onAbort, { once: true });
      }),
    ]);
    if (
      new Headers(headers).get('X-CourtBundle-Review-Protocol') === 'challenge' &&
      result.body &&
      typeof result.body === 'object' &&
      'detail' in result.body &&
      typeof result.body.detail === 'string'
    ) {
      const detail = result.body.detail;
      if (result.status === 409 && detail.startsWith('Signature-risk acknowledgment required'))
        return Response.json({ review_required: true, detail });
      if (result.status === 422 && detail.startsWith('Text overlaps'))
        return Response.json({ placement_rejected: true, detail });
    }
    if (
      result.status === 200 &&
      method === 'GET' &&
      /^\/documents\/[^/]+\/thumbnail(?:\?|$)/.test(path)
    ) {
      const { png } = result.body as { png: string };
      const bytes = Uint8Array.from(atob(png), (character) => character.charCodeAt(0));
      return new Response(bytes, {
        headers: { 'Content-Type': 'image/png', 'Cache-Control': 'no-store' },
      });
    }
    if (
      result.status === 200 &&
      result.body &&
      typeof result.body === 'object' &&
      'data' in result.body &&
      'media_type' in result.body &&
      typeof result.body.data === 'string' &&
      typeof result.body.media_type === 'string'
    ) {
      const bytes = Uint8Array.from(atob(result.body.data), (character) => character.charCodeAt(0));
      const headers = new Headers({
        'Content-Type': result.body.media_type,
        'Cache-Control': 'no-store',
      });
      if (
        'content_disposition' in result.body &&
        typeof result.body.content_disposition === 'string'
      )
        headers.set('Content-Disposition', result.body.content_disposition);
      if ('conversion_note' in result.body && typeof result.body.conversion_note === 'string')
        headers.set('X-CourtBundle-Conversion-Note', result.body.conversion_note);
      return new Response(bytes, { headers });
    }
    return new Response(JSON.stringify(result.body), {
      status: result.status,
      headers: { 'Content-Type': 'application/json' },
    });
  } finally {
    signal.removeEventListener('abort', onAbort);
  }
}
