import { describe, expect, it } from 'vitest'
import { buildMemoPartiesCsv } from './delhi'

describe('buildMemoPartiesCsv', () => {
  it('uses the court template field labels and preserves Excel-compatible UTF-8', () => {
    const csv = buildMemoPartiesCsv([{
      role: 'Petitioner',
      name: 'Synthetic Petitioner',
      fatherOrHusbandName: 'Synthetic Parent',
      address: 'New Delhi',
      email: 'synthetic@example.invalid',
      mobile: '0000000000',
    }])

    expect(csv.startsWith('\uFEFF')).toBe(true)
    expect(csv).toContain('"Petitioner or respondent ?","Plaintiff/Petitioner\'s Name"')
    expect(csv).toContain('"Petitioner","Synthetic Petitioner","Synthetic Parent"')
  })

  it('escapes commas and quotation marks', () => {
    const csv = buildMemoPartiesCsv([{
      role: 'Respondent',
      name: 'Example "Authority", Delhi',
      fatherOrHusbandName: '',
      address: '',
      email: '',
      mobile: '',
    }])

    expect(csv).toContain('"Example ""Authority"", Delhi"')
  })
})
