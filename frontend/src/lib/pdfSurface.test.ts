import { describe, expect, it } from 'vitest'
import { pdfRasterSize } from './pdfSurface'

describe('PDF surface resource limits', () => {
  it('preserves ordinary display size and caps high-density rendering', () => {
    expect(pdfRasterSize(600, 800, 1)).toEqual({width:600, height:800, scaleX:1, scaleY:1})
    expect(pdfRasterSize(600, 800, 3)).toEqual({width:1200, height:1600, scaleX:2, scaleY:2})
  })
  it('limits both large scans and extreme page aspect ratios', () => {
    for (const [width, height] of [[6000, 9000], [1e9, 1], [1, 1e9]]) {
      const size = pdfRasterSize(width, height, 2)
      expect(size.width * size.height).toBeLessThanOrEqual(16_000_000)
      expect(size.width).toBeGreaterThanOrEqual(1)
      expect(size.height).toBeGreaterThanOrEqual(1)
    }
    expect(() => pdfRasterSize(Infinity, 200)).toThrow()
    expect(() => pdfRasterSize(100, 0)).toThrow()
  })
})

