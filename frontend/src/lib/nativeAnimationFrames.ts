type FrameHost = Pick<Window, 'requestAnimationFrame' | 'cancelAnimationFrame' | 'setTimeout' | 'clearTimeout' | 'performance'>

/** WKWebView may stop delivering frames while its window is occluded, even
 * while async document work and native dialogs continue. Keep finite UI/PDF
 * work moving without changing PDF.js display/annotation rendering intent.
 * Normal frames win; only a stalled callback uses the bounded timer fallback.
 */
export function installNativeFrameFallback(host: FrameHost, maxDelay = 100): () => void {
  const originalRequest = host.requestAnimationFrame
  const originalCancel = host.cancelAnimationFrame
  const request = originalRequest.bind(host)
  const cancel = originalCancel.bind(host)
  const pending = new Map<number, number>()
  host.requestAnimationFrame = callback => {
    const finish = (time: number) => {
      const timer = pending.get(id)
      if (timer === undefined) return
      pending.delete(id)
      host.clearTimeout(timer)
      cancel(id)
      callback(time)
    }
    const id = request(finish)
    pending.set(id, host.setTimeout(() => finish(host.performance.now()), maxDelay))
    return id
  }
  host.cancelAnimationFrame = id => {
    const timer = pending.get(id)
    if (timer !== undefined) host.clearTimeout(timer)
    pending.delete(id)
    cancel(id)
  }
  return () => {
    for (const [id, timer] of pending) { host.clearTimeout(timer); cancel(id) }
    pending.clear()
    host.requestAnimationFrame = originalRequest
    host.cancelAnimationFrame = originalCancel
  }
}
