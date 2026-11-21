import { afterEach, expect, it, vi } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
afterEach(() => {
  vi.unstubAllGlobals();
  vi.resetModules();
});
it('routes the existing app API through direct commands with no network fallback', async () => {
  vi.stubGlobal('window', { __TAURI_INTERNALS__: {} });
  const fetchSpy = vi.fn();
  vi.stubGlobal('fetch', fetchSpy);
  const { invoke } = await import('@tauri-apps/api/core');
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === 'get_backend_connection') return { base_url: 'rust-ipc:', token: '' };
    const method = (args as { method: string }).method;
    return method === 'GET'
      ? { status: 200, body: [] }
      : { status: 501, body: { detail: 'Not implemented in Rust' } };
  });
  const { api, pdfDocumentSource } = await import('./api');
  expect(await api('/matters')).toEqual([]);
  await expect(api('/matters', 'POST', { name: 'Test' })).rejects.toMatchObject({ status: 501 });
  await expect(pdfDocumentSource('test', 1)).rejects.toMatchObject({ status: 501 });
  expect(fetchSpy).not.toHaveBeenCalled();
});

it('surfaces native startup errors and uses protected IPC after an explicit reconnect', async () => {
  vi.stubGlobal('window', { __TAURI_INTERNALS__: {} });
  const fetchSpy = vi.fn();
  vi.stubGlobal('fetch', fetchSpy);
  const { invoke } = await import('@tauri-apps/api/core');
  vi.mocked(invoke)
    .mockReset()
    .mockRejectedValueOnce('Cannot open the Rust workspace')
    .mockResolvedValueOnce({ base_url: 'rust-ipc:', token: '', rust_protected: true })
    .mockResolvedValueOnce({ status: 200, body: { enabled: true, state: 'setup' } });
  const { api, rustWorkspaceMode } = await import('./api');

  await expect(rustWorkspaceMode()).rejects.toMatchObject({
    status: 0,
    message: 'Cannot open the Rust workspace',
  });
  expect(await rustWorkspaceMode()).toBe('protected');
  expect(await api('/vault/status')).toEqual({ enabled: true, state: 'setup' });
  expect(invoke).toHaveBeenLastCalledWith('rust_request', {
    method: 'GET',
    path: '/vault/status',
    body: null,
  });
  expect(fetchSpy).not.toHaveBeenCalled();
});

it('opens the macOS Rust print panel with the same snapshot read by the preview', async () => {
  vi.stubGlobal('window', { __TAURI_INTERNALS__: {} });
  vi.stubGlobal('navigator', { platform: 'MacIntel' });
  const { invoke } = await import('@tauri-apps/api/core');
  vi.mocked(invoke).mockReset();
  const bytes = new TextEncoder().encode('%PDF-1.7').buffer;
  vi.mocked(invoke)
    .mockResolvedValueOnce({ base_url: 'rust-ipc:', token: '' })
    .mockResolvedValueOnce({
      status: 200,
      body: { token: 'print-token', length: bytes.byteLength },
    })
    .mockResolvedValueOnce(bytes)
    .mockResolvedValue(undefined);
  const { prepareNativePrint } = await import('./api');
  const options = { pages: '1-2', orientation: 'landscape', scale: 'actual' } as const;
  const source = { kind: 'working', documentId: 'doc', revision: 4, acknowledged: true } as const;
  const { signal } = new AbortController();
  const preview = await prepareNativePrint(source, options, signal);
  expect(invoke).toHaveBeenNthCalledWith(2, 'rust_prepare_print_pdf', {
    route: '/documents/doc/print',
    options: { ...options, expected_revision: 4, acknowledge_signature_risk: true },
  });
  expect(invoke).toHaveBeenNthCalledWith(3, 'rust_read_pdf', {
    token: 'print-token',
    begin: 0,
    end: bytes.byteLength,
  });
  expect(await preview?.file.arrayBuffer()).toEqual(bytes);
  await preview?.print();
  expect(invoke).toHaveBeenNthCalledWith(4, 'print_rust_pdf', { token: 'print-token' });
  preview?.dispose();
  expect(invoke).toHaveBeenNthCalledWith(5, 'rust_close_pdf', { token: 'print-token' });
  vi.stubGlobal('navigator', { platform: 'Win32' });
  expect(await prepareNativePrint(source, options, signal)).toBeNull();
  expect(invoke).toHaveBeenCalledTimes(5);
});
