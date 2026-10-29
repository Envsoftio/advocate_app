import { afterEach, describe, expect, it, vi } from 'vitest'
import { installNativeFrameFallback } from './nativeAnimationFrames'

function fixture() {
  vi.useFakeTimers()
  let sequence = 0
  const frames = new Map<number, FrameRequestCallback>()
  const cancel = vi.fn()
  const host = {
    requestAnimationFrame: (callback: FrameRequestCallback) => { const id = ++sequence; frames.set(id, callback); return id },
    cancelAnimationFrame: cancel,
    setTimeout: ((callback: () => void, delay: number) => setTimeout(callback, delay)) as Window['setTimeout'],
    clearTimeout: (id: number) => clearTimeout(id),
    performance: {now: () => 1234} as Performance,
  }
  const original = host.requestAnimationFrame
  const dispose = installNativeFrameFallback(host)
  return {host, frames, cancel, original, dispose}
}
afterEach(() => vi.useRealTimers())

describe('native frame delivery while WebKit is occluded', () => {
  it('uses the normal frame timestamp and suppresses the later timer', () => {
    const {host, frames, dispose} = fixture(), callback = vi.fn()
    const id = host.requestAnimationFrame(callback)
    frames.get(id)!(42)
    vi.advanceTimersByTime(200)
    expect(callback.mock.calls).toEqual([[42]])
    dispose()
  })
  it('finishes stalled work once and ignores a late native frame', () => {
    const {host, frames, cancel, dispose} = fixture(), callback = vi.fn()
    const id = host.requestAnimationFrame(callback)
    vi.advanceTimersByTime(99); expect(callback).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1)
    frames.get(id)!(1300)
    expect(callback.mock.calls).toEqual([[1234]])
    expect(cancel).toHaveBeenCalledWith(id)
    dispose()
  })
  it('cancels both delivery paths and cleans up on disposal', () => {
    const {host, frames, original, dispose} = fixture(), callback = vi.fn()
    const id = host.requestAnimationFrame(callback)
    host.cancelAnimationFrame(id)
    frames.get(id)!(42)
    host.requestAnimationFrame(callback)
    dispose(); vi.advanceTimersByTime(200)
    expect(callback).not.toHaveBeenCalled()
    expect(host.requestAnimationFrame).toBe(original)
  })
  it('allows a PDF continuation to schedule its next frame', () => {
    const {host, dispose} = fixture(), continuation = vi.fn()
    host.requestAnimationFrame(() => host.requestAnimationFrame(continuation))
    vi.advanceTimersByTime(100); expect(continuation).not.toHaveBeenCalled()
    vi.advanceTimersByTime(100); expect(continuation).toHaveBeenCalledOnce()
    dispose()
  })
})
