export type MemoPartyRole = 'Petitioner' | 'Respondent'

export type MemoPartyRow = {
  role: MemoPartyRole
  name: string
  fatherOrHusbandName: string
  address: string
  email: string
  mobile: string
}

const MEMO_PARTY_HEADINGS = [
  'Petitioner or respondent ?',
  "Plaintiff/Petitioner's Name",
  'Father/Husband Name',
  'Address',
  'e-Mail',
  'Mobile',
]

function csvCell(value: string) {
  return `"${value.replace(/"/g, '""')}"`
}

export function buildMemoPartiesCsv(parties: MemoPartyRow[]) {
  const rows = parties.map(party => [
    party.role,
    party.name,
    party.fatherOrHusbandName,
    party.address,
    party.email,
    party.mobile,
  ])
  return '\uFEFF' + [MEMO_PARTY_HEADINGS, ...rows]
    .map(row => row.map(csvCell).join(','))
    .join('\r\n')
}
