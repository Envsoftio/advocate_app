import { afterEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { downloadWorkingPdf } from './api'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
afterEach(() => { vi.unstubAllGlobals(); vi.clearAllMocks() })

describe('working PDF native save', () => {
  it('passes the exact revision and keeps picker cancellation distinct from a successful save', async () => {
    vi.stubGlobal('window', { __TAURI_INTERNALS__: {} })
    const fetch = vi.fn()
    vi.stubGlobal('fetch', fetch)
    vi.mocked(invoke).mockResolvedValueOnce(false)
    await expect(downloadWorkingPdf('0123456789abcdef0123456789abcdef', 7, '../Synthetic / Petition.pdf')).resolves.toBe(false)
    expect(invoke).toHaveBeenCalledWith('save_working_pdf', {
      documentId: '0123456789abcdef0123456789abcdef', expectedRevision: 7,
      filename: 'Synthetic-Petition-working-r7.pdf',
      acknowledgeSignatureRisk: false,
    })
    expect(fetch).not.toHaveBeenCalled()
  })

  it('does not claim success after native transfer failure or an invalid revision', async () => {
    vi.stubGlobal('window', { __TAURI_INTERNALS__: {} })
    vi.mocked(invoke).mockRejectedValueOnce(new Error('Working PDF revision changed'))
    await expect(downloadWorkingPdf('0123456789abcdef0123456789abcdef', 3, 'Synthetic')).rejects.toThrow('revision changed')
    await expect(downloadWorkingPdf('0123456789abcdef0123456789abcdef', 0, 'Synthetic')).rejects.toThrow('current working PDF revision')
    expect(invoke).toHaveBeenCalledTimes(1)
  })
})
