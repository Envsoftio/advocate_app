import { describe, expect, it } from 'vitest'
import { cropToDisplayedPercent, displayedToCropPercent, validPdfCrop } from './pdfCrop'

describe('actual-page crop preview coordinates', () => {
  const crop = { left: 10, top: 20, right: 75, bottom: 90 }
  it('shows exactly the unrotated backend crop area after each display rotation', () => {
    expect(cropToDisplayedPercent(crop, 0)).toEqual(crop)
    expect(cropToDisplayedPercent(crop, 90)).toEqual({ left: 10, top: 10, right: 80, bottom: 75 })
    expect(cropToDisplayedPercent(crop, 180)).toEqual({ left: 25, top: 10, right: 90, bottom: 80 })
    expect(cropToDisplayedPercent(crop, 270)).toEqual({ left: 20, top: 25, right: 90, bottom: 90 })
  })
  it('maps drag selections back to the same backend fractions without accumulating rotation', () => {
    for (const rotation of [0, 90, 180, 270, -90, 450]) {
      expect(displayedToCropPercent(cropToDisplayedPercent(crop, rotation), rotation)).toEqual(crop)
    }
    expect(displayedToCropPercent({ left: 0, top: 0, right: 100, bottom: 100 }, 90))
      .toEqual({ left: 0, top: 0, right: 100, bottom: 100 })
  })
  it('refuses invalid and empty rectangles instead of showing misleading geometry', () => {
    expect(validPdfCrop(crop)).toBe(true)
    for (const invalid of [{ ...crop, left: NaN }, { ...crop, right: Infinity },
      { ...crop, top: -1 }, { ...crop, bottom: 101 }, { ...crop, left: 75 }, { ...crop, top: 95 }])
      expect(validPdfCrop(invalid)).toBe(false)
  })
})
