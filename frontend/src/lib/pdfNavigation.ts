import type { PDFDocumentProxy } from 'pdfjs-dist'

export interface PdfSourceOutlineEntry {
  id: string
  title: string
  /** Physical one-based PDF page, or null for a heading/unresolvable target. */
  page: number | null
  url: string | null
  children: PdfSourceOutlineEntry[]
}

/** A link remains inert until an explicit user action; never open PDF actions. */
export function safePdfExternalUrl(value: unknown): string | null {
  if (typeof value !== 'string' || /[\u0000-\u001f\u007f]/u.test(value)) return null
  try {
    const url = new URL(value)
    return ['http:', 'https:', 'mailto:'].includes(url.protocol) && !url.username && !url.password ? url.href : null
  } catch { return null }
}

/** Handles index zero, indirect refs and named destinations; ignores PDF actions. */
export async function resolvePdfDestination(
  pdf: PDFDocumentProxy, destination: unknown, isCancelled: () => boolean = () => false,
): Promise<number | null> {
  if (isCancelled()) return null
  try {
    const resolved: unknown = typeof destination === 'string' ? await pdf.getDestination(destination) : destination
    if (isCancelled() || !Array.isArray(resolved) || !resolved.length) return null
    const target: unknown = resolved[0]
    let index: number
    if (typeof target === 'number' && Number.isInteger(target)) index = target
    else if (target && typeof target === 'object' && 'num' in target && 'gen' in target
      && typeof target.num === 'number' && Number.isInteger(target.num) && target.num > 0
      && typeof target.gen === 'number' && Number.isInteger(target.gen) && target.gen >= 0) {
      index = await pdf.getPageIndex({ num: target.num, gen: target.gen })
    } else return null
    if (isCancelled() || !Number.isInteger(index) || index < 0 || index >= pdf.numPages) return null
    return index + 1
  } catch { return null }
}

/** Bounded traversal keeps malformed/deep outlines from blocking the reader. */
export async function readPdfSourceOutline(
  pdf: PDFDocumentProxy,
  options: { isCancelled?: () => boolean; maxEntries?: number; maxDepth?: number } = {},
): Promise<{ entries: PdfSourceOutlineEntry[]; truncated: boolean }> {
  const isCancelled = options.isCancelled ?? (() => false)
  if (isCancelled()) return { entries: [], truncated: false }
  const source = await pdf.getOutline() || []
  const entries: PdfSourceOutlineEntry[] = []
  const maxEntries = Math.max(1, Math.floor(options.maxEntries ?? 2000))
  const maxDepth = Math.max(1, Math.floor(options.maxDepth ?? 20))
  const seen = new Set<object>()
  let count = 0, truncated = false
  async function visit(nodes: unknown[], target: PdfSourceOutlineEntry[], path: string, depth: number) {
    for (let index = 0; index < nodes.length; index++) {
      if (isCancelled()) return
      if (count >= maxEntries) { truncated = true; return }
      const node = nodes[index]
      if (!node || typeof node !== 'object' || seen.has(node)) continue
      seen.add(node)
      const item = node as { title?: unknown; dest?: unknown; url?: unknown; items?: unknown }
      const id = `${path}${index}`
      const page = await resolvePdfDestination(pdf, item.dest, isCancelled)
      if (isCancelled()) return
      const entry: PdfSourceOutlineEntry = { id, title: typeof item.title === 'string' && item.title.trim() ? item.title : 'Untitled bookmark',
        page, url: safePdfExternalUrl(item.url), children: [] }
      target.push(entry); count++
      if (Array.isArray(item.items) && item.items.length) {
        if (depth >= maxDepth) truncated = true
        else await visit(item.items, entry.children, `${id}.`, depth + 1)
      }
      if (count % 50 === 0) await new Promise<void>(resolve => setTimeout(resolve, 0))
    }
  }
  await visit(source, entries, '', 1)
  return isCancelled() ? { entries: [], truncated: false } : { entries, truncated }
}
