import { openBuildTool, submitBundleBuild, openOutputTools } from './navigation'
import { includeAllDocuments, openReaderView, closeDocumentActions, closeWorkspaceNavigation, openDocumentActions, openDocumentBrowser, openWorkspaceNavigation, openWorkspaces } from './navigation'
import { expect, test } from '@playwright/test'
import { createHash } from 'node:crypto'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'

const samples = resolve(import.meta.dirname, '../../samples')
const backend = `http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT || 8768}`
const headers = { Authorization: 'Bearer e2e-isolated-token' }

test('court-neutral checklist and reviewed CSV index import persist', async ({page, request}) => {
  const created = await request.post(`${backend}/matters`, {headers, data:{
    name:'E2E Court Neutral Review', court_profile:'general-demo', max_pages_per_volume:300,
  }})
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, {headers,
    multipart:{files:{name:'01_petition.pdf', mimeType:'application/pdf',
      buffer:await readFile(resolve(samples,'01_petition.pdf'))}},
  })
  const doc = (await uploaded.json())[0]
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Court Neutral Review'})).toBeVisible()
  await page.getByRole('button', {name:'Index & bookmarks'}).click()
  await page.locator('.structure-tabs').getByRole('button',{name:'Bookmarks',exact:true}).click()
  await page.locator('.bookmark-import>summary').click()
  const importer = page.getByRole('region', {name:'Import reviewed index rows'})
  await importer.getByLabel('Choose index file').setInputFiles({name:'index.csv',
    mimeType:'text/csv', buffer:Buffer.from('title,document,page\n"Petition, reviewed",unknown.pdf,2\n')})
  await importer.getByLabel('Source document').selectOption(doc.id)
  await importer.getByLabel('Checked target').check()
  await importer.getByRole('button', {name:'Add 1 reviewed bookmark(s)'}).click()
  await expect(importer.getByText('1 reviewed row(s) added as manual bookmarks.')).toBeVisible()
  const bookmarks = await (await request.get(`${backend}/matters/${matterId}/bookmarks`, {headers})).json()
  expect(bookmarks[0]).toMatchObject({title:'Petition, reviewed', document_id:doc.id,
    page_index:1, target_revision:doc.revision})
  await importer.getByLabel('Choose index file').setInputFiles({name:'index.csv',
    mimeType:'text/csv', buffer:Buffer.from('title,document,page\n"Petition, reviewed",unknown.pdf,2\n')})
  await importer.getByLabel('Source document').selectOption(doc.id)
  await importer.getByLabel('Checked target').check()
  await importer.getByRole('button', {name:'Add 1 reviewed bookmark(s)'}).click()
  await expect(importer.getByRole('alert')).toContainText('already exists as a bookmark')
  expect((await (await request.get(`${backend}/matters/${matterId}/bookmarks`, {headers})).json())).toHaveLength(1)
  await page.getByRole('button', {name:/Preflight checks/}).click()
  await page.getByRole('button',{name:'Open review checklist',exact:true}).click()
  const checklist = page.getByRole('region', {name:'User-authored filing review checklist'})
  await checklist.getByLabel('Court or forum').fill('Synthetic forum')
  await checklist.getByLabel('Case type').fill('Synthetic case')
  await checklist.getByLabel('Filing stage').fill('Initial')
  await checklist.getByRole('button', {name:'Add item'}).click()
  await checklist.getByRole('textbox', {name:'Item 1'}).fill('Review petition')
  await checklist.getByLabel('Part').selectOption('1')
  await checklist.getByRole('combobox', {name:'Review', exact:true}).selectOption('present')
  await checklist.getByRole('button', {name:'Save checklist'}).click()
  await expect(checklist.getByText('Review checklist saved.', {exact:false})).toBeVisible()
  const review = await (await request.get(`${backend}/matters/${matterId}/filing-review`, {headers})).json()
  expect(review).toMatchObject({court:'Synthetic forum', case_type:'Synthetic case', stage:'Initial',
    items:[{title:'Review petition', part:1, status:'present'}]})
  await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click()
  await openBuildTool(page,'Preview')
  const presets=page.locator('.bundle-structure')
  await presets.locator('.front-settings details>summary').click()
  await presets.getByRole('combobox', {name:'Index layout'}).selectOption('readable')
  await presets.getByRole('checkbox', {name:'Also create separate technical Part PDFs'}).check()
  await presets.getByRole('button', {name:'Apply settings',exact:true}).click()
  await presets.getByRole('button',{name:'Back to page map',exact:true}).click()
  await expect(presets.getByText('Technical Part PDFs:', {exact:false})).toBeVisible()
  await submitBundleBuild(page)
  await expect(page.locator('.generated-files')).toContainText('part-01.pdf', {timeout:90_000})
  const exports = await (await request.get(`${backend}/matters/${matterId}/exports`, {headers})).json()
  expect(exports[0].part_extracts).toMatchObject([{part:1, pages:doc.page_count}])
})

test('build workspace explains a late addition against a named prior build', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Rebuild Comparison', court_profile: 'general-demo', max_pages_per_volume: 300,
  } })
  const matterId = (await created.json()).id as string
  const upload = async (name:string) => request.post(`${backend}/matters/${matterId}/documents?included=true`, {headers,
    multipart: {files: {name, mimeType:'application/pdf', buffer:await readFile(resolve(samples, name))}},
  })
  expect((await upload('01_petition.pdf')).ok()).toBeTruthy()
  const started = await request.post(`${backend}/matters/${matterId}/build`, {headers})
  expect(started.status()).toBe(202)
  const jobId = (await started.json()).id as string
  await expect.poll(async () => (await (await request.get(`${backend}/jobs/${jobId}`, {headers})).json()).status,
    {timeout:90_000}).toBe('completed')
  const priorId = (await (await request.get(`${backend}/matters/${matterId}/exports`, {headers})).json())[0].id as string
  expect((await upload('03_affidavit.pdf')).ok()).toBeTruthy()
  const rebuilt = await request.post(`${backend}/matters/${matterId}/build`, {headers})
  const rebuiltJobId = (await rebuilt.json()).id as string
  await expect.poll(async () => (await (await request.get(`${backend}/jobs/${rebuiltJobId}`, {headers})).json()).status,
    {timeout:90_000}).toBe('completed')
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Rebuild Comparison'})).toBeVisible()
  await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click()
  await page.getByRole('combobox', {name:'Saved build'}).selectOption(priorId)
  await page.locator('.build-workspace').getByRole('button',{name:'Review changes',exact:true}).click()
  const comparison = page.getByRole('region', {name:'What changed since the selected build'})
  await expect(comparison.getByRole('button', {name:'03_affidavit',exact:true})).toBeVisible()
  await expect(comparison.getByText('added', {exact:true})).toBeVisible()
  await comparison.getByText('Recorded hashes, page ranges and settings',{exact:true}).click()
  await expect(comparison.getByText('Page shifts may affect citations outside this package.', {exact:false})).toBeVisible()
  await expect(page.locator('.build-workspace')).toContainText(priorId.slice(0, 8))
})

test('Bates and annexure policy update the page-map preview and actual-page mark preview', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Bates and separators', court_profile: 'general-demo',
    max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: '03_affidavit.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '03_affidavit.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  const document = (await uploaded.json())[0]
  const labelled = await request.patch(`${backend}/documents/${document.id}`, { headers,
    data: {kind:'annexure',label:'E2E-1',label_status:'confirmed',label_source:'user'},
  })
  expect(labelled.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Bates and separators'})).toBeVisible()
  await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click()
  await openBuildTool(page,'Technical marks & Part extracts')
  const presets = page.getByRole('region', {name:'Reusable output presets'})
  await expect(presets.getByText('Output policy: No Bates identifiers · No added annexure mark · 0 document exceptions')).toBeVisible()
  await expect(presets.locator('details.preset-section')).toHaveCount(3)
  await presets.locator('details.preset-section').filter({hasText:'Numbering'}).locator('summary').click()
  await presets.locator('details.preset-section').filter({hasText:'Annexure output'}).locator('summary').click()
  await presets.getByRole('checkbox', {name:'Bates identifiers'}).check()
  await presets.getByRole('textbox', {name:'Prefix'}).fill('E2E-')
  await presets.getByRole('spinbutton', {name:'Start'}).fill('42')
  await presets.getByRole('checkbox', {name:'Number index pages'}).check()
  await presets.getByRole('checkbox', {name:'Number separator pages'}).check()
  await presets.getByRole('combobox', {name:'Default for annexures'}).selectOption('separator')
  await expect(presets.getByText('Output policy: Bates E2E-000042 at bottom left · Separator before each annexure · 0 document exceptions · Unsaved')).toBeVisible()
  await presets.getByRole('button', {name:'Save numbering & annexure policy'}).click()
  await expect(presets.getByText('Output policy saved.', {exact:false})).toBeVisible()
  await openBuildTool(page,'Preview')
  await page.getByRole('button',{name:'Back to page map',exact:true}).click()
  await page.locator('.structure-map .page-map-details summary').click()
  await expect(page.locator('.structure-map').getByText('separator at physical 2', {exact:false})).toBeVisible()
  await openBuildTool(page,'Technical marks & Part extracts')
  const policy = await (await request.get(`${backend}/matters/${matterId}/output-marks`, {headers})).json()
  expect(policy.bates_start).toBe(42)
  expect(policy.annexure_mode).toBe('separator')
  await presets.locator('details.preset-section').filter({hasText:'Annexure output'}).locator('summary').click()
  await presets.getByRole('button', {name:'Preview page'}).click()
  await openDocumentActions(page)
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Output', exact:true}).click()
  await page.getByRole('button', {name:/Output marks/}).click()
  await expect(page.locator('.pdf-output-mark-preview.bates')).toContainText('E2E-000044')
})

test('technical output preset is saved and explicitly applied in the build workspace', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Output Preset', court_profile: 'general-demo', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Output Preset'})).toBeVisible()
  await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click()
  await openBuildTool(page,'Presets')
  const presets = page.getByRole('dialog', { name: 'Bundle presets' })
  await presets.getByRole('textbox', { name: 'Preset name' }).fill('E2E chamber technical')
  await presets.getByRole('button', { name: 'Save preset' }).click()
  await expect(presets.getByText('E2E chamber technical v1 saved locally.', {exact:false})).toBeVisible()
  await openBuildTool(page)
  await page.getByRole('spinbutton', { name: /Maximum pages per volume/ }).fill('6')
  await page.getByRole('button',{name:'Preview changes',exact:true}).click()
  await page.getByRole('button',{name:'Save choices',exact:true}).click()
  await expect(page.getByRole('button',{name:'Save choices',exact:true})).toBeHidden()
  await expect(page.getByRole('spinbutton', { name: /Maximum pages per volume/ })).toHaveValue('6')
  await openBuildTool(page,'Presets')
  await presets.getByLabel('E2E chamber technical',{exact:false}).check()
  await presets.getByRole('button',{name:'Use selected preset'}).click()
  await expect(page.locator('.bundle-structure')).toContainText('Applied: E2E chamber technical v1')
  await openBuildTool(page)
  await expect(page.getByRole('spinbutton', { name: /Maximum pages per volume/ })).toHaveValue('10')
  const snapshot = await request.get(`${backend}/matters/${matterId}/output-preset`, {headers})
  expect((await snapshot.json()).version).toBe(1)
})

test('Hindi OCR selection reaches the local engine', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Hindi OCR', court_profile: 'general-demo', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: '05_hindi_scan.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '05_hindi_scan.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Hindi OCR'})).toBeVisible()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab',{name:'OCR',exact:true}).click()
  await page.getByRole('combobox', { name: 'OCR language' }).selectOption('eng+hin')
  const submitted = page.waitForRequest(request => request.url().endsWith('/ocr') &&
    request.method() === 'POST')
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab',{name:'OCR',exact:true}).click()
  await page.getByRole('button', { name: 'Run English + Hindi OCR' }).click()
  expect((await submitted).postDataJSON().language).toBe('eng+hin')
})

async function dragWithPointer(page: import('@playwright/test').Page,
                               source: import('@playwright/test').Locator,
                               target: import('@playwright/test').Locator) {
  const from = await source.boundingBox()
  const to = await target.boundingBox()
  expect(from).not.toBeNull()
  expect(to).not.toBeNull()
  await page.mouse.move(from!.x + from!.width / 2, from!.y + from!.height / 2)
  await page.mouse.down()
  await page.mouse.move(from!.x + from!.width / 2, from!.y + from!.height / 2 + 8, {steps:2})
  await page.mouse.move(to!.x + to!.width / 2, to!.y + to!.height / 2, {steps:8})
  await page.mouse.up()
}

test('visible-signature review warns before a working-copy transformation', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Signature Warning', court_profile: 'general-demo', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name:'01_petition.pdf', mimeType:'application/pdf',
      buffer:await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  const document = (await uploaded.json())[0]
  const reviewed = await request.patch(`${backend}/documents/${document.id}`, { headers,
    data: {visible_signature_status:'present'},
  })
  expect(reviewed.ok()).toBeTruthy()
  expect((await reviewed.json()).signature_status).toBe('visible_signature_only')

  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Signature Warning'})).toBeVisible()
  await openDocumentActions(page)
  await page.getByRole('button', {name:/Document details/}).click()
  await openDocumentActions(page)
  const details = page.getByRole('region', {name:'Document details'})
  await expect(details).toContainText('Visible/scanned signature only')
  await expect(details.getByRole('combobox', {name:'Visible/scanned signature review'}))
    .toHaveValue('present')

  page.once('dialog', dialog => void dialog.dismiss())
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Pages', exact:true}).click()
  await page.getByRole('button', {name:'Rotate right'}).click()
  await expect.poll(async () => {
    const docs = await (await request.get(`${backend}/matters/${matterId}/documents`, {headers})).json()
    return docs[0].revision
  }).toBe(1)

  page.once('dialog', dialog => {
    expect(dialog.message()).toContain('Imported originals remain unchanged')
    void dialog.accept()
  })
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Pages', exact:true}).click()
  await page.getByRole('button', {name:'Rotate right'}).click()
  await expect.poll(async () => {
    const docs = await (await request.get(`${backend}/matters/${matterId}/documents`, {headers})).json()
    return docs[0].revision
  }).toBe(2)
})

test('document table preserves explicit selection and previews batch ordering before commit', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Document Table', court_profile: 'general-demo', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  for (const name of ['03_affidavit.pdf', '01_petition.pdf', '02_impugned_order.pdf']) {
    const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
      multipart: { files: { name, mimeType: 'application/pdf', buffer: await readFile(resolve(samples, name)) } },
    })
    expect(uploaded.ok()).toBeTruthy()
  }
  const initial = await (await request.get(`${backend}/matters/${matterId}/documents`, { headers })).json()

  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Document Table'})).toBeVisible()
  await openDocumentBrowser(page)
  await page.getByRole('button', { name: 'Open table' }).click()
  const register = page.getByRole('region', { name: 'Editable document table' })
  await expect(register.getByRole('heading', { name: 'Batch review' })).toBeVisible()
  const firstRow = register.locator('tr', { hasText: '03_affidavit.pdf' })
  const lastRow = register.locator('tr', { hasText: '02_impugned_order.pdf' })
  await firstRow.getByRole('checkbox', { name: /Select/ }).check()
  await lastRow.getByRole('checkbox', { name: /Select/ }).check()
  await expect(register.getByText('2 selected', { exact: true })).toBeVisible()
  await register.getByRole('searchbox', { name: 'Filter rows' }).fill('impugned')
  await expect(register.getByText('2 selected', {exact:true})).toHaveAttribute('title','1 visible in this filter')
  await register.getByRole('button',{name:'More tools',exact:true}).click()

  await register.getByRole('combobox', { name: 'Document type',exact:true }).selectOption('annexure')
  await register.getByRole('combobox', { name: 'Assign selected Part',exact:true }).selectOption('2')
  await register.getByRole('checkbox', { name: 'Change selected group' }).check()
  await register.locator('.batch-advanced>label input:not([type=checkbox])').fill('selected-pair')
  await register.getByRole('button', { name: 'Apply batch change' }).click()
  await expect(register.getByText('Updated 2 selected document(s) in one undoable action.')).toBeVisible()
  const changed = await (await request.get(`${backend}/matters/${matterId}/documents`, { headers })).json()
  expect(changed.filter((doc:{kind:string}) => doc.kind === 'annexure')).toHaveLength(2)
  expect(changed.find((doc:{original_name:string}) => doc.original_name === '01_petition.pdf').kind).toBe('other')

  await page.getByRole('button', { name: /^Undo(?: last matter change)?$/, exact: true }).first().click()
  await expect(page.getByText('Last edit undone.')).toBeVisible()
  const undone = await (await request.get(`${backend}/matters/${matterId}/documents`, { headers })).json()
  expect(undone.every((doc:{kind:string,part:number,group_key:string}) =>
    doc.kind === 'other' && doc.part === 1 && doc.group_key === '')).toBeTruthy()

  await register.getByRole('searchbox', { name: 'Filter rows' }).fill('')
  await register.getByRole('combobox', { name: 'Order method' }).selectOption('filename')
  await register.getByRole('button', { name: 'Preview order', exact: true }).click()
  const preview = register.getByRole('dialog', { name: 'Sort preview' })
  await expect(preview).toContainText('Proposed filename order')
  await preview.getByRole('button', { name: 'Cancel' }).click()
  await expect(preview).toBeHidden()
  const afterCancel = await (await request.get(`${backend}/matters/${matterId}/documents`, { headers })).json()
  expect(afterCancel.map((doc:{id:string}) => doc.id)).toEqual(initial.map((doc:{id:string}) => doc.id))

  await register.getByRole('button', { name: 'Preview order', exact: true }).click()
  await preview.getByRole('button', { name: 'Apply order' }).click()
  await expect(register.getByText('Applied filename order.')).toBeVisible()
  const sorted = await (await request.get(`${backend}/matters/${matterId}/documents`, { headers })).json()
  expect(sorted.map((doc:{original_name:string}) => doc.original_name)).toEqual([
    '01_petition.pdf', '02_impugned_order.pdf', '03_affidavit.pdf'])

  await register.getByRole('button',{name:'More tools',exact:true}).click()
  const petitionRow = register.locator('tr', { hasText: '01_petition.pdf' })
  await petitionRow.locator('input[type="date"]').fill('2026-10-16')
  const dateSaved = page.waitForResponse(response => response.url().includes('/metadata-review') &&
    response.request().method() === 'PUT')
  await petitionRow.locator('summary').click()
  await petitionRow.getByRole('button', { name: 'Save row' }).click()
  expect((await dateSaved).ok()).toBeTruthy()
  const dated = await (await request.get(`${backend}/matters/${matterId}/documents`, { headers })).json()
  const petition = dated.find((doc:{original_name:string}) => doc.original_name === '01_petition.pdf')
  expect(petition).toMatchObject({document_date:'2026-10-16', date_status:'confirmed', date_source:'user'})
})

test('label proposal preview cancels cleanly and applies one undoable grouped mapping', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Label Proposal', court_profile: 'general-demo', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  for (const name of ['01_petition.pdf', '02_impugned_order.pdf', '03_affidavit.pdf']) {
    const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
      multipart: { files: { name, mimeType: 'application/pdf', buffer: await readFile(resolve(samples, name)) } },
    })
    expect(uploaded.ok()).toBeTruthy()
  }
  const docs = await (await request.get(`${backend}/matters/${matterId}/documents`, { headers })).json()
  const grouped = await request.patch(`${backend}/matters/${matterId}/documents/batch`, { headers,
    data: { ids:[docs[0].id, docs[1].id], group_key:'linked-annexures' },
  })
  expect(grouped.ok()).toBeTruthy()

  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Label Proposal'})).toBeVisible()
  await openDocumentBrowser(page)
  await page.getByRole('button', { name: 'Open table' }).click()
  const register = page.getByRole('region', { name: 'Editable document table' })
  const designatedParent = register.locator('tr', { hasText: '02_impugned_order.pdf' })
  await designatedParent.locator('summary').click()
  await designatedParent.getByRole('combobox', { name: 'Hierarchy',exact:true })
    .selectOption('parent')
  await designatedParent.getByRole('button', { name: 'Save row' }).click()
  await expect(register.getByText('Changes saved in one undoable action.')).toBeVisible()
  await register.getByRole('button',{name:'More tools',exact:true}).click()
  await register.getByRole('checkbox', { name: 'Select all filtered documents' }).check()
  await register.getByRole('combobox', { name: 'Method', exact: true }).selectOption('grouped_suffix')
  await register.getByRole('combobox', { name: 'Suffix style' }).selectOption('upper_alpha')
  await register.getByRole('textbox', { name: 'Separator' }).fill('.')
  await register.getByRole('button', { name: 'Preview label changes' }).click()
  const preview = register.getByRole('dialog', { name: 'Label proposal preview' })
  await expect(preview.getByRole('textbox', { name: 'Proposed label for 01_petition' })).toHaveValue('P-1.A')
  await expect(preview.getByRole('textbox', { name: 'Proposed label for 02_impugned_order' })).toHaveValue('P-1')
  await expect(preview.getByRole('textbox', { name: 'Proposed label for 03_affidavit' })).toHaveValue('P-2')
  await preview.getByRole('button', { name: 'Cancel without changes' }).click()
  await expect(preview).toBeHidden()
  let current = await (await request.get(`${backend}/matters/${matterId}/documents`, { headers })).json()
  expect(current.every((doc:{label:string}) => doc.label === '')).toBeTruthy()

  await register.getByRole('button', { name: 'Preview label changes' }).click()
  await preview.getByRole('button', { name: 'Apply reviewed mapping' }).click()
  await expect(register.getByText('Applied 3 reviewed label mapping(s) in one undoable action.')).toBeVisible()
  current = await (await request.get(`${backend}/matters/${matterId}/documents`, { headers })).json()
  expect(current.map((doc:{label:string}) => doc.label)).toEqual(['P-1.A', 'P-1', 'P-2'])
  expect(current.every((doc:{label_status:string,label_source:string}) =>
    doc.label_status === 'confirmed' && doc.label_source === 'proposal')).toBeTruthy()

  await page.getByRole('button', { name: /^Undo(?: last matter change)?$/, exact: true }).first().click()
  await expect(page.getByText('Last edit undone.')).toBeVisible()
  current = await (await request.get(`${backend}/matters/${matterId}/documents`, { headers })).json()
  expect(current.every((doc:{label:string}) => doc.label === '')).toBeTruthy()
})

test('the workspace build flow blocks generation when settings cannot save', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Bundle Flow', court_profile: 'general-demo',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  for (const name of ['01_petition.pdf', '02_impugned_order.pdf']) {
    const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
      multipart: { files: { name, mimeType: 'application/pdf', buffer: await readFile(resolve(samples, name)) } },
    })
    expect(uploaded.ok()).toBeTruthy()
  }

  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Bundle Flow'})).toBeVisible()
  await page.getByRole('button', { name: 'Index & bookmarks' }).click()
  await expect(page.getByRole('region', { name: 'Index and page number preview' })).toContainText('1 volume(s)')
  await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click()

  let buildRequests = 0
  await page.route(`**/matters/${matterId}`, async route => {
    if (route.request().method() === 'PATCH') {
      await route.fulfill({ status: 500, contentType: 'application/json', body: JSON.stringify({detail:'synthetic settings failure'}) })
    } else await route.continue()
  })
  await page.route(`**/matters/${matterId}/build`, async route => { buildRequests += 1; await route.continue() })
  const buildPanel = page.locator('.build-workspace')
  await submitBundleBuild(page)
  await expect(page.getByText('synthetic settings failure')).toBeVisible()
  expect(buildRequests).toBe(0)

  await page.unroute(`**/matters/${matterId}`)
  await submitBundleBuild(page)
  await expect(page.getByRole('heading',{name:'Bundle ready for review'})).toBeVisible({ timeout: 90_000 })
  expect(buildRequests).toBe(1)
  await page.getByRole('button', { name: 'Review combined.pdf' }).click()
  const pdfReview = page.getByRole('dialog', { name: 'Review generated PDF' })
  await expect(pdfReview).toBeVisible()
  await pdfReview.getByRole('button', { name: 'Close review' }).click()
})

test('compression exposes measured results and retains a usable PDF', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Compression', court_profile: 'general-demo', max_pages_per_volume: 300,
  } })
  const id = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${id}/documents?included=true`, { headers,
    multipart: { files: { name: '04_scanned_annexure.jpg', mimeType: 'image/jpeg',
      buffer: await readFile(resolve(samples, '04_scanned_annexure.jpg')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  const doc = (await uploaded.json())[0]
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Compression'})).toBeVisible()
  await expect(page.locator('.page-counter input')).toHaveValue('1')
  await expect(page.locator('.page-counter')).toContainText('/ 1')
  await expect(page.getByRole('button', { name: 'Go to page 1' })).toBeVisible()
  await expect(page.locator('.thumbnail-button canvas').first()).toHaveAttribute('data-rendered', 'true')
  await page.getByRole('button', { name: 'Hide thumbnails' }).click()
  await expect(page.locator('.thumbnail-strip')).toHaveCount(0)
  const preview = page.locator('.pdf-paper')
  await page.getByRole('combobox', { name: 'Zoom' }).selectOption('100')
  await expect(page.locator('.pdf-placement-surface')).toHaveAttribute('aria-busy','false')
  const originalWidth = await preview.evaluate((node: HTMLCanvasElement) => node.getBoundingClientRect().width)
  await page.getByRole('combobox', { name: 'Zoom' }).selectOption('50')
  await expect.poll(() => preview.evaluate((node: HTMLCanvasElement) => node.getBoundingClientRect().width)).toBeCloseTo(originalWidth / 2, 0)
  await page.getByRole('combobox', { name: 'Zoom' }).selectOption('25')
  await expect.poll(() => preview.evaluate((node: HTMLCanvasElement) => node.getBoundingClientRect().width)).toBeCloseTo(originalWidth / 4, 0)
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Pages', exact:true}).click()
  await page.getByRole('button', { name: 'Page tools', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Page tools' })
  await dialog.getByRole('button', {name:'Split into PDFs',exact:true}).click()
  await expect(dialog.getByRole('button', {name:'Save split ZIP',exact:true})).toBeDisabled()
  await expect(dialog).toContainText('This PDF has only one page.')
  await dialog.getByRole('button', { name: 'Reduce file size', exact: true }).click()
  await dialog.getByRole('combobox', { name: 'Compression quality' }).selectOption('small')
  await dialog.getByRole('button', { name: 'Compress working PDF' }).click()
  await expect(dialog.getByRole('status')).toContainText('bytes.')
  await expect(dialog.getByRole('status')).toContainText('Completed')
  await expect(dialog.getByRole('button', { name: 'Compress working PDF' })).toBeEnabled()
  const pdf = await request.get(`${backend}/documents/${doc.id}/pdf`, { headers })
  expect(pdf.ok()).toBeTruthy()
  expect((await pdf.body()).subarray(0, 5).toString()).toBe('%PDF-')
})

test('document import undo and redo refresh the workspace list', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Import History', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Import History'})).toBeVisible()
  await expect(page.locator('.doc-item')).toHaveCount(1)
  await page.getByRole('button', { name: /^Undo(?: last matter change)?$/, exact: true }).first().click()
  await expect(page.locator('.doc-item')).toHaveCount(0)
  await page.reload()
  await expect(page.getByRole('button', { name: /^Redo(?: last matter change)?$/, exact: true })).toBeEnabled()
  await page.getByRole('button', { name: /^Redo(?: last matter change)?$/, exact: true }).click()
  await expect(page.locator('.doc-item')).toHaveCount(1)
})

test('manual bookmark survives reload and shows revision review', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Bookmark Matter', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  const docId = (await uploaded.json())[0].id as string
  await page.goto('/')
  await page.getByRole('button', { name: 'Index & bookmarks' }).click()
  await page.locator('.structure-tabs').getByRole('button',{name:'Bookmarks',exact:true}).click()
  await page.getByRole('button',{name:'Add bookmark',exact:true}).click()
  await page.getByRole('textbox', { name: 'Bookmark label' }).fill('Reviewed synthetic page')
  await page.locator('.structure-tabs').getByRole('button',{name:'Bookmarks',exact:true}).click()
  await page.getByRole('combobox', { name: 'Target document' }).selectOption(docId)
  await page.getByRole('button', { name: 'Save bookmark',exact:true }).click()
  await page.locator('.structure-tabs').getByRole('button',{name:'Bookmarks',exact:true}).click()
  await expect(page.getByRole('textbox', { name: 'Bookmark label' })).toHaveValue('Reviewed synthetic page')
  await page.locator('.structure-tabs').getByRole('button',{name:'Index & page map',exact:true}).click()
  await page.locator('.page-map-details>summary').click()
  await expect(page.getByRole('region', { name: 'Index and page number preview' })).toContainText('Reviewed synthetic page')
  await expect(page.getByRole('region', { name: 'Index and page number preview' })).toContainText('combined 2')
  await page.reload()
  await page.getByRole('button', { name: 'Index & bookmarks' }).click()
  await page.locator('.structure-tabs').getByRole('button',{name:'Bookmarks',exact:true}).click()
  await expect(page.getByRole('textbox', { name: 'Bookmark label' })).toHaveValue('Reviewed synthetic page')
  const rotated = await request.post(`${backend}/documents/${docId}/pages/rotate`,
    { headers, data: { page_index: 0, degrees: 90 } })
  expect(rotated.ok()).toBeTruthy()
  await page.reload()
  await page.getByRole('button', { name: 'Index & bookmarks' }).click()
  await page.locator('.structure-tabs').getByRole('button',{name:'Bookmarks',exact:true}).click()
  await expect(page.getByText('Needs resolution', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Confirm selected destination' }).click()
  await page.getByRole('button',{name:'Save bookmark',exact:true}).click()
  await expect(page.getByText('Needs resolution', { exact: true })).toBeHidden()
})

test('index preview updates with draft volume cap and opens the source document', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Index Preview', court_profile: 'general-demo', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  for (const name of ['01_petition.pdf', '02_impugned_order.pdf']) {
    const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
      multipart: { files: { name, mimeType: 'application/pdf', buffer: await readFile(resolve(samples, name)) } },
    })
    expect(uploaded.ok()).toBeTruthy()
  }
  await page.goto('/')
  await expect(page.locator('.matter-index-overview')).toHaveCount(0)
  await page.getByRole('button', { name: 'Index & bookmarks' }).click()
  await expect(page.getByRole('region', { name: 'Index and page number preview' })).toContainText('1 volume(s)')
  await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click()
  await openBuildTool(page)
  const preview = page.locator('.volume-workspace')
  await expect(preview).toContainText('1 volumes')
  await openBuildTool(page)
  await page.getByRole('spinbutton', { name: /Maximum pages per volume/ }).fill('4')
  await page.getByRole('button',{name:'Preview changes',exact:true}).click()
  await expect(preview).toContainText('2 volumes')
  await expect(preview).toContainText('5–7')
  await preview.getByRole('button',{name:'Save choices',exact:true}).click()
  await expect(preview.getByRole('button',{name:'Save choices',exact:true})).toBeHidden()
  await preview.getByRole('button',{name:'Index & page map',exact:true}).click()
  await page.getByRole('button', { name: 'Open 02_impugned_order working copy' }).click()
  await expect(page.locator('.doc-item.selected')).toContainText('02_impugned_order')
  await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click()
  await submitBundleBuild(page)
  await expect(page.getByRole('heading',{name:'Bundle ready for review'})).toBeVisible({timeout:90_000})
  await page.getByRole('button', { name: 'Review combined.pdf' }).click()
  const review = page.getByRole('dialog', { name: 'Review generated PDF' })
  if(await review.getByRole('button',{name:'PDF bookmarks',exact:true}).getAttribute('aria-expanded')!=='true')await review.getByRole('button',{name:'PDF bookmarks',exact:true}).click()
  await expect(review.getByRole('heading', { name: 'Bookmarks', exact: true })).toBeVisible()
  await expect(review.getByRole('button', { name: /02_impugned_order/ })).toBeVisible()
  await expect.poll(() => review.locator('canvas').evaluate((node: HTMLCanvasElement) => node.width)).toBeGreaterThan(0)
  const indexLink=review.getByRole('button',{name:/Go to PDF page/}).first()
  await expect(indexLink).toBeVisible()
  await indexLink.click()
  await expect(review.getByRole('spinbutton',{name:'PDF page number'})).not.toHaveValue('1')
  await openReaderView(page)
  await review.getByRole('button',{name:'Back to previous reading position'}).click()
  await review.getByRole('button',{name:'View options'}).click()
  await expect(review.getByRole('spinbutton',{name:'PDF page number'})).toHaveValue('1')
  await review.getByRole('button',{name:'Search artifact'}).click()
  await review.getByRole('searchbox',{name:'Find text in generated PDF'}).fill('Synthetic')
  await expect(review.getByLabel('Generated PDF search occurrences').getByRole('button').first()).toBeVisible()
  await review.getByLabel('Generated PDF search occurrences').getByRole('button').first().click()
  await expect(review.locator('.pdf-search-highlight.is-active').first()).toBeVisible()
  await review.getByRole('searchbox',{name:'Find text in generated PDF'}).fill('')
  await review.getByRole('button', { name: /02_impugned_order/ }).click()
  await expect(review.getByRole('spinbutton', { name: 'PDF page number' })).not.toHaveValue('1')
  await openReaderView(page)
  await review.getByRole('combobox',{name:'Reading mode'}).selectOption('continuous')
  await expect(review.locator('.canvas-scroll')).toHaveClass(/continuous-reader/)
  await review.getByRole('button', { name: 'Close review' }).click()
  await expect(review).toBeHidden()
})

test('bookmark the current PDF page while preparing a matter', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, {headers, data: {
    name:'E2E Page Bookmark', court_profile:'general-demo', max_pages_per_volume:10,
  }})
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, {headers,
    multipart:{files:{name:'01_petition.pdf',mimeType:'application/pdf',
      buffer:await readFile(resolve(samples,'01_petition.pdf'))}},
  })
  expect(uploaded.ok()).toBeTruthy()
  await page.setViewportSize({width:1024,height:800})
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Page Bookmark'})).toBeVisible()
  await expect.poll(() => page.locator('.preview-panel').evaluate(node => {
    const gap = window.innerHeight - node.getBoundingClientRect().bottom
    return gap >= -20 && gap <= 60
  })).toBe(true)
  const bottomGap = await page.locator('.preview-panel').evaluate(node =>
    window.innerHeight - node.getBoundingClientRect().bottom)
  expect(bottomGap).toBeGreaterThanOrEqual(-20)
  expect(bottomGap).toBeLessThanOrEqual(60)
  await page.setViewportSize({width:1440,height:800})
  await openDocumentActions(page)
  await page.getByRole('button', {name:'Document details',exact:true}).click()
  await expect(page.locator('.inspector-panel')).toBeVisible()
  await expect.poll(() => page.locator('.inspector-panel').evaluate(node => {
    const gap = window.innerHeight - node.getBoundingClientRect().bottom
    return gap >= -20 && gap <= 60
  })).toBe(true)
  const inspectorGap = await page.locator('.inspector-panel').evaluate(node =>
    window.innerHeight - node.getBoundingClientRect().bottom)
  expect(inspectorGap).toBeGreaterThanOrEqual(-20)
  expect(inspectorGap).toBeLessThanOrEqual(60)
  await openDocumentActions(page)
  await page.getByRole('button', {name:'Document details',exact:true}).click()
  await page.getByRole('button', {name:'Next page'}).click()
  await openWorkspaceNavigation(page)
  await page.getByRole('combobox', {name:'Theme'}).selectOption('dark')
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Annotate', exact:true}).click()
  await page.getByRole('button', {name:'Bookmark page'}).click()
  await expect(page.locator('.page-bookmark-editor')).toBeVisible()
  await page.getByRole('textbox', {name:'Bookmark title'}).fill('Reviewed second page')
  await page.getByRole('button', {name:'Add bookmark'}).click()
  await expect(page.getByRole('status').filter({hasText:'Bookmarked page 2'})).toContainText('Bookmarked page 2')
  const noticeContrast = await page.locator('.page-bookmark-notice').evaluate(node => {
    const channel = (value:number) => {
      const normalized = value / 255
      return normalized <= 0.04045 ? normalized / 12.92 : ((normalized + 0.055) / 1.055) ** 2.4
    }
    const luminance = (value:string) => {
      const channels = value.match(/[\d.]+/g)?.slice(0, 3).map(Number) ?? []
      return 0.2126 * channel(channels[0]) + 0.7152 * channel(channels[1]) + 0.0722 * channel(channels[2])
    }
    const style = getComputedStyle(node)
    const foreground = luminance(style.color)
    const background = luminance(style.backgroundColor)
    return (Math.max(foreground, background) + 0.05) / (Math.min(foreground, background) + 0.05)
  })
  expect(noticeContrast).toBeGreaterThanOrEqual(4.5)
  await page.getByRole('button', {name:/Page 2 bookmarks/}).click()
  const savedOnPage = page.getByLabel('Bookmarks on page 2')
  await expect(savedOnPage).toContainText('Reviewed second page')
  await page.getByRole('button', {name:'Next page'}).click()
  await expect(savedOnPage).toBeHidden()
  await page.getByRole('button', {name:'Previous page'}).click()
  await page.getByRole('button', {name:/Page 2 bookmarks/}).click()
  await expect(savedOnPage).toContainText('Reviewed second page')
  await expect(page.locator('.preview-status.error-msg')).toHaveCount(0)
  await expect(page.locator('.pdf-paper')).toHaveCSS('visibility', 'visible')
  const savedDocument = (await uploaded.json())[0]
  await expect.poll(async () => (await (await request.get(`${backend}/reader-position`, {
    headers, params: {key: `${savedDocument.id}:${savedDocument.revision}`},
  })).json())?.page).toBe(2)
  await page.reload()
  await expect(page.getByRole('spinbutton', {name:'Go to page',exact:true})).toHaveValue('2')
  await page.getByRole('button', {name:/Page 2 bookmarks/}).click()
  await expect(savedOnPage).toContainText('Reviewed second page')
  await page.getByRole('button', {name:'Open Index & bookmarks',exact:true}).click()
  await page.locator('.structure-tabs').getByRole('button',{name:'Bookmarks',exact:true}).click()
  await expect(page.getByRole('textbox', {name:'Bookmark label'})).toHaveValue('Reviewed second page')
  await page.locator('.structure-tabs').getByRole('button',{name:'Index & page map',exact:true}).click()
  await page.locator('.page-map-details>summary').click()
  await expect(page.getByRole('region', {name:'Index and page number preview'})).toContainText('combined 3')
})

test('manual bookmark order is keyboard accessible and persists', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Bookmark Keyboard', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  const docId = (await uploaded.json())[0].id as string
  await page.goto('/')
  await page.getByRole('button', { name: 'Index & bookmarks' }).click()
  await page.locator('.structure-tabs').getByRole('button',{name:'Bookmarks',exact:true}).click()
  await page.getByRole('button',{name:'Add bookmark',exact:true}).click()
  await page.getByRole('combobox', { name: 'Target document' }).selectOption(docId)
  await page.getByRole('textbox', { name: 'Bookmark label' }).fill('First')
  await page.getByRole('button', { name: 'Save bookmark',exact:true }).click()
  await expect(page.getByRole('textbox', { name: 'Bookmark label' })).toHaveValue('First')
  await page.locator('.structure-tabs').getByRole('button',{name:'Bookmarks',exact:true}).click()
  await page.getByRole('button',{name:'Add bookmark',exact:true}).click()
  await page.getByRole('combobox', { name: 'Target document' }).selectOption(docId)
  await page.getByRole('textbox', { name: 'Bookmark label' }).fill('Second')
  await page.getByRole('button', { name: 'Save bookmark',exact:true }).click()
  await expect(page.getByRole('textbox', { name: 'Bookmark label' })).toHaveValue('Second')
  await expect(page.getByRole('button',{name:'Add bookmark',exact:true})).toBeEnabled()
  await page.getByRole('button', { name: /Reorder bookmark 2:/ }).focus()
  await page.keyboard.press('ArrowUp')
  await page.getByRole('button',{name:'Save bookmark',exact:true}).click()
  await page.locator('.structure-tabs').getByRole('button',{name:'Bookmarks',exact:true}).click()
  await expect(page.getByRole('textbox', { name: 'Bookmark label' })).toHaveValue('Second')
  await page.reload()
  await page.getByRole('button', { name: 'Index & bookmarks' }).click()
  await page.locator('.structure-tabs').getByRole('button',{name:'Bookmarks',exact:true}).click()
  await expect(page.getByRole('textbox', { name: 'Bookmark label' })).toHaveValue('Second')
})

test('dragging document, bookmark and page order updates the shared index plan', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Drag Index', court_profile: 'general-demo', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  for (const name of ['01_petition.pdf', '02_impugned_order.pdf']) {
    const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
      multipart: { files: { name, mimeType: 'application/pdf', buffer: await readFile(resolve(samples, name)) } },
    })
    expect(uploaded.ok()).toBeTruthy()
  }
  await page.goto('/')
  await expect(page.locator('body')).toHaveCSS('font-family', /^Inter,/)
  await page.getByRole('button', { name: 'Index & bookmarks' }).click()
  const order = page.getByRole('region', { name: 'Index document order' }).locator('.index-order-row')
  await dragWithPointer(page, order.nth(0).locator('.grip'), order.nth(1))
  await expect(order.first()).toContainText('02_impugned_order')
  const preview = page.getByRole('region', { name: 'Index and page number preview' })
  await expect(preview.locator('tbody tr').first()).toContainText('02_impugned_order')
  await expect(preview).toContainText('Printed volume')
  await preview.getByRole('button', { name: 'Bookmark 02_impugned_order first page' }).click()
  await expect(page.getByRole('textbox', { name: 'Bookmark label' })).toHaveValue('02_impugned_order')
  await page.getByRole('button', { name: 'Save bookmark',exact:true }).click()
  await expect(page.getByRole('textbox', { name: 'Bookmark label' })).toHaveValue('02_impugned_order')
  await page.locator('.structure-tabs').getByRole('button',{name:'Index & page map',exact:true}).click()
  await preview.getByRole('button', { name: 'Bookmark 01_petition first page' }).click()
  await page.getByRole('button', { name: 'Save bookmark',exact:true }).click()
  const bookmarks = page.getByRole('region', { name: 'Manual bookmarks' }).locator('.bookmark-tree-row')
  await expect(page.getByRole('textbox', { name: 'Bookmark label' })).toHaveValue('01_petition')
  await expect(page.getByRole('button',{name:'Add bookmark',exact:true})).toBeEnabled()
  await dragWithPointer(page, bookmarks.nth(1).locator('.bookmark-drag-handle'), bookmarks.nth(0))
  await page.getByRole('button',{name:'Save bookmark',exact:true}).click()
  await expect(page.getByRole('textbox', { name: 'Bookmark label' })).toHaveValue('01_petition')
  await expect(page.getByRole('button', { name: 'Add bookmark' })).toBeEnabled()
  await page.reload()
  await page.getByRole('button', { name: 'Index & bookmarks' }).click()
  await page.locator('.structure-tabs').getByRole('button',{name:'Bookmarks',exact:true}).click()
  await expect(page.getByRole('textbox', { name: 'Bookmark label' })).toHaveValue('01_petition')
  await page.locator('.structure-tabs').getByRole('button',{name:'Index & page map',exact:true}).click()
  await expect(order.first()).toContainText('02_impugned_order')
  await expect(preview).toContainText('Source 1')
  await page.setViewportSize({ width: 1440, height: 1100 })
  await page.getByRole('button', { name: 'Documents', exact: true }).click()
  await openDocumentBrowser(page)
  const rows = page.locator('.docs-list .doc-item')
  await expect(rows.first()).toContainText('02_impugned_order')
  await expect(rows.first()).toHaveCSS('user-select', 'none')
  await expect(page.getByRole('textbox', { name: 'Search documents' })).toHaveCSS('user-select', 'text')
  await expect(rows.first()).not.toHaveClass(/document-list-move/)
  await page.evaluate(() => {
    const state = window as Window & {__courtBundleDocumentListAnimated?:boolean}
    state.__courtBundleDocumentListAnimated = false
    const list = document.querySelector('.docs-list')
    if (!list) return
    new MutationObserver(records => {
      for (const record of records) {
        const element = record.target as HTMLElement
        if (element.matches('.doc-item') && element.style.transform && element.style.transition.includes('transform')) {
          state.__courtBundleDocumentListAnimated = true
        }
      }
    }).observe(list, {subtree:true,attributes:true,attributeFilter:['style']})
  })
  const start = await rows.nth(0).locator('.grip').boundingBox()
  const destination = await rows.nth(1).boundingBox()
  expect(start).not.toBeNull()
  expect(destination).not.toBeNull()
  await page.mouse.move(start!.x+14, start!.y+14)
  await page.mouse.down()
  await page.mouse.move(start!.x+14, start!.y+30, {steps:4})
  await page.mouse.move(destination!.x+90, destination!.y+190, {steps:20})
  await page.mouse.move(destination!.x+90, destination!.y+205, {steps:4})
  await page.mouse.up()
  await expect(rows.first()).toContainText('01_petition')
  expect(await page.evaluate(() => (window as Window & {__courtBundleDocumentListAnimated?:boolean}).__courtBundleDocumentListAnimated)).toBe(true)
  await expect.poll(async () => (await (await request.get(`${backend}/matters/${matterId}/documents`, { headers })).json())
    .map((document: {title:string}) => document.title)).toEqual(['01_petition', '02_impugned_order'])
  await page.reload()
  await expect(rows.first()).toContainText('01_petition')
  await openDocumentBrowser(page)
  await rows.first().click()
  const thumbs = page.locator('.thumbnail-strip .thumbnail-button')
  await expect(thumbs).toHaveCount(3)
  const docId = (await (await request.get(`${backend}/matters/${matterId}/documents`, { headers })).json())[0].id as string
  await thumbs.nth(0).dragTo(thumbs.nth(1))
  await expect.poll(async () => (await (await request.get(`${backend}/matters/${matterId}/documents`, { headers })).json())
    .find((document: {id:string}) => document.id === docId)?.revision).toBe(2)
  await expect(page.locator('.page-counter input')).toHaveValue('2')
  await expect(page.locator('.page-counter')).toContainText('/ 3')
})

test('text editing works on a page already marked as rotated', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Rotated Text', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  const docId = (await uploaded.json())[0].id as string
  const rotated = await request.post(`${backend}/documents/${docId}/pages/rotate`, { headers,
    data: { page_index: 0, degrees: 90 },
  })
  expect(rotated.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Rotated Text'})).toBeVisible()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Edit', exact:true}).click()
  await page.getByRole('button', { name: 'Edit text', exact: true }).click()
  const run = page.getByRole('button', { name: /Edit text: DRAFT PETITION/ })
  await expect(run).toBeVisible()
  const canvas = await page.locator('.pdf-paper').boundingBox()
  const box = await run.boundingBox()
  expect(canvas && box).toBeTruthy()
  expect(box!.x).toBeGreaterThanOrEqual(canvas!.x)
  expect(box!.y).toBeGreaterThanOrEqual(canvas!.y)
  expect(box!.x + box!.width).toBeLessThanOrEqual(canvas!.x + canvas!.width + 1)
  expect(box!.y + box!.height).toBeLessThanOrEqual(canvas!.y + canvas!.height + 1)
  await run.click()
  await expect(page.locator('.pdf-inline-editor')).toBeVisible()
  await expect(page.getByRole('textbox', { name: 'Replacement' })).toBeFocused()
  await page.getByRole('textbox', { name: 'Replacement' }).fill('UPDATED PETITION')
  await page.getByRole('button', { name: 'Apply replacement' }).click()
  await expect(page.locator('.text-overlay-panel [role="status"]')).toContainText('Removed and redrew selected text')
})

test('selectable PDF text can be selected and replaced from the preview', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Replace Text', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  expect(created.ok()).toBeTruthy()
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Replace Text'})).toBeVisible()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Edit', exact:true}).click()
  await page.getByRole('button', { name: 'Edit text', exact: true }).click()
  await page.getByRole('button', { name: /Edit text: DRAFT PETITION/ }).click()
  await expect(page.getByRole('textbox', { name: 'Replacement' })).toHaveValue('DRAFT PETITION - EXAMPLE ONLY')
  await expect(page.locator('.pdf-inline-editor')).toBeVisible()
  await page.getByRole('button', { name: 'Cancel', exact: true }).click()
  await expect(page.locator('.pdf-inline-editor')).toHaveCount(0)
  await page.getByRole('button', { name: /Edit text: DRAFT PETITION/ }).click()
  await page.getByRole('textbox', { name: 'Replacement' }).fill('UPDATED PETITION WITH MORE DETAILS')
  await page.route('**/documents/*/pdf?revision=*', async route => {
    await new Promise(resolve => setTimeout(resolve, 350))
    await route.continue()
  })
  await page.getByRole('button', { name: 'Apply replacement' }).click()
  await expect(page.locator('.pdf-placement-surface')).toHaveClass(/is-edit-refreshing/)
  await expect(page.locator('.pdf-edit-snapshot')).toBeVisible()
  await expect(page.locator('.text-overlay-panel [role="status"]')).toContainText('Removed and redrew selected text')
  await expect(page.locator('.pdf-edit-snapshot')).toHaveCount(0)
  await page.unroute('**/documents/*/pdf?revision=*')
  await expect.poll(async () => {
    const result = await request.get(`${backend}/matters/${matterId}/search?q=UPDATED%20PETITION%20WITH%20MORE%20DETAILS`, { headers })
    return (await result.json()).matches.length
  }).toBe(1)
  const old = await request.get(`${backend}/matters/${matterId}/search?q=DRAFT%20PETITION`, { headers })
  expect((await old.json()).matches.filter((match: {page:number}) => match.page === 1)).toHaveLength(0)
  await page.getByRole('button', { name: /Edit text: UPDATED PETITION/ }).click()
  await page.getByRole('textbox', { name: 'Replacement' }).fill('W'.repeat(200))
  await page.getByRole('button', { name: 'Apply replacement' }).click()
  await expect(page.locator('.toast.error')).toContainText('No clear room for this replacement')
  expect((await page.locator('.toast.error').boundingBox())!.width).toBeLessThanOrEqual(380)
  await expect(page.locator('.pdf-inline-editor [role="alert"]')).toHaveCount(0)
  await expect(page.getByRole('textbox', { name: 'Replacement' })).toHaveValue('W'.repeat(200))
})

test('Edit text reviews signature risk without a failed request or inline error', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Signed Text Edit', court_profile: 'general-demo', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  const docId = (await uploaded.json())[0].id as string
  const marked = await request.patch(`${backend}/documents/${docId}`, {
    headers, data: {visible_signature_status: 'present'},
  })
  expect(marked.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Signed Text Edit'})).toBeVisible()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name: 'Edit', exact: true}).click()
  await page.getByRole('button', {name: 'Edit text', exact: true}).click()
  await page.getByRole('button', {name: /Edit text: DRAFT PETITION/}).click()
  await page.getByRole('textbox', {name: 'Replacement'}).fill('REVIEWED PETITION')
  await page.route(`**/matters/${matterId}/documents`, async route => {
    const response = await route.fetch()
    const listed = await response.json() as Array<{id:string,signature_status:string}>
    await route.fulfill({response, json: listed.map(item => item.id === docId
      ? {...item, signature_status: 'unsigned'} : item)})
  })
  const prompts: string[] = []
  page.on('dialog', async dialog => { prompts.push(dialog.message()); await dialog.accept() })
  const responses: number[] = []
  const acknowledgments: boolean[] = []
  page.on('response', response => {
    if (!response.url().endsWith('/pages/replace-text-run')) return
    responses.push(response.status())
    acknowledgments.push(response.request().postDataJSON().acknowledge_signature_risk)
  })
  await page.getByRole('button', {name: 'Apply replacement'}).click()
  await expect(page.locator('.text-overlay-panel [role="status"]')).toContainText('Removed and redrew selected text')
  expect(prompts).toHaveLength(1)
  expect(prompts[0]).toContain('01_petition')
  expect(acknowledgments).toEqual([false, true])
  expect(responses).toEqual([200, 200])
  await expect(page.locator('.pdf-inline-editor [role="alert"]')).toHaveCount(0)
})

test('PDFium paragraph trial edits a synthetic working revision from the viewer', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E PDFium Paragraph', court_profile: 'general-demo', max_pages_per_volume: 10,
  } })
  expect(created.ok()).toBeTruthy()
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E PDFium Paragraph'})).toBeVisible()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Edit', exact:true}).click()
  await page.getByRole('button', { name: 'Edit text', exact: true }).click()
  await page.getByRole('button', { name: /Edit text: IN THE COURT/ }).click()
  await page.getByRole('button', {name:'Edit whole paragraph'}).click()
  await expect(page.getByRole('textbox', { name: 'Replacement' })).toHaveValue('IN THE COURT (DEMONSTRATION)')
  await page.getByRole('textbox', { name: 'Replacement' }).fill('IN THE COURT - UPDATED\nDEMONSTRATION ONLY')
  const applied = page.waitForResponse(response => response.url().endsWith('/pages/paragraph-trial'))
  await page.getByRole('button', { name: 'Apply replacement' }).click()
  const appliedResponse = await applied
  expect(appliedResponse.status(), await appliedResponse.text()).toBe(200)
  await expect(page.locator('.text-overlay-panel [role="status"]')).toContainText('Whole paragraph updated')
  await expect.poll(async () => {
    const response = await request.get(`${backend}/matters/${matterId}/search?q=UPDATED`, { headers })
    return (await response.json()).matches.length
  }).toBe(1)
})

test('selectable text can be dragged diagonally and saved to the working copy', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Move Text', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  const doc = (await uploaded.json())[0]
  const runsUrl = `${backend}/documents/${doc.id}/pages/0/text-runs`
  const before = await request.get(runsUrl, { headers })
  const original = (await before.json()).runs.find((run: {text:string}) => run.text.startsWith('IN THE COURT'))
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Move Text'})).toBeVisible()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Edit', exact:true}).click()
  await page.getByRole('button', { name: 'Edit text', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Move text vertically' })).toHaveCount(0)
  const run = page.getByRole('button', { name: /Edit text: IN THE COURT/ })
  await expect(page.locator('.preview-status')).toHaveCount(0)
  const preview = page.locator('.pdf-placement-surface')
  await expect.poll(async () => (await preview.boundingBox())!.y - (await page.locator('.canvas-scroll').boundingBox())!.y).toBeCloseTo(24, 0)
  await run.scrollIntoViewIfNeeded()
  const previewTop = (await preview.boundingBox())!.y
  await page.route('**/documents/*/pdf?revision=*', async route => {
    await new Promise(resolve => setTimeout(resolve, 350))
    await route.continue()
  })
  const bounds = await run.boundingBox()
  expect(bounds).toBeTruthy()
  await page.mouse.move(bounds!.x + bounds!.width / 2, bounds!.y + bounds!.height / 2)
  await page.mouse.down()
  await page.mouse.move(bounds!.x + bounds!.width / 2 + 14, bounds!.y + bounds!.height / 2 + 14, { steps: 4 })
  await page.mouse.up()
  await expect(page.getByRole('button', { name: 'Apply move' })).toBeVisible()
  await page.getByRole('button', { name: 'Apply move' }).click()
  await expect(page.locator('.pdf-edit-snapshot')).toBeVisible()
  expect((await preview.boundingBox())!.y).toBeCloseTo(previewTop, 0)
  await expect(page.locator('.pdf-edit-snapshot')).toHaveCount(0)
  await page.unroute('**/documents/*/pdf?revision=*')
  await expect(page.locator('.text-overlay-panel [role="status"]')).toContainText('Text moved')
  await expect(page.locator('.pdf-inline-editor')).toHaveCount(0)
  const after = await request.get(runsUrl, { headers })
  const moved = (await after.json()).runs.find((item: {text:string}) => item.text === original.text)
  expect(moved.relative_bbox[0]).toBeGreaterThan(original.relative_bbox[0])
  expect(moved.relative_bbox[1]).toBeGreaterThan(original.relative_bbox[1])
})

test('added text remains movable and enters saved PDF output', async ({ page, request }) => {
  await page.setViewportSize({width:1440,height:1000})
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Text Overlay', court_profile: 'general-demo', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  const doc = (await uploaded.json())[0]
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Text Overlay'})).toBeVisible()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Edit', exact:true}).click()
  await page.getByRole('button', { name: 'Add text' }).click()
  await page.locator('.pdf-paper').click({ position: { x: 120, y: 180 } })
  await page.getByRole('textbox', { name: /Add text on page/ }).fill('REVIEW OVERLAY')
  await page.getByRole('button', { name: 'Apply text' }).click()
  const mark=page.locator('.pdf-review-shape.text')
  await expect(mark).toBeVisible()
  const before=(await (await request.get(`${backend}/matters/${matterId}/annotations`,{headers})).json())[0]
  expect(before.output_scope).toBe('pdf')
  await mark.scrollIntoViewIfNeeded()
  const position=await mark.boundingBox()
  expect(position).not.toBeNull()
  await page.mouse.move(position!.x+position!.width/2,position!.y+position!.height/2)
  await page.mouse.down()
  await page.mouse.move(position!.x+position!.width/2+20,position!.y+position!.height/2+20,{steps:5})
  await page.mouse.up()
  await expect.poll(async()=> (await (await request.get(`${backend}/matters/${matterId}/annotations`,{headers})).json())[0].rect_x as number).toBeGreaterThan(before.rect_x)
  await page.reload()
  await expect(mark).toBeVisible()
  const snapshot=await request.get(`${backend}/documents/${doc.id}/pdf?revision=1&editable=true`,{headers})
  expect(snapshot.ok()).toBeTruthy()
  expect(snapshot.headers()['content-type']).toContain('application/pdf')
  const download=page.waitForEvent('download')
  await openDocumentActions(page)
  await page.getByRole('button',{name:'Save working copy'}).click()
  const saved=await download
  expect(saved.suggestedFilename()).toContain('working-r1.pdf')
})

test('redaction mark stays movable and is applied in saved PDF output', async ({ page, request }) => {
  await page.setViewportSize({width: 1440, height: 1000})
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Permanent Redaction', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  const document = (await uploaded.json())[0]
  const marked = await request.patch(`${backend}/documents/${document.id}`, {
    headers, data: {visible_signature_status: 'present'},
  })
  expect(marked.ok()).toBeTruthy()
  const runsResponse = await request.get(`${backend}/documents/${document.id}/pages/0/text-runs`, { headers })
  const run = (await runsResponse.json()).runs.find((item: {text:string}) => item.text.includes('DRAFT PETITION'))
  expect(run).toBeDefined()
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Permanent Redaction'})).toBeVisible()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Edit', exact:true}).click()
  await page.getByRole('button', { name: 'Redact', exact: true }).click()
  await page.getByRole('combobox', { name: 'Zoom' }).selectOption('50')
  const apply = page.getByRole('button', { name: 'Add redaction mark' })
  await expect(page.getByText('Drag over the content to remove.')).toBeVisible()
  await expect(apply).toBeDisabled()
  await page.locator('.pdf-placement-surface').evaluate(surface => {
    const canvas = surface.querySelector('canvas')!
    const original = canvas.getBoundingClientRect.bind(canvas)
    canvas.getBoundingClientRect = () => new DOMRect(0, 0, 0, 0)
    surface.dispatchEvent(new PointerEvent('pointerdown', {
      bubbles: true, button: 0, pointerId: 42, clientX: 100, clientY: 100,
    }))
    canvas.getBoundingClientRect = original
  })
  await expect(page.locator('.pdf-redaction-region')).toHaveCount(0)
  await expect(apply).toBeDisabled()
  await expect(page.locator('.pdf-placement-surface')).not.toHaveClass(/is-stale|is-busy|is-switching/)
  await page.locator('.canvas-scroll').evaluate(element => { element.scrollTop = 0 })
  const canvas = await page.locator('.pdf-paper').boundingBox()
  expect(canvas).not.toBeNull()
  const [x0,y0,x1,y1] = run.relative_bbox as number[]
  await page.mouse.move(canvas!.x + Math.max(0,x0-.01)*canvas!.width,
    canvas!.y + Math.max(0,y0-.01)*canvas!.height)
  await page.mouse.down()
  await page.mouse.move(canvas!.x + Math.min(1,x1+.01)*canvas!.width,
    canvas!.y + Math.min(1,y1+.01)*canvas!.height, {steps:8})
  await page.mouse.up()
  await expect(page.locator('.pdf-redaction-region')).toBeVisible()
  await expect(page.locator('.pdf-redaction-region')).toHaveCSS('background-color', 'rgb(16, 16, 16)')
  await expect(apply).toBeEnabled()
  await expect(page.getByRole('checkbox', { name: 'I reviewed this area and the document-wide cleanup.' })).toHaveCount(0)
  const originalMark = await page.locator('.pdf-redaction-region').boundingBox()
  const resize = await page.getByRole('button', {name:'Resize redaction area from se corner'}).boundingBox()
  expect(originalMark).not.toBeNull()
  expect(resize).not.toBeNull()
  await page.mouse.move(resize!.x + resize!.width / 2, resize!.y + resize!.height / 2)
  await page.mouse.down()
  await page.mouse.move(resize!.x + resize!.width / 2 + 12, resize!.y + resize!.height / 2 + 12, {steps:4})
  await page.mouse.up()
  const expandedMark = await page.locator('.pdf-redaction-region').boundingBox()
  expect(expandedMark!.width).toBeGreaterThan(originalMark!.width)
  expect(expandedMark!.height).toBeGreaterThan(originalMark!.height)
  await apply.click()
  await expect(page.locator('.review-tool-notice')).toContainText('Redaction mark saved.')
  const staged=page.locator('.pdf-review-shape.redaction')
  await expect(staged).toBeVisible()
  const before=(await (await request.get(`${backend}/matters/${matterId}/annotations`,{headers})).json())[0]
  const area=await staged.boundingBox()
  expect(area).not.toBeNull()
  await page.mouse.move(area!.x+area!.width/2,area!.y+area!.height/2)
  await page.mouse.down()
  await page.mouse.move(area!.x+area!.width/2+20,area!.y+area!.height/2+12,{steps:5})
  await page.mouse.up()
  await expect.poll(async()=> (await (await request.get(`${backend}/matters/${matterId}/annotations`,{headers})).json())[0].rect_x as number).toBeGreaterThan(before.rect_x)
  await page.reload()
  await expect(staged).toBeVisible()
  const saved=await request.get(`${backend}/documents/${document.id}/pdf?revision=${document.revision}&editable=true&acknowledge_signature_risk=true`,{headers})
  expect(saved.ok()).toBeTruthy()
  expect(saved.headers()['content-type']).toContain('application/pdf')
  await staged.click()
  await staged.press('Delete')
  await expect(staged).toHaveCount(0)
})

test('redaction applies to a rotated synthetic scan', async ({ page, request }) => {
  page.setDefaultTimeout(10_000)
  await page.setViewportSize({width: 1280, height: 1100})
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Rotated Scan Redaction', court_profile: 'general-demo', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: '04_scanned_annexure.jpg', mimeType: 'image/jpeg',
      buffer: await readFile(resolve(samples, '04_scanned_annexure.jpg')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  const document = (await uploaded.json())[0]
  const rotated = await request.post(`${backend}/documents/${document.id}/pages/rotate`, {
    headers, data: { page_index: 0, degrees: 90 },
  })
  expect(rotated.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Rotated Scan Redaction'})).toBeVisible()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Edit', exact:true}).click()
  await page.getByRole('button', { name: 'Redact', exact: true }).click()
  await page.getByRole('combobox', { name: 'Zoom' }).selectOption('50')
  await expect(page.locator('.pdf-placement-surface')).not.toHaveClass(/is-switching/)
  await page.locator('.canvas-scroll').evaluate(element => { element.scrollTop = 0 })
  const canvas = await page.locator('.pdf-paper').boundingBox()
  expect(canvas).not.toBeNull()
  await page.mouse.move(canvas!.x + canvas!.width * .2, canvas!.y + canvas!.height * .2)
  await page.mouse.down()
  await page.mouse.move(canvas!.x + canvas!.width * .35, canvas!.y + canvas!.height * .35, { steps: 8 })
  await page.mouse.up()
  await page.getByRole('button', { name: 'Add redaction mark' }).click()
  await expect(page.locator('.review-tool-notice')).toContainText('Redaction mark saved.')
  const current = await request.get(`${backend}/matters/${matterId}/documents`, { headers })
  expect((await current.json())[0].revision).toBe(2)
})

test('image overlays, highlights and comments can be placed, edited and removed', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E PDF Annotations', court_profile: 'general-demo', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name:'01_petition.pdf', mimeType:'application/pdf',
      buffer:await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()

  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E PDF Annotations'})).toBeVisible()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Annotate', exact:true}).click()
  await page.getByRole('button', {name:'Highlight', exact:true}).click()
  const paper = page.locator('.pdf-paper')
  const draw = async (x:number, y:number, width:number, height:number) => {
    await paper.scrollIntoViewIfNeeded()
    await expect(page.locator('.pdf-placement-surface')).not.toHaveClass(/is-stale/)
    const bounds = await paper.boundingBox()
    expect(bounds).toBeTruthy()
    await page.mouse.move(bounds!.x + x, bounds!.y + y)
    await page.mouse.down()
    await page.mouse.move(bounds!.x + x + width, bounds!.y + y + height, {steps:5})
    await page.mouse.up()
  }

  await draw(80, 12, 150, 28)
  await page.getByRole('textbox', {name:'Optional highlight note'}).fill('Important passage')
  await expect(page.getByRole('button', {name:'Add mark'})).toBeEnabled()
  await page.getByRole('button', {name:'Add mark'}).click()
  await expect(page.getByText('Highlight saved as a private review annotation.')).toBeVisible()
  await expect(page.locator('.pdf-review-highlight')).toHaveCount(1)

  await page.getByRole('button', {name:'Comment', exact:true}).click()
  await draw(250, 12, 45, 28)
  await page.getByRole('textbox', {name:'Comment text'}).fill('Check the cited proposition')
  await expect(page.getByRole('button', {name:'Add mark'})).toBeEnabled()
  await page.getByRole('button', {name:'Add mark'}).click()
  await expect(page.locator('.pdf-review-comment')).toHaveCount(1)

  await openDocumentActions(page)
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Stamps', exact:true}).click()
  await page.getByRole('button', {name:'Image / stamp', exact:true}).click()
  await page.getByLabel('PNG or JPEG').setInputFiles(resolve(samples, '04_scanned_annexure.jpg'))
  await draw(320, 12, 120, 28)
  await expect(page.getByRole('button', {name:'Add mark'})).toBeEnabled()
  await page.getByRole('button', {name:'Add mark'}).click()
  await expect(page.locator('.pdf-image-overlay')).toHaveCount(1)
  let annotations = await (await request.get(`${backend}/matters/${matterId}/annotations`, {headers})).json()
  expect(annotations.map((item:{kind:string}) => item.kind).sort()).toEqual(['comment','highlight','image'])

  await page.locator('.pdf-review-comment').click()
  await page.getByRole('textbox', {name:'Comment text'}).fill('Reviewed comment text')
  await page.getByRole('button', {name:'Update mark'}).click()
  await expect(page.getByText('Annotation updated.')).toBeVisible()
  annotations = await (await request.get(`${backend}/matters/${matterId}/annotations`, {headers})).json()
  expect(annotations.find((item:{kind:string}) => item.kind === 'comment').body).toBe('Reviewed comment text')

  page.once('dialog', dialog => void dialog.accept())
  await page.locator('.pdf-review-highlight').click()
  await page.getByRole('button', {name:'Remove highlight'}).click()
  await expect(page.locator('.pdf-review-highlight')).toHaveCount(0)
  annotations = await (await request.get(`${backend}/matters/${matterId}/annotations`, {headers})).json()
  expect(annotations).toHaveLength(2)
})

test('reviewed link and output-mark preview persist through the matter workspace', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Links And Marks', court_profile: 'general-demo', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  for (const name of ['01_petition.pdf', '02_impugned_order.pdf']) {
    const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
      multipart: { files: { name, mimeType: 'application/pdf',
        buffer: await readFile(resolve(samples, name)) } },
    })
    expect(uploaded.ok()).toBeTruthy()
  }
  const documents = await (await request.get(`${backend}/matters/${matterId}/documents`, {headers})).json()

  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Links And Marks'})).toBeVisible()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Annotate', exact:true}).click()
  await page.getByRole('button', {name:'Links', exact:true}).click()
  const paper = page.locator('.pdf-paper')
  await expect(paper).toBeVisible()
  await expect(page.locator('.pdf-placement-surface')).not.toHaveClass(/is-stale/)
  // Wait for the paper's actionability after opening a tool changes panel width.
  await paper.click({trial:true,position:{x:90,y:12}})
  const bounds = await paper.boundingBox()
  expect(bounds).toBeTruthy()
  await page.mouse.move(bounds!.x + 90, bounds!.y + 12)
  await page.mouse.down()
  await page.mouse.move(bounds!.x + 230, bounds!.y + 47, {steps:5})
  await page.mouse.up()
  await page.getByRole('textbox', {name:'Readable label'}).fill('Open supporting order')
  await page.getByRole('combobox', {name:'Target document'}).selectOption(documents[1].id)
  await page.getByRole('button', {name:'Save reviewed link'}).click()
  await expect(page.getByText('Reviewed link saved.')).toBeVisible()
  const links = await (await request.get(`${backend}/matters/${matterId}/links`, {headers})).json()
  expect(links).toHaveLength(1)
  expect(links[0].target_document_id).toBe(documents[1].id)
  expect(links[0].resolved).toBeTruthy()

  await openDocumentActions(page)
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Output', exact:true}).click()
  await page.getByRole('button', {name:'Output marks', exact:true}).click()
  await page.getByRole('checkbox', {name:/Watermark/}).check()
  await page.getByRole('textbox', {name:'Watermark text'}).fill('REVIEW COPY')
  await page.getByRole('checkbox', {name:/Build timestamp/}).check()
  await page.getByRole('textbox', {name:'Timezone'}).fill('Asia/Kolkata')
  await expect(page.locator('.pdf-output-mark-preview.watermark')).toContainText('REVIEW COPY')
  await expect(page.locator('.pdf-output-mark-preview.timestamp')).toContainText('IST')
  await page.getByRole('button', {name:'Save output marks'}).click()
  await expect(page.getByText('Output marks saved.')).toBeVisible()
  const marks = await (await request.get(`${backend}/matters/${matterId}/output-marks`, {headers})).json()
  expect(marks.watermark_text).toBe('REVIEW COPY')
  expect(marks.timestamp_timezone).toBe('Asia/Kolkata')
})

test('appearance persists and keyboard search respects text editing', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Appearance Matter', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  expect(created.ok()).toBeTruthy()
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Appearance Matter'})).toBeVisible()
  // Check readable rendered controls rather than pinning the previous palette.
  const contrast = async (selector: string) => page.locator(selector).first().evaluate(el => {
    const luminance = (color: string) => {
      const channels = color.match(/[\d.]+/g)!.slice(0, 3).map(Number).map(value => {
        const c = value / 255
        return c <= .04045 ? c / 12.92 : ((c + .055) / 1.055) ** 2.4
      })
      return channels[0]! * .2126 + channels[1]! * .7152 + channels[2]! * .0722
    }
    let background = el
    while (getComputedStyle(background).backgroundColor === 'rgba(0, 0, 0, 0)' && background.parentElement) background = background.parentElement
    const fg = luminance(getComputedStyle(el).color)
    const bg = luminance(getComputedStyle(background).backgroundColor)
    return (Math.max(fg, bg) + .05) / (Math.min(fg, bg) + .05)
  })
  await openWorkspaceNavigation(page)
  await page.getByRole('combobox', { name: 'Theme' }).selectOption('dark')
  await openWorkspaceNavigation(page)
  await page.getByRole('combobox', { name: 'Interface size' }).selectOption('125')
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark')
  await expect(page.locator('html')).toHaveAttribute('data-effective-theme', 'dark')
  await expect(page.locator('html')).toHaveCSS('zoom', '1.25')
  await expect.poll(() => contrast('.panel')).toBeGreaterThan(4.5)
  await expect.poll(() => contrast('.upload-zone')).toBeGreaterThan(4.5)
  await expect.poll(() => contrast('.canvas-scroll')).toBeGreaterThan(4.5)
  await expect.poll(() => contrast('.page-editbar')).toBeGreaterThan(4.5)
  await page.getByRole('button',{name:'Close workspace navigation',exact:true}).click()
  await expect(page.locator('.application-bar')).toBeVisible()
  await expect(page.locator('.matter-rail')).toBeVisible()
  await expect(page.locator('.workspace-drawer')).toBeHidden()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab',{name:'Edit',exact:true}).click()
  const pageTool=page.getByRole('button',{name:'Add text',exact:true})
  await expect(pageTool).toBeEnabled()
  const beforeHover=await pageTool.evaluate(el=>getComputedStyle(el).backgroundColor)
  await pageTool.hover();await expect(pageTool).not.toHaveCSS('background-color',beforeHover)
  await openDocumentActions(page);await page.getByRole('button',{name:'Document details',exact:true}).click()
  const readerBox=await page.locator('.canvas-scroll').boundingBox(),detailsBox=await page.locator('.inspector-panel').boundingBox()
  expect(readerBox!.width).toBeGreaterThan(300);expect(readerBox!.height).toBeGreaterThan(180)
  expect(readerBox!.x+readerBox!.width).toBeLessThanOrEqual(detailsBox!.x+1)
  await expect.poll(()=>contrast('.inspector-panel .field input')).toBeGreaterThan(4.5)
  await page.getByRole('button',{name:'Close document details'}).click()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab',{name:'Pages',exact:true}).click();await page.getByRole('button',{name:'Page tools',exact:true}).click()
  const tools=page.getByRole('dialog',{name:'Page tools'});await expect(tools).toBeVisible()
  await expect(tools.getByRole('combobox',{name:'Source document'})).toHaveCSS('color-scheme','dark')
  await page.keyboard.press('Escape');await expect(tools).toBeHidden()
  await openWorkspaceNavigation(page);await page.locator('.sidebar').getByRole('button',{name:'Court rules'}).click()
  await expect.poll(()=>contrast('.guide-card')).toBeGreaterThan(4.5)
  await expect(page.locator('.party-form select').first()).toHaveCSS('color-scheme','dark')
  await page.getByRole('button',{name:'Documents',exact:true}).click();await page.reload()
  await openWorkspaceNavigation(page)
  await expect(page.getByRole('combobox',{name:'Theme'})).toHaveValue('dark')
  await expect(page.getByRole('combobox',{name:'Interface size'})).toHaveValue('125')
  await page.getByRole('button',{name:'Close workspace navigation',exact:true}).click()
  await expect(page.locator('.pdf-paper')).toBeVisible()
  await page.getByRole('link',{name:'Skip to main content'}).focus();await page.keyboard.press('Enter')
  await expect(page.locator('#main-content')).toBeFocused()
  await page.keyboard.press('/')
  await expect(page.getByRole('textbox',{name:'Search documents'})).toBeFocused()
  await page.keyboard.type('sample');await page.keyboard.press('Control+z')
  await expect(page.getByRole('textbox',{name:'Search documents'})).toBeFocused()
  await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click();await page.keyboard.press('/')
  await expect(page.getByRole('textbox',{name:'Search documents'})).toBeFocused()
  await page.setViewportSize({width:820,height:700})
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true)
  await openWorkspaceNavigation(page);await expect(page.getByRole('combobox',{name:'Theme'})).toBeVisible()
  await page.getByRole('combobox',{name:'Theme'}).focus();await page.keyboard.press('Escape')
  await expect(page.locator('.workspace-drawer')).toBeHidden()
  await openWorkspaceNavigation(page);await page.emulateMedia({colorScheme:'dark'})
  await page.getByRole('combobox',{name:'Theme'}).selectOption('system')
  await expect(page.locator('html')).toHaveAttribute('data-effective-theme','dark')
  await page.emulateMedia({colorScheme:'light'});await expect(page.locator('html')).toHaveAttribute('data-effective-theme','light')
  await expect(page.getByRole('combobox',{name:'Theme'})).toHaveCSS('color-scheme','light')

})

test('working-copy page tools edit and save synthetic pages', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Page Tools', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const id = (await created.json()).id as string
  for (const name of ['01_petition.pdf', '03_affidavit.pdf']) {
    const uploaded = await request.post(`${backend}/matters/${id}/documents?included=true`, { headers,
      multipart: { files: { name, mimeType: 'application/pdf', buffer: await readFile(resolve(samples, name)) } },
    })
    expect(uploaded.ok()).toBeTruthy()
  }
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Page Tools'})).toBeVisible()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Pages', exact:true}).click()
  await page.getByRole('button', { name: 'Page tools', exact: true }).click()
  const tools = page.getByRole('dialog', { name: 'Page tools' })
  await expect(tools.getByRole('button', {name:'Reduce file size',exact:true})).toBeInViewport()
  await expect(tools.getByRole('button', {name:'Add selected pages',exact:true})).toBeDisabled()
  await expect(tools).toContainText('Choose the document to copy pages from.')
  await tools.getByRole('combobox', { name: 'Source document' }).selectOption({ index: 2 })
  await tools.getByRole('textbox', { name: 'Source pages' }).fill('1')
  await tools.getByRole('textbox', { name: 'Source pages' }).fill('999')
  await expect(tools.getByRole('button', {name:'Add selected pages',exact:true})).toBeDisabled()
  await tools.getByRole('textbox', { name: 'Source pages' }).fill('1')
  await tools.getByRole('spinbutton', { name: 'Insert before page' }).fill('2')
  await page.screenshot({path:'test-results/page-tools-add.png',animations:'disabled'})
  await tools.getByRole('button', { name: 'Add selected pages' }).click()
  await expect(page.locator('.page-counter')).toContainText('/ 4')
  await tools.getByRole('combobox', { name: 'Where to add pages' }).selectOption('after')
  await tools.getByRole('spinbutton', { name: 'Insert after page' }).fill('1')
  await tools.getByRole('button', { name: 'Add selected pages' }).click()
  await expect(page.locator('.page-counter')).toContainText('/ 5')
  await tools.getByRole('button', { name: 'Replace pages', exact: true }).click()
  await tools.getByRole('textbox', { name: 'Target pages to replace' }).fill('1,2')
  await expect(tools.getByRole('button', {name:'Replace selected pages',exact:true})).toBeDisabled()
  await tools.getByRole('textbox', { name: 'Target pages to replace' }).fill('1')
  await tools.getByRole('button', { name: 'Replace selected pages' }).click()
  await expect(tools.getByRole('button', { name: 'Replace selected pages' })).toBeEnabled()
  await tools.getByRole('button', { name: 'Combine PDFs', exact: true }).click()
  await tools.getByRole('button', { name: 'Combine documents', exact: true }).click()
  await expect(page.locator('.page-counter')).toContainText('/ 7')
  await tools.getByRole('button', { name: 'Crop margins', exact: true }).click()
  await tools.getByRole('textbox', { name: 'Pages to crop' }).fill('1')
  await tools.getByRole('spinbutton', { name: 'left %' }).fill('10')
  await tools.getByRole('spinbutton', { name: 'right %' }).fill('5')
  await expect(tools.getByRole('button', {name:'Crop selected pages',exact:true})).toBeDisabled()
  await tools.getByRole('spinbutton', { name: 'right %' }).fill('90')
  await page.screenshot({path:'test-results/page-tools-crop.png',animations:'disabled'})
  await tools.getByRole('button', { name: 'Crop selected pages' }).click()
  await expect(tools.getByRole('button', { name: 'Crop selected pages' })).toBeEnabled()
  await expect.poll(async () => {
    const documents = await request.get(`${backend}/matters/${id}/documents`, { headers })
    return (await documents.json())[0].revision
  }).toBe(6)
  await tools.getByRole('button', { name: 'Save selected pages', exact: true }).click()
  await tools.getByRole('textbox', { name: 'Pages to extract' }).fill('1,3-4')
  const extractedDownload = page.waitForEvent('download')
  await tools.getByRole('button', { name: 'Save extracted PDF' }).click()
  const extracted = await extractedDownload
  expect(extracted.suggestedFilename()).toBe('extracted-pages.pdf')
  expect((await readFile(await extracted.path())).subarray(0, 4).toString()).toBe('%PDF')
  await tools.getByRole('button', { name: 'Split into PDFs', exact: true }).click()
  await tools.getByRole('textbox', { name: 'Split after pages' }).fill('3,2')
  await expect(tools.getByRole('button', {name:'Save split ZIP',exact:true})).toBeDisabled()
  await tools.getByRole('textbox', { name: 'Split after pages' }).fill('2')
  const splitDownload = page.waitForEvent('download')
  await tools.getByRole('button', { name: 'Save split ZIP' }).click()
  const split = await splitDownload
  expect(split.suggestedFilename()).toBe('split-pages.zip')
  expect((await readFile(await split.path())).subarray(0, 2).toString()).toBe('PK')
})

test('preview owns document actions while details stay collapsed until toggled', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Preview Actions', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const id = (await created.json()).id as string
  for (const filename of ['01_petition.pdf', '02_impugned_order.pdf']) {
    const uploaded = await request.post(`${backend}/matters/${id}/documents?included=true`, { headers,
      multipart: { files: { name: filename, mimeType: 'application/pdf',
        buffer: await readFile(resolve(samples, filename)) } },
    })
    expect(uploaded.ok()).toBeTruthy()
  }
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Preview Actions'})).toBeVisible()
  await expect.poll(() => page.locator('.pdf-paper').evaluate((node: HTMLCanvasElement) => node.width)).toBeGreaterThan(0)
  await expect(page.getByText('Loading PDF…')).toBeHidden()
  await expect(page.locator('.inspector-panel')).toHaveCount(0)
  await openDocumentActions(page)
  await expect(page.getByRole('button', { name: 'Document details', exact: true })).toHaveAttribute('aria-expanded', 'false')
  await openDocumentActions(page)
  await expect(page.getByRole('button', { name: 'Document details', exact: true })).toBeInViewport()
  await openDocumentActions(page)
  await page.getByRole('button', { name: 'Document details', exact: true }).click()
  await expect.poll(() => page.locator('.preview-panel').evaluate(panel => {
    const canvas = panel.querySelector('canvas.pdf-paper')!.getBoundingClientRect()
    const scroll = panel.querySelector('.canvas-scroll')!.getBoundingClientRect()
    return canvas.width <= scroll.width - 20
  })).toBeTruthy()
  await expect(page.locator('.inspector-panel')).toBeVisible()
  await page.getByRole('tab',{name:'OCR',exact:true}).click()
  await expect(page.getByRole('button', { name: 'Run English OCR' })).toBeInViewport()
  await openDocumentActions(page)
  await expect(page.getByRole('button', { name: 'Document details', exact: true })).toHaveAttribute('aria-expanded', 'true')
  await closeDocumentActions(page)
  await page.getByRole('button', { name: 'Close document details' }).click()
  await expect(page.locator('.inspector-panel')).toHaveCount(0)
  await openDocumentBrowser(page)
  await expect(page.getByRole('button', { name: 'Move document down' })).toHaveCount(0)
  await page.getByRole('button', { name: /Reorder document 01_petition/ }).focus()
  await page.keyboard.press('ArrowDown')
  await expect.poll(async () => {
    const response = await request.get(`${backend}/matters/${id}/documents`, { headers })
    return (await response.json() as {original_name:string}[]).map(doc => doc.original_name)
  }).toEqual(['02_impugned_order.pdf', '01_petition.pdf'])
  await page.getByRole('button', { name: 'Index & bookmarks' }).click()
  await page.getByRole('region', { name: 'Index document order' })
    .getByRole('button', { name: /Reorder document 02_impugned_order/ }).focus()
  await page.keyboard.press('ArrowDown')
  await expect.poll(async () => {
    const response = await request.get(`${backend}/matters/${id}/documents`, { headers })
    return (await response.json() as {original_name:string}[]).map(doc => doc.original_name)
  }).toEqual(['01_petition.pdf', '02_impugned_order.pdf'])
})

test('visible rotation controls turn the selected PDF page in either direction', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Page Rotation', court_profile: 'general-demo', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  const sourceHash = (await uploaded.json())[0].source_sha256 as string
  const pdfRequests: string[] = []
  page.on('request', request => {
    if (request.url().includes('/documents/') && request.url().includes('/pdf?revision=')) pdfRequests.push(request.url())
  })
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Page Rotation'})).toBeVisible()
  await page.getByRole('button', { name: 'Next page' }).click()
  await expect(page.locator('.page-counter input')).toHaveValue('2')
  await expect(page.locator('.page-counter')).toContainText('/ 3')
  const canvas = page.locator('.pdf-paper')
  const ratio = () => canvas.evaluate((node: HTMLCanvasElement) => node.width / node.height)
  await expect.poll(()=>canvas.evaluate((node:HTMLCanvasElement)=>node.width)).toBeGreaterThan(0)
  const originalRatio = await ratio()
  await page.route('**/pages/rotate', async route => {
    await new Promise(resolve => setTimeout(resolve, 350))
    await route.continue()
  })
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Pages', exact:true}).click()
  await page.getByRole('button', { name: 'Rotate left' }).click()
  const surface = page.locator('.pdf-placement-surface')
  await expect(surface).toHaveClass(/is-rotating/)
  await expect.poll(() => surface.evaluate(element => Number(getComputedStyle(element).opacity))).toBeLessThan(0.1)
  await expect(page.locator('.page-counter input')).toHaveValue('2')
  await expect(page.locator('.page-counter')).toContainText('/ 3')
  await expect.poll(ratio).toBeCloseTo(1 / originalRatio, 1)
  await expect(surface).not.toHaveClass(/is-rotating/)
  await page.unroute('**/pages/rotate')
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Pages', exact:true}).click()
  await page.getByRole('button', { name: 'Rotate right' }).click()
  await expect(page.locator('.page-counter input')).toHaveValue('2')
  await expect(page.locator('.page-counter')).toContainText('/ 3')
  await expect.poll(ratio).toBeCloseTo(originalRatio, 1)
  expect(pdfRequests.map(url => new URL(url).searchParams.get('revision'))).toEqual(['1', '2', '3'])
  const documents = await request.get(`${backend}/matters/${matterId}/documents`, { headers })
  expect((await documents.json())[0].source_sha256).toBe(sourceHash)
})

test('unavailable engine shows a bounded error and reconnects', async ({ page }) => {
  await page.route('**/health', route => route.abort())
  await page.goto('/')
  await expect(page.locator('.application-status').getByText('Engine disconnected',{exact:true})).toBeVisible({ timeout: 20_000 })
  await expect(page.locator('.engine-notice').getByRole('status')).toContainText('Local engine unavailable')
  await page.unroute('**/health')
  await page.getByRole('button', { name: 'Retry connection' }).click()
  await expect(page.locator('.application-status').getByText('Processing locally',{exact:true})).toBeVisible()
})

test('page text search opens a result and thumbnails navigate', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Search Matter', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const id = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${id}/documents?included=true`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Search Matter'})).toBeVisible()
  await openDocumentBrowser(page)
  await page.getByRole('textbox', {name:'Find text on pages'}).fill('page 3/3')
  await expect(page.getByRole('button', {name:/01_petition · page 3/})).toBeVisible()
  await openDocumentBrowser(page)
  await page.getByRole('button', {name:/01_petition · page 3/}).click()
  await expect(page.locator('.page-counter input')).toHaveValue('3')
  await page.getByRole('button', {name:'Go to page 1'}).click()
  await expect(page.locator('.page-counter input')).toHaveValue('1')
  await expect(page.locator('.page-counter')).toContainText('/ 3')
  await expect(page.locator('.thumbnail-button canvas').first()).toHaveAttribute('data-rendered', 'true')
  await openDocumentBrowser(page)
  await page.getByRole('button', {name:/01_petition · page 3/}).click()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Pages', exact:true}).click()
  page.once('dialog', dialog => void dialog.accept())
  await page.getByRole('button', {name:/Delete page \d+/}).click()
  await expect(page.locator('.page-counter')).toContainText('/ 2')
  await openDocumentBrowser(page)
  await expect(page.getByText('0 matching page(s)')).toBeVisible()
})

test('PDF editor groups tools and finds text in the current working PDF', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E PDF Editor Navigation', court_profile: 'general-demo', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E PDF Editor Navigation'})).toBeVisible()
  await expect(page.getByRole('tab', {name:'Read'})).toHaveAttribute('aria-selected', 'true')
  await expect(page.getByRole('button', {name:'Go to page 1'})).toBeVisible()
  await page.getByRole('button', {name:'Search',exact:true}).click()
  await page.getByRole('searchbox', {name:'Find text in this PDF'}).fill('page 3/3')
  await expect(page.locator('.pdf-findbar').getByText('1 match', {exact:false})).toBeVisible()
  await page.getByRole('button', {name:/Page 3 ·/}).click()
  await expect(page.locator('.page-counter input')).toHaveValue('3')
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Pages'}).click()
  await expect(page.getByRole('button', {name:'Rotate right'})).toBeVisible()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Annotate'}).click()
  await expect(page.getByRole('button', {name:'Bookmark page'})).toBeVisible()
  await expect(page.getByRole('button', {name:'Rotate right'})).toHaveCount(0)
})

test('synthetic browser journey, retained export and download bytes', async ({ page, request }) => {
  page.setDefaultTimeout(10_000)
  await page.goto('/')
  await expect(page.locator('.application-status').getByText('Processing locally',{exact:true})).toBeVisible()
  await openWorkspaceNavigation(page)
  await page.locator('.sidebar').getByRole('button', { name: 'New matter' }).click()
  await page.getByRole('dialog', { name: 'Create matter' }).getByPlaceholder('Example: ABC Pvt Ltd v. XYZ').fill('E2E Synthetic Matter')
  await page.getByRole('dialog', { name: 'Create matter' }).getByRole('button', { name: 'Create matter' }).click()
  await expect(page.locator('.application-matter').filter({hasText:'E2E Synthetic Matter'})).toBeVisible()

  await page.locator('input[accept=".pdf,.jpg,.jpeg,.png"]').setInputFiles([
    resolve(samples, '01_petition.pdf'),
    resolve(samples, '02_impugned_order.pdf'),
    resolve(samples, '03_affidavit.pdf'),
    resolve(samples, '04_scanned_annexure.jpg'),
  ])
  await expect(page.locator('.doc-item')).toHaveCount(4)
  await includeAllDocuments(page)
  await expect(page.locator('.page-counter input')).toHaveValue('1')
  await expect(page.locator('.page-counter')).toContainText('/ 3')
  const canvas = page.locator('.pdf-paper')
  await expect.poll(() => canvas.evaluate((node: HTMLCanvasElement) => node.width)).toBeGreaterThan(0)
  const before = await canvas.evaluate((node: HTMLCanvasElement) => `${node.width}x${node.height}`)
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Pages', exact:true}).click()
  await page.getByRole('button', { name: 'Rotate right' }).click()
  await expect.poll(() => canvas.evaluate((node: HTMLCanvasElement) => `${node.width}x${node.height}`)).not.toBe(before)
  await page.locator('button[title="Move page right"]').click()
  await expect(page.locator('.page-counter input')).toHaveValue('2')
  await expect(page.locator('.page-counter')).toContainText('/ 3')
  page.once('dialog', dialog => void dialog.accept())
  await page.locator('button[title^="Delete page"]').click()
  await expect(page.locator('.page-counter input')).toHaveValue('2')
  await expect(page.locator('.page-counter')).toContainText('/ 2')

  await openDocumentBrowser(page)
  await page.locator('.doc-item', { hasText: '04_scanned_annexure' }).click()
  await expect(page.locator('.page-counter input')).toHaveValue('1')
  await expect(page.locator('.page-counter')).toContainText('/ 1')
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab',{name:'OCR',exact:true}).click()
  await page.getByRole('button', { name: 'Run English OCR' }).click()
  await openDocumentActions(page)
  await page.getByRole('button', { name: 'Document details' }).click()
  await expect(page.locator('.inspector-heading')).toContainText('processed', { timeout: 90_000 })
  await expect(page.locator('.ocr-provenance')).toContainText('Revision 2: eng')
  await expect(page.locator('.ocr-provenance')).toContainText('eng model SHA-256:')
  await page.getByRole('textbox', { name: 'Display title' }).fill('Synthetic OCR Annexure')
  await page.getByRole('button', { name: 'Save document details' }).click()
  await expect(page.getByText('Document details updated.')).toBeVisible()
  await page.reload()
  await expect(page.locator('.doc-item')).toHaveCount(4)
  await openDocumentBrowser(page)
  await page.locator('.doc-item', { hasText: 'Synthetic OCR Annexure' }).click()
  await openDocumentActions(page)
  await page.getByRole('button', { name: 'Document details' }).click()
  await expect(page.locator('.inspector-heading')).toContainText('processed')
  await expect(page.locator('.ocr-provenance')).toContainText('Revision 2: eng')

  await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click()
  await openBuildTool(page)
  await page.getByRole('spinbutton', { name: /Maximum pages per volume/ }).fill('6')
  await submitBundleBuild(page)
  await expect(page.getByRole('heading',{name:'Bundle ready for review'})).toBeVisible({ timeout: 90_000 })

  const matterResponse = await request.get(`${backend}/matters`, { headers })
  expect(matterResponse.ok()).toBeTruthy()
  const matterId = (await matterResponse.json()).find((item:{name:string}) => item.name === 'E2E Synthetic Matter').id as string
  const exportResponse = await request.get(`${backend}/matters/${matterId}/exports`, { headers })
  expect(exportResponse.ok()).toBeTruthy()
  const exportId = (await exportResponse.json())[0].id as string
  const stored = await request.get(`${backend}/matters/${matterId}/exports/${exportId}/filing-package.zip`, { headers })
  expect(stored.ok()).toBeTruthy()
  const downloadPromise = page.waitForEvent('download')
  await openOutputTools(page)
  await page.getByRole('button', { name: 'Download filing-package.zip' }).click()
  const downloaded = await downloadPromise
  const bytes = await readFile(await downloaded.path())
  expect(bytes.subarray(0, 2).toString()).toBe('PK')
  expect(createHash('sha256').update(bytes).digest('hex'))
    .toBe(createHash('sha256').update(await stored.body()).digest('hex'))

  await openBuildTool(page)
  await page.locator('.volume-workspace').getByRole('button',{name:'Back to build',exact:true}).click()
  await request.patch(`${backend}/matters/${matterId}`,{headers,data:{max_pages_per_volume:2}})
  await page.reload()
  await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click()
  await submitBundleBuild(page)
  await expect(page.locator('.build-workspace')).toContainText('Earlier build', { timeout: 90_000 })
  await expect(page.locator('.build-error-detail')).toContainText('exceeds', { timeout: 90_000 })
  const retained = await request.get(`${backend}/matters/${matterId}/exports`, { headers })
  expect((await retained.json()).map((item: {id: string}) => item.id)).toEqual([exportId])
})

test('Memo of Parties autosaves per matter and reports a failed save', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Memo Matter', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  expect(created.ok()).toBeTruthy()
  const id = (await created.json()).id as string
  await page.goto('/')
  await expect(page.locator('.application-status').getByText('Processing locally',{exact:true})).toBeVisible()
  await openWorkspaceNavigation(page)
  await page.getByRole('button', { name: 'Court rules' }).click()
  const helper = page.locator('.memo-helper')
  await expect(helper.getByRole('status')).toContainText('Saved to this matter')
  await helper.getByRole('textbox', { name: 'Party name' }).fill('Synthetic Saved Party')
  await expect(helper.getByRole('status')).toContainText('Saved to this matter')
  const stored = await request.get(`${backend}/matters/${id}/memo-parties`, { headers })
  expect((await stored.json()).parties[0].name).toBe('Synthetic Saved Party')
  await page.reload()
  await openWorkspaceNavigation(page)
  await page.getByRole('button', { name: 'Court rules' }).click()
  await expect(page.locator('.memo-helper').getByRole('textbox', { name: 'Party name' }))
    .toHaveValue('Synthetic Saved Party')

  await page.route(`**/matters/${id}/memo-parties`, route =>
    route.request().method() === 'PUT' ? route.abort() : route.continue())
  await page.locator('.memo-helper').getByRole('textbox', { name: 'Party name' })
    .fill('Synthetic Retried Party')
  await expect(page.locator('.memo-helper').getByRole('status')).toContainText('Save failed', { timeout: 10_000 })
  await page.locator('.memo-helper').getByRole('textbox', { name: 'Party name' }).fill('Synthetic Retried Party after disconnect')
  await expect(page.locator('.memo-helper').getByRole('button', { name: 'Retry save' })).toBeDisabled()
  await page.unroute(`**/matters/${id}/memo-parties`)
  await page.getByRole('button', { name: 'Retry connection', exact: true }).click()
  await expect(page.locator('.engine-notice')).toHaveCount(0)
  await expect(page.locator('.memo-helper').getByRole('textbox', { name: 'Party name' })).toHaveValue('Synthetic Retried Party after disconnect')
  await page.locator('.memo-helper').getByRole('button', { name: 'Retry save' }).click()
  await expect(page.locator('.memo-helper').getByRole('status')).toContainText('Saved to this matter')
  const retried = await request.get(`${backend}/matters/${id}/memo-parties`, { headers })
  expect((await retried.json()).parties[0].name).toBe('Synthetic Retried Party after disconnect')
})

test('Memo of Parties Undo flushes a pending edit and reloads saved rows', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Memo History', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const id = (await created.json()).id as string
  await page.goto('/')
  await openWorkspaceNavigation(page)
  await page.getByRole('button', { name: 'Court rules' }).click()
  const helper = page.locator('.memo-helper')
  const name = helper.getByRole('textbox', { name: 'Party name' })
  await name.fill('Synthetic First')
  await expect(helper.getByRole('status')).toContainText('Saved to this matter')
  await expect(helper.getByRole('button', { name: 'Undo matter edit' })).toBeEnabled()
  await name.fill('Synthetic Second')
  await helper.getByRole('button', { name: 'Undo matter edit' }).click()
  await expect(helper.getByRole('textbox', { name: 'Party name' })).toHaveValue('Synthetic First')
  const stored = await request.get(`${backend}/matters/${id}/memo-parties`, { headers })
  expect((await stored.json()).parties[0].name).toBe('Synthetic First')
})

test('unsaved details are guarded and a failed settings save stops a build', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Draft Guard', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const id = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${id}/documents?included=true`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Draft Guard'})).toBeVisible()
  await openDocumentActions(page)
  await page.getByRole('button', { name: 'Document details' }).click()
  await page.getByRole('textbox', { name: 'Display title' }).fill('Unsaved synthetic title')
  await expect(page.getByText('Unsaved document changes')).toBeVisible()
  await page.getByRole('button', { name: 'Close document details' }).click()
  await expect(page.getByText('Save document details to use PDF tools.')).toBeVisible()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Pages', exact:true}).click()
  await expect(page.getByRole('button', { name: 'Rotate left' })).toBeDisabled()
  await expect(page.getByRole('button', { name: 'Rotate right' })).toBeDisabled()
  await page.getByRole('button', { name: 'Open details' }).click()
  await expect(page.getByRole('textbox', { name: 'Display title' })).toHaveValue('Unsaved synthetic title')
  page.once('dialog', dialog => void dialog.dismiss())
  await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click()
  await expect(page.getByText('Unsaved document changes')).toBeVisible()
  page.once('dialog', dialog => void dialog.accept())
  await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click()
  await expect(page.getByText('Unsaved document changes')).toBeHidden()
  await openBuildTool(page)
  await page.getByRole('spinbutton', { name: /Maximum pages per volume/ }).fill('6')
  await expect(page.getByText('Unapplied choices',{exact:true})).toBeVisible()
  await page.route(`**/matters/${id}/bundle-settings`, route =>
    route.request().method() === 'PUT' ? route.abort() : route.continue())
  await page.getByRole('button',{name:'Preview changes',exact:true}).click()
  await page.getByRole('button',{name:'Apply & rebuild',exact:true}).click()
  await expect(page.locator('.volume-workspace').getByRole('alert')).toBeVisible()
  await expect(page.locator('.generated-files')).toHaveCount(0)
  const jobs = await request.get(`${backend}/matters/${id}/jobs`, { headers })
  expect(await jobs.json()).toEqual([])
})

test('portable matter backup restores a separate local copy', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Portable Matter', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  expect(uploaded.ok()).toBeTruthy()
  const docId = (await uploaded.json())[0].id as string
  expect((await request.post(`${backend}/documents/${docId}/pages/rotate`, { headers,
    data: { page_index: 0, degrees: 90 } })).ok()).toBeTruthy()
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Portable Matter'})).toBeVisible()
  const pendingDownload = page.waitForEvent('download')
  await page.getByRole('button',{name:'Overview',exact:true}).click()
  await page.locator('.matter-more summary').click()
  await page.getByRole('button', { name: 'Back up matter' }).click()
  const download = await pendingDownload
  const packagePath = await download.path()
  expect(packagePath).toBeTruthy()
  await page.locator('input[accept=".cbmatter"]').setInputFiles(packagePath!)
  await expect(page.getByText('Matter restored with originals and working revisions.')).toBeVisible()
  const matters = await request.get(`${backend}/matters`, { headers })
  const copies = (await matters.json()).filter((item: {name: string}) => item.name === 'E2E Portable Matter')
  expect(copies).toHaveLength(2)
  const restoredId = copies.find((item: {id: string}) => item.id !== matterId).id as string
  const documents = await request.get(`${backend}/matters/${restoredId}/documents`, { headers })
  expect((await documents.json())[0].revision).toBe(2)
})

test('archived document and matter remain recoverable', async ({ page, request }) => {
  for (let index = 0; index < 30; index++) {
    const extra = await request.post(`${backend}/matters`, { headers, data: {
      name: `Archive sidebar fixture ${index}`, court_profile: 'general-demo',
      max_pages_per_volume: 10,
    } })
    expect(extra.ok()).toBeTruthy()
  }
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Archive Matter', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const id = (await created.json()).id as string
  await request.post(`${backend}/matters/${id}/documents?included=true`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Archive Matter'})).toBeVisible()
  page.once('dialog', dialog => void dialog.accept())
  await openDocumentActions(page)
  await page.getByRole('button', { name: 'Archive document' }).click()
  await expect(page.locator('.doc-item')).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Restore', exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Restore', exact: true }).click()
  await expect(page.locator('.doc-item')).toHaveCount(1)
  await expect(page.getByText('Archived document restored.',{exact:true})).toBeVisible()
  page.once('dialog', dialog => void dialog.accept())
  await page.getByRole('button',{name:'Overview',exact:true}).click()
  await page.locator('.matter-more summary').click()
  await page.getByRole('button', { name: 'Archive matter', exact: true }).click()
  await openWorkspaceNavigation(page)
  await expect(page.locator('button[title="Restore E2E Archive Matter"]')).toBeVisible()
  await page.locator('button[title="Restore E2E Archive Matter"]').click()
  await expect(page.locator('.application-matter').filter({hasText:'E2E Archive Matter'})).toBeVisible()
  await expect(page.locator('.doc-item')).toHaveCount(1)
})

test('undo and redo metadata persist across browser reload', async ({ page, request }) => {
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E History Matter', court_profile: 'delhi-hc-illustrative',
    case_type: 'Synthetic demonstration', max_pages_per_volume: 10,
  } })
  const id = (await created.json()).id as string
  await request.post(`${backend}/matters/${id}/documents?included=true`, { headers,
    multipart: { files: { name: '01_petition.pdf', mimeType: 'application/pdf',
      buffer: await readFile(resolve(samples, '01_petition.pdf')) } },
  })
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E History Matter'})).toBeVisible()
  await openDocumentActions(page)
  await page.getByRole('button', { name: 'Document details' }).click()
  await page.getByRole('textbox', { name: 'Display title' }).fill('Changed title')
  await page.getByRole('button', { name: 'Save document details' }).click()
  await expect(page.locator('.doc-item')).toContainText('Changed title')
  await page.getByRole('button', { name: /^Undo(?: last matter change)?$/, exact: true }).first().click()
  await expect(page.locator('.doc-item')).toContainText('01_petition')
  await page.reload()
  await expect(page.getByRole('button', { name: /^Redo(?: last matter change)?$/, exact: true })).toBeEnabled()
  await page.getByRole('button', { name: /^Redo(?: last matter change)?$/, exact: true }).click()
  await expect(page.locator('.doc-item')).toContainText('Changed title')
})
