export type ImportedIndexRow = {title:string; document:string; page:string}

export function parseIndexCsv(input:string): ImportedIndexRow[] {
  const text = input.replace(/^\uFEFF/, '')
  if (!text.trim()) throw new Error('The index file is empty.')
  const delimiter = text.slice(0, text.indexOf('\n') < 0 ? undefined : text.indexOf('\n')).includes('\t') ? '\t' : ','
  const rows:string[][] = []
  let row:string[] = []
  let cell = ''
  let quoted = false
  for (let i=0; i<text.length; i++) {
    const char = text[i]
    if (quoted) {
      if (char === '"' && text[i+1] === '"') {cell += '"'; i++}
      else if (char === '"') quoted = false
      else cell += char
    } else if (char === '"' && !cell) quoted = true
    else if (char === delimiter) {row.push(cell.trim()); cell = ''}
    else if (char === '\n') {row.push(cell.trim()); rows.push(row); row=[]; cell=''}
    else if (char !== '\r') cell += char
  }
  if (quoted) throw new Error('The index file has an unclosed quoted field.')
  if (cell || row.length) {row.push(cell.trim()); rows.push(row)}
  const header = rows.shift()?.map(value => value.trim().toLowerCase()) || []
  if (header.join('|') !== 'title|document|page') {
    throw new Error('Use three columns in this order: title,document,page (CSV or tab-separated).')
  }
  const content = rows.filter(fields => fields.some(value => value))
  if (!content.length) throw new Error('Add at least one index row below the header.')
  if (content.length > 100) throw new Error('Import at most 100 index rows at a time.')
  if (content.some(fields => fields.length !== 3)) throw new Error('Every index row must have exactly three columns.')
  return content.map(fields => ({title:fields[0], document:fields[1], page:fields[2]}))
}
