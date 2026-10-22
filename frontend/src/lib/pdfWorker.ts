// PDF.js also imports this export if a browser requires its main-thread fallback.
// @ts-expect-error PDF.js ships this worker runtime without TypeScript declarations.
export { WorkerMessageHandler } from 'pdfjs-dist/build/pdf.worker.mjs'

// PDF.js 4.10.38 setupDoc().onFailure() calls ensureNotTerminated() after
// an in-flight getPdfManager() rejects during explicit shutdown. That throws
// this exact error from an otherwise handled cancellation promise. This entry
// runs inside the document-owned worker: it never suppresses window errors or
// errors from a live document, and it uses no private PDF.js API.
// Never install the handler in PDF.js's main-thread/fake-worker fallback.
if (typeof document === 'undefined' && typeof self !== 'undefined') {
  let terminationRequested = false
  self.addEventListener('message', (event: MessageEvent) => {
    if (event.data?.action === 'Terminate' && typeof event.data.targetName === 'string'
        && event.data.targetName.endsWith('_worker')) terminationRequested = true
  })
  self.addEventListener('unhandledrejection', event => {
    const reason: unknown = event.reason
    if (terminationRequested && reason && typeof reason === 'object' && 'message' in reason
        && reason.message === 'Worker was terminated') event.preventDefault()
  })
}
