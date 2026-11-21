import { describe, it, expect, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { rustPilotImportUnavailableOnWindows, rustResponse } from './rustTransport';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
describe('direct Rust transport', () => {
  it('uses commands and preserves unsupported status without HTTP fallback', async () => {
    vi.mocked(invoke).mockResolvedValue({ status: 501, body: { detail: 'Not implemented' } });
    const response = await rustResponse('/matters', 'POST', new AbortController().signal);
    expect(invoke).toHaveBeenCalledWith('rust_request', {
      path: '/matters',
      method: 'POST',
      body: null,
    });
    expect(response.status).toBe(501);
    expect(await response.json()).toEqual({ detail: 'Not implemented' });
  });
  it('returns core read data', async () => {
    vi.mocked(invoke).mockResolvedValue({ status: 200, body: [] });
    expect(
      await (await rustResponse('/matters', 'GET', new AbortController().signal)).json(),
    ).toEqual([]);
  });
  it('retains metadata key order through Tauri JSON arguments', async () => {
    const source = '{"rows":[{"changes":{"z":1,"2":3,"1":4}}]}';
    const path = '/matters/id/metadata-review';
    vi.mocked(invoke).mockResolvedValue({ status: 422, body: { detail: [] } });

    const response = await rustResponse(path, 'PUT', new AbortController().signal, source);

    expect(invoke).toHaveBeenLastCalledWith('rust_request', {
      path,
      method: 'PUT',
      body: JSON.parse(source),
      keySource: source,
    });
    expect(response.status).toBe(422);
  });
  it.each([
    ['POST', '/documents/synthetic/annotations'],
    ['PATCH', '/annotations/synthetic'],
  ])('retains annotation validation order for %s via Tauri', async (method, path) => {
    const source = '{"extra_z":0,"text_style":{"odd_z":1,"odd_a":2},"extra_a":0}';
    vi.mocked(invoke).mockResolvedValue({ status: 422, body: { detail: [] } });
    const response = await rustResponse(path, method, new AbortController().signal, source);
    expect(response.status).toBe(422);
    expect(invoke).toHaveBeenLastCalledWith('rust_request', {
      path,
      method,
      body: JSON.parse(source),
      keySource: source,
    });
  });
  it('exposes a Rust thumbnail as PNG bytes', async () => {
    vi.mocked(invoke).mockResolvedValue({ status: 200, body: { png: 'iVBORw0KGgo=' } });
    const response = await rustResponse(
      '/documents/id/thumbnail?revision=1',
      'GET',
      new AbortController().signal,
    );
    expect(response.headers.get('Content-Type')).toBe('image/png');
    expect(new Uint8Array(await response.arrayBuffer())).toEqual(
      Uint8Array.of(137, 80, 78, 71, 13, 10, 26, 10),
    );
  });
  it('exposes the editable review download headers', async () => {
    const media = 'application/vnd.openxmlformats-officedocument.wordprocessingml.document';
    const disposition = 'attachment; filename="combined-review.docx"';
    const note = 'Editable text draft; PDF layout, images and signatures are not reproduced';
    vi.mocked(invoke).mockResolvedValue({
      status: 200,
      body: {
        data: 'UEsDBA==',
        media_type: media,
        content_disposition: disposition,
        conversion_note: note,
      },
    });
    const response = await rustResponse(
      '/matters/id/exports/build/combined.pdf/office/docx',
      'GET',
      new AbortController().signal,
    );
    expect(response.headers.get('Content-Type')).toBe(media);
    expect(response.headers.get('Cache-Control')).toBe('no-store');
    expect(response.headers.get('Content-Disposition')).toBe(disposition);
    expect(response.headers.get('X-CourtBundle-Conversion-Note')).toBe(note);
    expect(new Uint8Array(await response.arrayBuffer())).toEqual(Uint8Array.of(80, 75, 3, 4));
  });
  it('bounds waiting for a stalled command without replay', async () => {
    vi.mocked(invoke).mockImplementation(() => new Promise(() => {}));
    const controller = new AbortController();
    const pending = rustResponse('/health', 'GET', controller.signal);
    controller.abort();
    await expect(pending).rejects.toMatchObject({ name: 'AbortError' });
  });
});

it('passes structured mutation payloads without dropping fields', async () => {
  vi.mocked(invoke).mockResolvedValue({ status: 201, body: { id: 'synthetic' } });
  const payload = { name: 'Synthetic हिंदी', max_pages_per_volume: 400 };
  const response = await rustResponse(
    '/matters',
    'POST',
    new AbortController().signal,
    JSON.stringify(payload),
  );
  expect(response.status).toBe(201);
  expect(invoke).toHaveBeenLastCalledWith('rust_request', {
    path: '/matters',
    method: 'POST',
    body: payload,
  });
});
it('rejects binary payloads before invoking a command', async () => {
  vi.mocked(invoke).mockClear();
  const response = await rustResponse(
    '/matters/id/documents',
    'POST',
    new AbortController().signal,
    new Blob(['synthetic']),
  );
  expect(response.status).toBe(501);
  expect(invoke).not.toHaveBeenCalled();
});
it('returns the working-image upload limit before invoking Rust', async () => {
  vi.mocked(invoke).mockClear();
  const form = new FormData();
  form.set('file', new File([new Uint8Array(10 * 1024 * 1024 + 1)], 'synthetic.png'));
  const response = await rustResponse(
    '/documents/0123456789abcdef0123456789abcdef/pages/image-overlay',
    'POST',
    new AbortController().signal,
    form,
  );
  expect(response.status).toBe(413);
  expect(await response.json()).toEqual({ detail: 'Image exceeds 10 MiB' });
  expect(invoke).not.toHaveBeenCalled();
});
it('lets Rust check an editable image revision without reading oversized bytes', async () => {
  const detail = 'Document changed; reload and place the image again';
  vi.mocked(invoke).mockResolvedValue({ status: 409, body: { detail } });
  const form = new FormData();
  const file = new File([new Uint8Array(10 * 1024 * 1024 + 1)], 'synthetic.png');
  const read = vi.spyOn(file, 'arrayBuffer');
  form.set('file', file);
  form.set('expected_revision', '2');
  form.set('page_index', '0');
  form.set('rect_x', '0.1');
  form.set('rect_y', '0.2');
  form.set('rect_width', '0.3');
  form.set('rect_height', '0.4');
  const path = '/documents/0123456789abcdef0123456789abcdef/annotations/image';
  const response = await rustResponse(path, 'POST', new AbortController().signal, form);
  expect(response.status).toBe(409);
  expect(await response.json()).toEqual({ detail });
  expect(read).not.toHaveBeenCalled();
  expect(invoke).toHaveBeenLastCalledWith('rust_request', {
    method: 'POST',
    path,
    body: {
      expected_revision: '2',
      page_index: '0',
      rect_x: '0.1',
      rect_y: '0.2',
      rect_width: '0.3',
      rect_height: '0.4',
      filename: 'synthetic.png',
      file_size: file.size,
    },
  });
});
it('refuses browser PDF bytes before IPC serialization', async () => {
  vi.mocked(invoke).mockClear();
  const changed = new Uint8Array(6004);
  changed.set(new TextEncoder().encode('%PDF-'));
  const file = new File([changed], 'synthetic.pdf', { type: 'application/pdf' });
  const response = await rustResponse(
    '/matters/0123456789abcdef0123456789abcdef/documents/raw',
    'POST',
    new AbortController().signal,
    file,
  );
  expect(response.status).toBe(501);
  expect(invoke).not.toHaveBeenCalled();
});
it('refuses wrong-sized pilot input before reading it', async () => {
  vi.mocked(invoke).mockClear();
  const file = new File(['%PDF-synthetic'], 'synthetic.pdf');
  const read = vi.spyOn(file, 'arrayBuffer');
  const response = await rustResponse(
    '/matters/0123456789abcdef0123456789abcdef/documents/raw',
    'POST',
    new AbortController().signal,
    file,
  );
  expect(response.status).toBe(422);
  expect(read).not.toHaveBeenCalled();
  expect(invoke).not.toHaveBeenCalled();
});
it('rejects oversized pilot input before IPC serialization', async () => {
  vi.mocked(invoke).mockClear();
  const file = new File([new Uint8Array(4 * 1024 * 1024 + 1)], 'large.pdf');
  const response = await rustResponse(
    '/matters/0123456789abcdef0123456789abcdef/documents/raw',
    'POST',
    new AbortController().signal,
    file,
  );
  expect(response.status).toBe(413);
  expect(invoke).not.toHaveBeenCalled();
});
it('sends protected raw uploads as bytes with the scoped import metadata', async () => {
  const matter = '0123456789abcdef0123456789abcdef';
  const filename = 'scanned café + cover.png';
  const file = new File([new Uint8Array([137, 80, 78, 71])], filename, { type: 'image/png' });
  const sourceFileMtime = '2026-11-21T10:05:06.123Z';
  vi.mocked(invoke).mockClear();
  vi.mocked(invoke).mockResolvedValue({ status: 201, body: [{ original_name: filename }] });
  const response = await rustResponse(
    `/matters/${matter}/documents/raw?included=true`,
    'POST',
    new AbortController().signal,
    file,
    {
      'Content-Type': 'application/octet-stream',
      'X-CourtBundle-Filename': encodeURIComponent(filename),
      'X-CourtBundle-File-Mtime': sourceFileMtime,
      'X-CourtBundle-Pdf-Password': encodeURIComponent('synthetic+password & fixture'),
      'X-CourtBundle-Signature-Acknowledge': 'true',
    },
    true,
  );
  expect(response.status).toBe(201);
  const [command, body, options] = vi.mocked(invoke).mock.lastCall!;
  expect(command).toBe('rust_import_raw_document');
  expect(body).toEqual(await file.arrayBuffer());
  expect(options?.headers).toMatchObject({ 'X-MatterDraft-Matter-Id': matter });
  const fields = new Headers(options?.headers);
  expect(Object.fromEntries(new URLSearchParams(fields.get('X-MatterDraft-Import')!))).toEqual({
    content_type: 'application/octet-stream',
    filename,
    source_file_mtime: sourceFileMtime,
    password: 'synthetic+password & fixture',
    included: 'true',
    acknowledge_signature_risk: 'true',
  });
  expect(await response.json()).toEqual([{ original_name: filename }]);
});
it('streams a replacement above the JSON upload limit into the scoped command', async () => {
  const documentId = '0123456789abcdef0123456789abcdef';
  const file = new File([new Uint8Array(10 * 1024 * 1024 + 1)], 'replacement café.PDF');
  vi.mocked(invoke).mockClear();
  vi.mocked(invoke).mockResolvedValue({ status: 200, body: { revision: 3 } });
  const response = await rustResponse(
    `/documents/${documentId}/pages/replace-file`,
    'POST',
    new AbortController().signal,
    file,
    {
      'Content-Type': 'application/octet-stream',
      'X-CourtBundle-Filename': encodeURIComponent(file.name),
      'X-CourtBundle-Target-Pages': '[2,0]',
      'X-CourtBundle-Expected-Revision': '2',
      'X-CourtBundle-Signature-Acknowledge': 'true',
    },
    true,
  );
  expect(response.status).toBe(200);
  const [command, bytes, options] = vi.mocked(invoke).mock.lastCall!;
  expect(command).toBe('rust_replace_raw_document');
  expect((bytes as ArrayBuffer).byteLength).toBe(file.size);
  const headers = new Headers(options?.headers);
  expect(headers.get('X-MatterDraft-Document-Id')).toBe(documentId);
  expect(
    Object.fromEntries(new URLSearchParams(headers.get('X-MatterDraft-Replacement')!)),
  ).toEqual({
    content_type: 'application/octet-stream',
    filename: file.name,
    expected_revision: '2',
    target_pages: '[2,0]',
    acknowledge_signature_risk: 'true',
  });
});
it('refuses synthetic Windows PDF import before reading or serializing file bytes', async () => {
  vi.mocked(invoke).mockClear();
  vi.stubGlobal('navigator', { platform: 'Win32', userAgent: 'Windows NT 10.0' });
  try {
    const file = new File(['%PDF-synthetic'], 'synthetic.pdf');
    const response = await rustResponse(
      '/matters/0123456789abcdef0123456789abcdef/documents/raw',
      'POST',
      new AbortController().signal,
      file,
    );
    expect(response.status).toBe(501);
    expect(invoke).not.toHaveBeenCalled();
  } finally {
    vi.unstubAllGlobals();
  }
});
it('uses binary IPC for protected imports with a Windows WebView identity', async () => {
  vi.mocked(invoke).mockClear();
  vi.mocked(invoke).mockResolvedValue({ status: 201, body: [] });
  vi.stubGlobal('navigator', { platform: 'Win32', userAgent: 'Windows NT 10.0' });
  try {
    const file = new File(['%PDF-synthetic'], 'synthetic.pdf');
    const response = await rustResponse(
      '/matters/0123456789abcdef0123456789abcdef/documents/raw',
      'POST',
      new AbortController().signal,
      file,
      { 'Content-Type': 'application/octet-stream', 'X-CourtBundle-Filename': file.name },
      true,
    );
    expect(response.status).toBe(201);
    const [command, bytes] = vi.mocked(invoke).mock.lastCall!;
    expect(command).toBe('rust_import_raw_document');
    expect(bytes).toEqual(await file.arrayBuffer());
  } finally {
    vi.unstubAllGlobals();
  }
});
it('does not mistake Darwin for Windows', () => {
  vi.stubGlobal('navigator', { platform: 'Darwin', userAgent: 'MatterDraft macOS' });
  try {
    expect(rustPilotImportUnavailableOnWindows()).toBe(false);
  } finally {
    vi.unstubAllGlobals();
  }
});
