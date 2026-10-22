/** Backend crop fractions use the unrotated current crop box. */
export interface PdfCropPercent { left: number; top: number; right: number; bottom: number }

export function validPdfCrop(crop: PdfCropPercent): boolean {
  return Object.values(crop).every(value => Number.isFinite(value) && value >= 0 && value <= 100)
    && crop.left < crop.right && crop.top < crop.bottom
}

function rotatePoint(x: number, y: number, rotation: number): [number, number] {
  switch (((rotation % 360) + 360) % 360) {
    case 90: return [100 - y, x]
    case 180: return [100 - x, 100 - y]
    case 270: return [y, 100 - x]
    default: return [x, y]
  }
}

export function cropToDisplayedPercent(crop: PdfCropPercent, rotation: number): PdfCropPercent {
  const points = [rotatePoint(crop.left, crop.top, rotation), rotatePoint(crop.right, crop.top, rotation),
    rotatePoint(crop.left, crop.bottom, rotation), rotatePoint(crop.right, crop.bottom, rotation)]
  return { left: Math.min(...points.map(point => point[0])), top: Math.min(...points.map(point => point[1])),
    right: Math.max(...points.map(point => point[0])), bottom: Math.max(...points.map(point => point[1])) }
}

export function displayedToCropPercent(crop: PdfCropPercent, rotation: number): PdfCropPercent {
  return cropToDisplayedPercent(crop, -rotation)
}
