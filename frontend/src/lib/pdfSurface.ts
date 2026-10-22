/** Bound bitmap memory independently of a PDF's requested display size. */
export function pdfRasterSize(width: number, height: number, deviceRatio = 1, maxPixels = 16_000_000) {
  if (![width, height, maxPixels].every(value => Number.isFinite(value) && value > 0))
    throw new Error('PDF page dimensions must be positive and finite.')
  const limit = Math.max(1, Math.floor(maxPixels))
  const ratio = Math.min(Number.isFinite(deviceRatio) && deviceRatio > 0 ? deviceRatio : 1,
    2, Math.sqrt(limit / width / height))
  let pixelWidth = Math.max(1, Math.floor(width * ratio))
  let pixelHeight = Math.max(1, Math.floor(height * ratio))
  // An unusually narrow page can reach the one-pixel minimum on one axis.
  if (pixelWidth * pixelHeight > limit) {
    if (pixelWidth >= pixelHeight) pixelWidth = Math.max(1, Math.floor(limit / pixelHeight))
    else pixelHeight = Math.max(1, Math.floor(limit / pixelWidth))
  }
  return {width:pixelWidth, height:pixelHeight, scaleX:pixelWidth / width, scaleY:pixelHeight / height}
}

