import type { MatterDocument, ExportManifest } from './types'
export function needsDocumentReview(doc: MatterDocument) {
  return doc.label_status !== 'confirmed' || doc.date_status !== 'confirmed' || doc.ocr_status === 'needs_ocr'
}
export function summarizeMatter(documents: MatterDocument[], outputs: ExportManifest[]) {
  const latest = [...outputs].sort((a,b) => b.created_at.localeCompare(a.created_at))[0]
  return { documents: documents.length, pages: documents.reduce((sum,d)=>sum+d.page_count,0),
    review: documents.filter(needsDocumentReview).length,
    package: latest ? latest.stale ? 'Stale' : 'Current' : 'Not built' }
}
export interface ImportEntry {
  id: number
  file: File
  status: 'ready' | 'unsupported' | 'duplicate' | 'oversize' | 'processing' | 'complete' | 'failed' | 'cancelled'
  message?: string
  pages?: number
  hash?: string
}
