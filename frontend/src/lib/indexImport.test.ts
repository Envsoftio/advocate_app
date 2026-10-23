import { describe, expect, it } from 'vitest'
import { parseIndexCsv } from './indexImport'

describe('index import parser', () => {
  it('preserves quoted commas and Hindi titles', () => {
    expect(parseIndexCsv('title,document,page\r\n"पत्र, भाग १",Affidavit.pdf,2\r\n'))
      .toEqual([{title:'पत्र, भाग १', document:'Affidavit.pdf', page:'2'}])
  })
  it('accepts tab-separated rows and rejects missing columns', () => {
    expect(parseIndexCsv('title\tdocument\tpage\nA\tDoc\t1'))
      .toEqual([{title:'A', document:'Doc', page:'1'}])
    expect(() => parseIndexCsv('title,page\nA,1')).toThrow(/three columns/)
  })
})
