import { afterEach, describe, expect, it, vi } from 'vitest'
import { confirmSignatureTransformation, signatureStatusLabel } from './signatures'
import type { MatterDocument } from './types'

function document(signature_status: MatterDocument['signature_status']): MatterDocument {
  return { id:'d', matter_id:'m', title:'Synthetic signed input', kind:'other', part:1,
    group_key:'', group_role:'member', label:'', label_status:'unreviewed', label_source:'none',
    document_date:'', date_status:'unknown', date_source:'none', date_note:'', source_file_mtime:'',
    position:0, page_count:1, ocr_status:'not_requested', original_name:'source.pdf', source_sha256:'x',
    certificate_signature_status: signature_status === 'certificate_signature_detected' ? 'detected' : 'not_detected',
    certificate_signature_count: signature_status === 'certificate_signature_detected' ? 1 : 0,
    signature_inspection_note:'Synthetic', visible_signature_status:'unreviewed', signature_status,
    revision:1, created_at:'', updated_at:'' }
}

describe('signature transformation warnings', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('does not interrupt reviewed unsigned documents', () => {
    const confirm = vi.fn()
    vi.stubGlobal('window', {confirm})
    expect(confirmSignatureTransformation([document('unsigned')], 'OCR')).toEqual({proceed:true, acknowledged:false})
    expect(confirm).not.toHaveBeenCalled()
  })

  it('requires explicit confirmation for certificate indicators', () => {
    const confirm = vi.fn().mockReturnValue(true)
    vi.stubGlobal('window', {confirm})
    expect(confirmSignatureTransformation([document('certificate_signature_detected')], 'OCR'))
      .toEqual({proceed:true, acknowledged:true})
    expect(confirm).toHaveBeenCalledWith(expect.stringContaining('may invalidate or omit signature protections'))
  })

  it('labels inconclusive inspection without claiming validity', () => {
    expect(signatureStatusLabel('inspection_inconclusive')).toContain('inconclusive')
  })
})
