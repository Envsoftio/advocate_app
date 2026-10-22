import { describe, expect, it, vi } from 'vitest'
import type { PDFDocumentProxy } from 'pdfjs-dist'
import { readPdfSourceOutline, resolvePdfDestination, safePdfExternalUrl } from './pdfNavigation'

function navigationPdf(outline: unknown[] = []) {
  return { numPages: 6,
    getDestination: vi.fn(async (name: string) => name === 'first' ? [0, { name: 'Fit' }] : null),
    getPageIndex: vi.fn(async () => 3),
    getOutline: vi.fn(async () => outline),
  } as unknown as PDFDocumentProxy
}

describe('safe imported PDF navigation', () => {
  it('resolves direct first-page index, named and indirect destinations', async () => {
    const pdf = navigationPdf()
    expect(await resolvePdfDestination(pdf, [0, { name: 'Fit' }])).toBe(1)
    expect(await resolvePdfDestination(pdf, 'first')).toBe(1)
    expect(await resolvePdfDestination(pdf, [{ num: 4, gen: 0 }, { name: 'XYZ' }])).toBe(4)
    expect(pdf.getPageIndex).toHaveBeenCalledExactlyOnceWith({ num: 4, gen: 0 })
  })

  it('refuses invalid, out-of-range and unresolved targets without failing the reader', async () => {
    const pdf = navigationPdf()
    for (const target of [null, [], [-1], [6], [1.5], ['0'], [{ num: 0, gen: 0 }], [{ num: 1 }], 'missing']) {
      expect(await resolvePdfDestination(pdf, target)).toBeNull()
    }
    vi.mocked(pdf.getPageIndex).mockRejectedValueOnce(new Error('bad reference'))
    expect(await resolvePdfDestination(pdf, [{ num: 100, gen: 0 }])).toBeNull()
  })

  it('ignores a destination resolved after its caller is cancelled', async () => {
    const pdf = navigationPdf()
    let cancelled = false
    vi.mocked(pdf.getDestination).mockImplementationOnce(async () => { cancelled = true; return [2] })
    expect(await resolvePdfDestination(pdf, 'late', () => cancelled)).toBeNull()
  })

  it('allows only explicit external web/mail links without control chars or credentials', () => {
    expect(safePdfExternalUrl('https://example.org/path?q=1')).toBe('https://example.org/path?q=1')
    expect(safePdfExternalUrl('mailto:review@example.org')).toBe('mailto:review@example.org')
    for (const url of ['javascript:alert(1)', 'data:text/html,test', 'file:///tmp/private.pdf', '/relative', 'https://user:pass@example.org', 'ht\ntps://example.org', 'https://example.org\u007f']) {
      expect(safePdfExternalUrl(url)).toBeNull()
    }
  })

  it('retains heading hierarchy and a valid child under an invalid parent target', async () => {
    const pdf = navigationPdf([{ title: 'Section', dest: 'missing', items: [
      { title: 'First page', dest: [0] }, { title: 'Website', url: 'https://example.org' },
      { title: 'Unsafe', url: 'javascript:alert(1)' },
    ] }])
    const result = await readPdfSourceOutline(pdf)
    expect(result.truncated).toBe(false)
    expect(result.entries[0]).toMatchObject({ id: '0', title: 'Section', page: null, children: [
      { id: '0.0', title: 'First page', page: 1 },
      { id: '0.1', title: 'Website', url: 'https://example.org/' },
      { id: '0.2', title: 'Unsafe', url: null },
    ] })
  })

  it('bounds entry count/depth and skips cyclic malformed trees', async () => {
    const loop: { title: string; items: unknown[] } = { title: 'Loop', items: [] }
    loop.items.push(loop)
    expect((await readPdfSourceOutline(navigationPdf([loop]))).entries[0].children).toEqual([])
    const pdf = navigationPdf([{ title: 'Parent', items: [{ title: 'Child', items: [{ title: 'Grandchild' }] }] }, { title: 'Sibling' }])
    const count = await readPdfSourceOutline(pdf, { maxEntries: 2 })
    expect(count.truncated).toBe(true)
    expect(count.entries).toHaveLength(1)
    const depth = await readPdfSourceOutline(pdf, { maxDepth: 1 })
    expect(depth.truncated).toBe(true)
    expect(depth.entries).toHaveLength(2)
    expect(depth.entries[0].children).toEqual([])
  })
})
