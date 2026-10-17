import { afterEach, describe, expect, it, vi } from 'vitest'
import { api, ApiError } from './api'

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

describe('local API failures', () => {
  it('bounds a stalled health request', async () => {
    vi.useFakeTimers()
    vi.stubGlobal('fetch', vi.fn((_url: string, init: RequestInit) =>
      new Promise((_resolve, reject) => {
        init.signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')))
      })))
    const pending = api('/health', 'GET', undefined, 100)
    const result = expect(pending).rejects.toThrow('Local Python service timed out')
    await vi.advanceTimersByTimeAsync(100)
    await result
  })

  it('reuses the current session after a transient connection failure', async () => {
    const fetchMock = vi.fn()
      .mockRejectedValueOnce(new TypeError('connection refused'))
      .mockResolvedValueOnce(new Response(JSON.stringify({ status: 'ok' }), { status: 200 }))
    vi.stubGlobal('fetch', fetchMock)
    await expect(api('/health')).rejects.toBeInstanceOf(ApiError)
    await expect(api<{status: string}>('/health')).resolves.toEqual({ status: 'ok' })
    expect(fetchMock).toHaveBeenCalledTimes(2)
    const first = fetchMock.mock.calls[0][1].headers as Headers
    const second = fetchMock.mock.calls[1][1].headers as Headers
    expect(second.get('Authorization')).toBe(first.get('Authorization'))
  })

  it('shows field validation failures as readable text', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(JSON.stringify({ detail: [
      { loc: ['body', 'rect_width'], msg: 'Input should be greater than 0' },
      { loc: ['body'], msg: 'Redaction region must stay within the displayed page' },
    ] }), { status: 422 })))
    await expect(api('/documents/synthetic/pages/redact', 'POST', {})).rejects.toThrow(
      'rect_width: Input should be greater than 0; Redaction region must stay within the displayed page')
  })
})
