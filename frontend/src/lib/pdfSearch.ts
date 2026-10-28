import { computed, onScopeDispose, ref, shallowRef, watch, type Ref } from 'vue'
import { engineBlocked } from './engineConnection'
import type { PDFDocumentProxy } from 'pdfjs-dist'

/** Offsets refer to the original PDF.js item's str, in UTF-16 DOM Range units. */
export interface PdfSearchFragment {
  matchIndex: number
  /** Index in getTextContent().items, including any marked-content entries. */
  itemIndex: number
  start: number
  end: number
}

export interface PdfSearchMatch {
  index: number
  /** Physical, one-based page number in this PDF; never a printed bundle number. */
  page: number
  snippet: string
  fragments: PdfSearchFragment[]
}

type SearchItem = { str: string; hasEOL?: boolean; transform?: number[]; width?: number; height?: number; dir?: string }
type SourceSpan = { itemIndex: number; start: number; end: number }
type NormalizedText = { text: string; starts: number[]; ends: number[] }

function isTextItem(item: unknown): item is SearchItem {
  return !!item && typeof item === 'object' && 'str' in item && typeof item.str === 'string'
}

/**
 * PDF.js normally supplies spaces. Where a producer splits runs without one,
 * infer a separator only from a real line break or a positive text-axis gap.
 * Adjacent style runs within a word must remain searchable as one word.
 */
function needsSeparator(previous: SearchItem, next: SearchItem): boolean {
  if (previous.hasEOL) return true
  if (!previous.str || !next.str || /\s$/u.test(previous.str) || /^\s/u.test(next.str)) return false
  const a = previous.transform, b = next.transform
  if (!a || !b || a.length < 6 || b.length < 6 || previous.dir === 'rtl' || previous.dir === 'ttb') return false
  const axis = Math.hypot(a[0], a[1])
  if (!axis || !Number.isFinite(previous.width)) return false
  const dx = b[4] - a[4], dy = b[5] - a[5]
  const along = (dx * a[0] + dy * a[1]) / axis
  const across = Math.abs((-dx * a[1] + dy * a[0]) / axis)
  const height = previous.height || Math.hypot(a[2], a[3]) || axis
  return across > height * .5 || along - previous.width! > Math.max(.5, height * .12)
}

/**
 * Search uses compatibility/canonical normalization and Unicode default lower
 * case, not locale-specific collation or full Unicode case folding (e.g. ß/ss).
 * Combining marks stay attached to their source cluster, including Hindi nukta.
 * Every normalized UTF-16 unit maps back to an entire original cluster, so an
 * expanded ligature/case mapping cannot shift the DOM highlight to another word.
 */
function normalizeWithOffsets(source: string): NormalizedText {
  let text = ''
  const starts: number[] = [], ends: number[] = []
  const clusters = source.matchAll(/(?:[^\p{M}]\p{M}*|\p{M}+)/gu)
  for (const cluster of clusters) {
    const start = cluster.index!, end = start + cluster[0].length
    const normalized = cluster[0].normalize('NFKC').toLowerCase()
    for (const point of normalized) {
      if (/\s/u.test(point)) {
        if (!text) continue
        if (text.endsWith(' ')) { ends[ends.length - 1] = end; continue }
        text += ' '
        starts.push(start); ends.push(end)
      } else {
        text += point
        for (let unit = 0; unit < point.length; unit++) { starts.push(start); ends.push(end) }
      }
    }
  }
  if (text.endsWith(' ')) { text = text.slice(0, -1); starts.pop(); ends.pop() }
  return { text, starts, ends }
}

export function normalizePdfSearchQuery(query: string): string {
  return normalizeWithOffsets(query).text
}

/** Finds non-overlapping occurrences, retaining exact original text-item offsets. */
export function findPdfTextMatches(
  items: readonly unknown[], query: string,
  options: { page?: number; matchIndexOffset?: number; limit?: number } = {},
): { matches: PdfSearchMatch[]; truncated: boolean } {
  const needle = normalizePdfSearchQuery(query)
  const limit = Math.max(0, Math.floor(options.limit ?? 1000))
  if (!needle) return { matches: [], truncated: false }
  const spans: SourceSpan[] = []
  const pieces: string[] = []
  let length = 0, previous: SearchItem | null = null
  items.forEach((item, itemIndex) => {
    if (!isTextItem(item)) return
    if (previous && needsSeparator(previous, item)) { pieces.push('\n'); length++ }
    const start = length
    pieces.push(item.str); length += item.str.length
    spans.push({ itemIndex, start, end: length })
    previous = item
  })
  const source = pieces.join('')
  const normalized = normalizeWithOffsets(source)
  const matches: PdfSearchMatch[] = []
  let from = 0, spanCursor = 0
  while (from <= normalized.text.length - needle.length) {
    const offset = normalized.text.indexOf(needle, from)
    if (offset < 0) break
    if (matches.length >= limit) return { matches, truncated: true }
    const start = normalized.starts[offset], end = normalized.ends[offset + needle.length - 1]
    const index = (options.matchIndexOffset ?? 0) + matches.length
    const fragments: PdfSearchFragment[] = []
    while (spanCursor < spans.length && spans[spanCursor].end <= start) spanCursor++
    for (let cursor = spanCursor; cursor < spans.length && spans[cursor].start < end; cursor++) {
      const span = spans[cursor]
      const fragmentStart = Math.max(start, span.start) - span.start
      const fragmentEnd = Math.min(end, span.end) - span.start
      if (fragmentEnd > fragmentStart) fragments.push({ matchIndex: index, itemIndex: span.itemIndex, start: fragmentStart, end: fragmentEnd })
    }
    const snippetStart = Math.max(0, start - 36), snippetEnd = Math.min(source.length, end + 56)
    matches.push({ index, page: options.page ?? 1,
      snippet: `${snippetStart ? '…' : ''}${source.slice(snippetStart, snippetEnd).replace(/\s+/gu, ' ').trim()}${snippetEnd < source.length ? '…' : ''}`,
      fragments })
    from = offset + needle.length
  }
  return { matches, truncated: false }
}

export interface PdfSearchOptions {
  debounceMs?: number
  maxResults?: number
  minQueryLength?: number
}

/** In-memory, document-lifetime search shared by working and generated readers. */
export function usePdfSearch(document: Ref<PDFDocumentProxy | null>, query: Ref<string>, options: PdfSearchOptions = {}) {
  const matches = shallowRef<PdfSearchMatch[]>([])
  const selectedIndex = ref(0)
  const busy = ref(false), error = ref(''), truncated = ref(false), searchedPages = ref(0)
  const activeMatch = computed(() => matches.value[selectedIndex.value] ?? null)
  const maxResults = Math.max(1, Math.floor(options.maxResults ?? 1000))
  let generation = 0
  let timer: ReturnType<typeof setTimeout> | undefined

  function cancel() {
    generation++
    if (timer !== undefined) { clearTimeout(timer); timer = undefined }
    busy.value = false
  }
  function select(index: number) {
    if (Number.isInteger(index) && index >= 0 && index < matches.value.length) selectedIndex.value = index
    return activeMatch.value
  }
  function step(delta: number) {
    if (matches.value.length) selectedIndex.value = ((selectedIndex.value + delta) % matches.value.length + matches.value.length) % matches.value.length
    return activeMatch.value
  }
  // PDF.js range failures can leave a text promise pending. Bound each read,
  // retain earlier results, and do not infer that unvisited pages have no text.
  async function available<T>(work: Promise<T>): Promise<T> {
    let timeout: ReturnType<typeof setTimeout> | undefined
    try {
      return await Promise.race([work, new Promise<T>((_, reject) => {
        timeout = setTimeout(() => reject(new Error('PDF data unavailable')), engineBlocked.value ? 2000 : 10000)
      })])
    } finally { if (timeout) clearTimeout(timeout) }
  }
  async function search(pdf: PDFDocumentProxy, needle: string, call: number) {
    const stale = () => call !== generation || document.value !== pdf
    const found: PdfSearchMatch[] = []
    let pageNumber = 1
    try {
      for (; pageNumber <= pdf.numPages; pageNumber++) {
        if (stale()) return
        const page = await available(pdf.getPage(pageNumber))
        if (stale()) return
        const content = await available(page.getTextContent())
        if (stale()) return
        const result = findPdfTextMatches(content.items, needle, { page: pageNumber, matchIndexOffset: found.length, limit: maxResults - found.length })
        found.push(...result.matches)
        if (result.matches.length) matches.value = found.slice()
        searchedPages.value = pageNumber
        if (result.truncated) { truncated.value = true; break }
        // Let interaction/rendering run between page extractions. No document
        // text cache is retained; PDF.js owns its own document-lifetime cache.
        if (pageNumber < pdf.numPages) await new Promise<void>(resolve => setTimeout(resolve, 0))
      }
    } catch {
      if (!stale()) error.value = `Search could not read page ${pageNumber}. Results may be incomplete.`
    } finally { if (!stale()) busy.value = false }
  }
  const stop = watch([document, query], ([pdf, value]) => {
    cancel()
    matches.value = []; selectedIndex.value = 0; error.value = ''; truncated.value = false; searchedPages.value = 0
    const needle = normalizePdfSearchQuery(value)
    if (!pdf || needle.length < (options.minQueryLength ?? 1)) return
    busy.value = true
    const call = generation
    timer = setTimeout(() => { timer = undefined; void search(pdf, needle, call) }, options.debounceMs ?? 200)
  }, { immediate: true, flush: 'sync' })
  onScopeDispose(() => { stop(); cancel(); matches.value = [] })
  return { matches, selectedIndex, activeMatch, busy, error, truncated, searchedPages, select, step, cancel }
}
