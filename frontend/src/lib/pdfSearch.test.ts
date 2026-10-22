import { afterEach, describe, expect, it, vi } from 'vitest'
import { effectScope, ref, shallowRef, type EffectScope } from 'vue'
import type { PDFDocumentProxy } from 'pdfjs-dist'
import { findPdfTextMatches, usePdfSearch } from './pdfSearch'

describe('PDF occurrence search', () => {
  it('finds each occurrence and phrases across styled runs with original item indices', () => {
    const items = [{ str: 'Court ' }, { type: 'beginMarkedContent' }, { str: 'Bun' }, { str: 'dle; COURT bundle' }]
    const result = findPdfTextMatches(items, 'court bundle', { page: 4, matchIndexOffset: 7 })
    expect(result.matches).toHaveLength(2)
    expect(result.matches[0]).toMatchObject({ index: 7, page: 4, fragments: [
      { matchIndex: 7, itemIndex: 0, start: 0, end: 6 },
      { matchIndex: 7, itemIndex: 2, start: 0, end: 3 },
      { matchIndex: 7, itemIndex: 3, start: 0, end: 3 },
    ] })
    expect(result.matches[1].fragments).toEqual([{ matchIndex: 8, itemIndex: 3, start: 5, end: 17 }])
  })

  it('normalizes line breaks, Unicode spaces and case while retaining source offsets', () => {
    const items = [{ str: '  COURT\u00a0\t', hasEOL: true }, { str: 'Bundle  ' }]
    const result = findPdfTextMatches(items, '\tcourt   BUNDLE\n')
    expect(result.matches).toHaveLength(1)
    expect(result.matches[0].fragments).toEqual([
      { matchIndex: 0, itemIndex: 0, start: 2, end: 9 },
      { matchIndex: 0, itemIndex: 1, start: 0, end: 6 },
    ])
  })

  it('infers real text-axis gaps on rotated text without inserting spaces within words', () => {
    const run = (str: string, y: number, width: number) => ({ str, transform: [0, 12, -12, 0, 100, y], width, height: 12 })
    const items = [run('Court', 10, 30), run('Bun', 44, 18), run('dle', 62, 18)]
    expect(findPdfTextMatches(items, 'court bundle').matches).toHaveLength(1)
    expect(findPdfTextMatches(items, 'bun dle').matches).toHaveLength(0)
  })

  it('matches Hindi combining forms across runs and maps to original UTF-16 offsets', () => {
    const items = [{ str: '😀 हिंदी क' }, { str: '\u093cानून; हिंदी क़ानून' }]
    const result = findPdfTextMatches(items, 'हिंदी क़ानून')
    expect(result.matches).toHaveLength(2)
    expect(result.matches[0].fragments).toEqual([
      { matchIndex: 0, itemIndex: 0, start: 3, end: 10 },
      { matchIndex: 0, itemIndex: 1, start: 0, end: 5 },
    ])
    expect(result.matches[1].fragments[0]).toMatchObject({ itemIndex: 1, start: 7, end: 18 })
  })

  it('keeps offsets correct when lowercase/ligature normalization expands a character', () => {
    expect(findPdfTextMatches([{ str: '😀İ ffi ﬃ end' }], 'i\u0307').matches[0].fragments)
      .toEqual([{ matchIndex: 0, itemIndex: 0, start: 2, end: 3 }])
    const matches = findPdfTextMatches([{ str: '😀İ ffi ﬃ end' }], 'ffi').matches
    expect(matches.map(match => match.fragments[0])).toEqual([
      { matchIndex: 0, itemIndex: 0, start: 4, end: 7 },
      { matchIndex: 1, itemIndex: 0, start: 8, end: 9 },
    ])
  })

  it('reports actual truncation and treats empty/scanned pages as zero results', () => {
    expect(findPdfTextMatches([{ str: 'one one' }], 'one', { limit: 2 })).toMatchObject({ truncated: false })
    expect(findPdfTextMatches([{ str: 'one one one' }], 'one', { limit: 2 })).toMatchObject({ truncated: true, matches: [expect.anything(), expect.anything()] })
    expect(findPdfTextMatches([], 'one')).toEqual({ matches: [], truncated: false })
    expect(findPdfTextMatches([{ str: 'text' }], '\t ')).toEqual({ matches: [], truncated: false })
  })
})

function pdfWithPages(texts: string[]) {
  return { numPages: texts.length,
    getPage: vi.fn(async (page: number) => ({ getTextContent: async () => ({ items: [{ str: texts[page - 1] }] }) })),
  } as unknown as PDFDocumentProxy
}
function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>(done => { resolve = done })
  return { promise, resolve }
}

describe('document-lifetime search controller', () => {
  let scope: EffectScope | undefined
  afterEach(() => { scope?.stop(); vi.useRealTimers() })
  function start(pdf: PDFDocumentProxy, value = 'court', maxResults?: number) {
    vi.useFakeTimers()
    const document = shallowRef<PDFDocumentProxy | null>(pdf), query = ref(value)
    scope = effectScope()
    const search = scope.run(() => usePdfSearch(document, query, { debounceMs: 10, maxResults }))!
    return { document, query, search }
  }

  it('searches pages sequentially, counts occurrences and wraps next/previous', async () => {
    const pdf = pdfWithPages(['court COURT', 'scanned page', 'court'])
    const { search } = start(pdf)
    await vi.runAllTimersAsync()
    expect(search.matches.value.map(match => match.page)).toEqual([1, 1, 3])
    expect(search.searchedPages.value).toBe(3)
    expect(search.busy.value).toBe(false)
    expect(search.step(-1)?.index).toBe(2)
    expect(search.step(1)?.index).toBe(0)
    expect(search.select(1)?.index).toBe(1)
    expect(search.select(100)?.index).toBe(1)
  })

  it('bounds results and stops scanning after finding an omitted occurrence', async () => {
    const pdf = pdfWithPages(['court court', 'court', 'court'])
    const { search } = start(pdf, 'court', 2)
    await vi.runAllTimersAsync()
    expect(search.matches.value).toHaveLength(2)
    expect(search.truncated.value).toBe(true)
    expect(pdf.getPage).toHaveBeenCalledTimes(2)
  })

  it('clears hits immediately on document switch and ignores delayed old extraction', async () => {
    const oldText = deferred<{ items: { str: string }[] }>()
    const oldPdf = { numPages: 1, getPage: async () => ({ getTextContent: () => oldText.promise }) } as unknown as PDFDocumentProxy
    const { document, search } = start(oldPdf)
    await vi.advanceTimersByTimeAsync(10)
    document.value = pdfWithPages(['new court court'])
    expect(search.matches.value).toEqual([])
    await vi.runAllTimersAsync()
    expect(search.matches.value).toHaveLength(2)
    oldText.resolve({ items: [{ str: 'old court' }] })
    await Promise.resolve(); await Promise.resolve()
    expect(search.matches.value).toHaveLength(2)
    expect(search.matches.value[0].snippet).toBe('new court court')
  })

  it('invalidates a pending query and clears retained text when its scope closes', async () => {
    const pdf = pdfWithPages(['court bundle'])
    const { query, search } = start(pdf)
    query.value = 'bundle'
    await vi.runAllTimersAsync()
    expect(pdf.getPage).toHaveBeenCalledTimes(1)
    expect(search.matches.value[0].fragments[0]).toMatchObject({ start: 6, end: 12 })
    query.value = 'court'
    expect(search.matches.value).toEqual([])
    scope!.stop()
    await vi.runAllTimersAsync()
    expect(pdf.getPage).toHaveBeenCalledTimes(1)
    expect(search.matches.value).toEqual([])
    expect(search.busy.value).toBe(false)
  })

  it('reports an unreadable page without discarding prior occurrences', async () => {
    const pdf = pdfWithPages(['court', 'court'])
    const getPage = pdf.getPage
    vi.mocked(pdf.getPage).mockImplementation(async pageNumber => {
      if (pageNumber === 2) throw new Error('Unreadable PDF')
      return { getTextContent: async () => ({ items: [{ str: 'court' }] }) } as Awaited<ReturnType<typeof getPage>>
    })
    const { search } = start(pdf)
    await vi.runAllTimersAsync()
    expect(search.error.value).toBe('Search could not read page 2. Results may be incomplete.')
    expect(search.matches.value).toHaveLength(1)
    expect(search.busy.value).toBe(false)
  })
})
