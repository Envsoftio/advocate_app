import type { MatterDocument, SignatureStatus } from './types'

const risky = new Set<SignatureStatus>([
  'certificate_signature_detected', 'inspection_inconclusive', 'visible_signature_only',
])

export function signatureStatusLabel(status: SignatureStatus): string {
  return ({
    certificate_signature_detected: 'Certificate signature indicators detected',
    no_certificate_signature_detected: 'No certificate signature detected · visible marks unreviewed',
    inspection_inconclusive: 'Signature inspection inconclusive',
    visible_signature_only: 'Visible/scanned signature only · reviewer marked',
    unsigned: 'No signature detected · reviewer marked',
  } as Record<SignatureStatus, string>)[status]
}

export function signatureRiskDocuments(documents: MatterDocument[]): MatterDocument[] {
  return documents.filter(document => risky.has(document.signature_status))
}

export function confirmSignatureTransformation(documents: MatterDocument[], action: string):
  { proceed: boolean, acknowledged: boolean } {
  const affected = signatureRiskDocuments(documents)
  if (!affected.length) return { proceed: true, acknowledged: false }
  const names = affected.slice(0, 5).map(document => `“${document.title}”`).join(', ')
  const remainder = affected.length > 5 ? ` and ${affected.length - 5} more` : ''
  const proceed = window.confirm(
    `${action} will change ${names}${remainder}. Certificate validity and visible signatures are unverified; ` +
    'edits may invalidate or omit signature protections. Imported originals remain unchanged. Continue?')
  return { proceed, acknowledged: proceed }
}
