import { enterLatestMatter } from './navigation'
import { closeWorkspaceNavigation } from './navigation'
import { expect, test } from '@playwright/test'
import { execFileSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { resolve } from 'node:path'

const backend = `http://127.0.0.1:${process.env.MATTERDRAFT_E2E_BACKEND_PORT || 8768}`
const headers = { Authorization: 'Bearer e2e-isolated-token' }
const backendRoot = resolve(import.meta.dirname, '../../backend')
const python = process.env.MATTERDRAFT_TEST_PYTHON || [
  resolve(backendRoot, '.venv312/bin/python'), resolve(backendRoot, '.venv/bin/python'),
].find(existsSync) || 'python3'

function syntheticPdf(pages = 30) {
  return execFileSync(python, ['-c', `import pymupdf,sys
with pymupdf.open() as pdf:
 for index in range(${pages}):
  page=pdf.new_page(width=420,height=595)
  page.insert_text((36,70),f'Organizer synthetic page {index+1}',fontsize=16)
 sys.stdout.buffer.write(pdf.tobytes())
`])
}

test('page organizer bounds previews, preserves explicit selections, and applies one undoable action', async ({ page, request }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Page Organizer', court_profile: 'general-demo', max_pages_per_volume: 100,
  } })
  expect(created.ok()).toBeTruthy()
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: 'organizer-synthetic.pdf', mimeType: 'application/pdf', buffer: syntheticPdf() } },
  })
  expect(uploaded.ok()).toBeTruthy()
  const document = (await uploaded.json())[0]
  async function history() {
    return (await (await request.get(`${backend}/matters/${matterId}/history`, { headers })).json()).undo_count as number
  }
  async function state() {
    const response = await request.get(`${backend}/documents/${document.id}/pdf`, { headers })
    return JSON.parse(execFileSync(python, ['-c', `import pymupdf,sys,json
with pymupdf.open(stream=sys.stdin.buffer.read(),filetype='pdf') as pdf:
 print(json.dumps([{'text':page.get_text().strip(),'rotation':page.rotation} for page in pdf]))
`], { input: await response.body(), encoding: 'utf8' })) as { text: string, rotation: number }[]
  }
  await page.setViewportSize({width:1440,height:900})
  await page.goto('/');await enterLatestMatter(page)
  await expect(page.locator('.open-workspace-tab.active .open-workspace-tab-select:visible, .application-matter:visible').filter({hasText:'E2E Page Organizer'})).toBeVisible()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', { name: 'Pages', exact: true }).click()
  await page.getByRole('button', { name: 'Organize pages', exact: true }).click()
  const organizer = page.getByRole('dialog', { name: 'Organize pages' })
  await expect(organizer).toBeVisible()
  await expect.poll(async()=>{const box=await organizer.boundingBox();return [Math.round(box!.x),Math.round(box!.y),Math.round(box!.width),Math.round(box!.height)]}).toEqual([64,116,1376,756])
  await expect(organizer.getByRole('button', { name: 'Next page grid' })).toBeEnabled()
  await expect(organizer.locator('canvas')).toHaveCount(24)
  await expect(organizer.locator('.thumbnail-ready').first()).toBeVisible()
  await organizer.screenshot({ path: test.info().outputPath('page-organizer.png') })
  await page.screenshot({ path: test.info().outputPath('09A-full-workspace.png'), animations:'disabled' })
  await organizer.getByRole('checkbox', { name: 'Select page 2', exact: true }).click()
  await organizer.getByRole('checkbox', { name: 'Select page 5', exact: true }).click({ modifiers: ['Shift'] })
  await expect(organizer.getByText('Revision 1 · 4 pages selected · Imported original preserved')).toBeVisible()
  await organizer.getByRole('button', { name: 'Selection options' }).click()
  await organizer.getByLabel('Select page ranges', { exact: true }).fill('1-2, 25-26')
  await organizer.getByRole('button', { name: 'Select ranges', exact: true }).click()
  await organizer.getByRole('button', { name: 'Selection options' }).click()
  await organizer.getByRole('button', { name: 'Next page grid' }).click()
  await expect(organizer.locator('canvas')).toHaveCount(6)
  await expect(organizer.getByRole('checkbox', { name: 'Select page 25', exact: true })).toBeChecked()
  await expect(organizer.getByRole('checkbox', { name: 'Select page 26', exact: true })).toBeChecked()
  await expect(organizer.getByText('Revision 1 · 4 pages selected · Imported original preserved')).toBeVisible()
  const before = await history()
  await organizer.getByRole('button', { name: 'Rotate right', exact: true }).click()
  await expect(organizer.getByRole('region', { name: 'Review page action' })).toContainText('Pages 1–2, 25–26')
  await organizer.getByRole('button', { name: 'Rotate 4 pages', exact: true }).click()
  await expect(organizer.getByRole('status')).toContainText('4 pages rotated')
  expect(await history()).toBe(before + 1)
  expect((await state()).map(item => item.rotation)).toEqual(Array.from({ length: 30 }, (_, index) => [0, 1, 24, 25].includes(index) ? 90 : 0))
  await organizer.getByRole('button', { name: 'Done editing', exact: true }).click()
  await page.getByRole('button', { name: 'Undo last matter change', exact: true }).click()
  await expect.poll(async () => (await state()).every(item => item.rotation === 0)).toBeTruthy()

  await page.getByRole('button', { name: 'Organize pages', exact: true }).click()
  await expect(organizer.getByRole('button', { name: 'Move page 1 later', exact: true })).toBeEnabled()
  const beforePreview = await history()
  await organizer.getByRole('button', { name: 'Move page 1 later', exact: true }).click()
  await expect(organizer.locator('.organizer-page').first()).toHaveAttribute('data-page', '1')
  await expect(organizer.getByRole('button', { name: 'Apply page order', exact: true })).toBeEnabled()
  expect(await history()).toBe(beforePreview)
  page.once('dialog', prompt => prompt.accept())
  await organizer.getByRole('button', { name: 'Close page organizer' }).click()
  expect((await state())[0].text).toBe('Organizer synthetic page 1')

  await page.getByRole('button', { name: 'Organize pages', exact: true }).click()
  await expect(organizer.getByRole('button', { name: 'Move page 1 later', exact: true })).toBeEnabled()
  await organizer.getByRole('button', { name: 'Move page 1 later', exact: true }).click()
  await organizer.getByRole('button', { name: 'Apply page order', exact: true }).click()
  await organizer.getByRole('region', { name: 'Review page action' }).getByRole('button', { name: 'Apply page order', exact: true }).click()
  await expect(organizer.getByRole('status')).toContainText('Page order applied')
  expect((await state()).slice(0, 3).map(item => item.text)).toEqual([
    'Organizer synthetic page 2', 'Organizer synthetic page 1', 'Organizer synthetic page 3',
  ])
  expect(await history()).toBe(beforePreview + 1)
  await organizer.getByRole('button', { name: 'Selection options' }).click()
  await organizer.getByRole('button', { name: 'Select all', exact: true }).click()
  await expect(organizer.getByRole('button', { name: 'Delete selected', exact: true })).toBeDisabled()
  await organizer.getByLabel('Select page ranges', { exact: true }).fill('3-4')
  await organizer.getByRole('button', { name: 'Select ranges', exact: true }).click()
  await organizer.getByRole('button', { name: 'Selection options' }).click()
  await organizer.getByRole('button', { name: 'Delete selected', exact: true }).click()
  await expect(organizer.getByRole('alertdialog', { name: 'Review page action' })).toBeVisible()
  await page.screenshot({ path: test.info().outputPath('09B-delete-confirmation.png'), animations:'disabled' })
  await organizer.getByRole('button', { name: 'Delete 2 pages', exact: true }).click()
  await expect(organizer.getByRole('status')).toContainText('2 pages deleted')
  expect(await state()).toHaveLength(28)
  expect(await history()).toBe(beforePreview + 2)
  await organizer.getByRole('button', { name: 'Done editing', exact: true }).click()
  await page.getByRole('button', { name: 'Undo last matter change', exact: true }).click()
  await expect.poll(async () => (await state()).length).toBe(30)

  await page.getByRole('button', { name: 'Organize pages', exact: true }).click()
  await organizer.getByRole('checkbox', { name: 'Select page 3', exact: true }).click()
  await organizer.getByRole('checkbox', { name: 'Select page 4', exact: true }).click()
  await organizer.getByRole('button', { name: 'Replace selected' }).click()
  const replace = organizer.getByRole('dialog', { name: 'Replace working pages' })
  await expect(replace).toBeVisible()
  await expect(replace.getByRole('button',{name:'Choose file',exact:true}).last()).toBeFocused()
  await replace.locator('input[type=file]').setInputFiles({name:'replacement.pdf',mimeType:'application/pdf',buffer:syntheticPdf(2)})
  await expect(replace).toContainText('replacement.pdf · 2 pages')
  await expect(replace.getByRole('checkbox',{name:'Review page mapping after replacement'})).toBeChecked()
  await replace.getByRole('checkbox',{name:'Review page mapping after replacement'}).uncheck()
  await page.screenshot({ path: test.info().outputPath('09C-replace-file.png'), animations:'disabled' })
  await replace.getByRole('button', { name: 'Review replacement' }).click()
  await organizer.getByRole('alertdialog', { name: 'Review page action' }).getByRole('button', { name: 'Replace 2 pages' }).click()
  await expect(organizer.getByRole('status')).toContainText('2 working pages replaced')
  expect((await state()).slice(2,4).map(item=>item.text)).toEqual(['Organizer synthetic page 1','Organizer synthetic page 2'])
  await organizer.getByRole('checkbox', { name: 'Select page 2', exact: true }).click()
  await organizer.getByRole('button', { name: 'Crop', exact: true }).click()
  const tools = page.getByRole('dialog', { name: 'Page tools' })
  await expect(tools).toBeVisible()
  await expect(tools.getByLabel('Pages to crop')).toHaveValue('2')
  await tools.getByRole('button',{name:'Close page tools'}).click()
  await page.getByRole('button',{name:'Organize pages',exact:true}).click()
  await organizer.getByRole('checkbox',{name:'Select page 1',exact:true}).click()
  await organizer.getByRole('button',{name:'Replace selected'}).click()
  const reviewMapping = organizer.getByRole('dialog',{name:'Replace working pages'})
  await reviewMapping.locator('input[type=file]').setInputFiles({name:'mapping.pdf',mimeType:'application/pdf',buffer:syntheticPdf(1)})
  await expect(reviewMapping.getByRole('checkbox',{name:'Review page mapping after replacement'})).toBeChecked()
  await reviewMapping.getByRole('button',{name:'Review replacement'}).click()
  await organizer.getByRole('alertdialog',{name:'Review page action'}).getByRole('button',{name:'Replace 1 page'}).click()
  await expect(page.locator('.bundle-structure')).toBeVisible()
  expect(errors).toEqual([])
})

test('10A signature warning requires a reviewed choice before page rotation', async ({page,request},info) => {
  const created = await request.post(`${backend}/matters`, {headers,data:{name:'Synthetic signature warning',court_profile:'general-demo'}})
  expect(created.ok()).toBeTruthy()
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, {headers,
    multipart:{files:{name:'signature-warning.pdf',mimeType:'application/pdf',buffer:syntheticPdf(3)}},
  })
  expect(uploaded.ok()).toBeTruthy()
  const [doc] = await uploaded.json()
  const marked = await request.patch(`${backend}/documents/${doc.id}`,{headers,data:{visible_signature_status:'present'}})
  expect(marked.ok()).toBeTruthy()
  const markedDoc = await marked.json()
  await page.setViewportSize({width:1440,height:900})
  await page.goto('/');await enterLatestMatter(page);await closeWorkspaceNavigation(page)
  await page.getByRole('tab',{name:'Pages',exact:true}).click()
  await page.getByRole('button',{name:'Organize pages',exact:true}).click()
  const organizer = page.getByRole('dialog',{name:'Organize pages'})
  await organizer.getByRole('checkbox',{name:'Select page 1',exact:true}).click()
  await organizer.getByRole('button',{name:'Rotate right',exact:true}).click()
  const warning = organizer.getByRole('alertdialog',{name:'Possible signature detected'})
  await expect(warning).toBeVisible()
  await expect(warning.getByRole('button',{name:'Cancel'})).toBeFocused()
  await expect.poll(async()=>{const box=await warning.boundingBox();return [Math.round(box!.x),Math.round(box!.y),Math.round(box!.width),Math.round(box!.height)]}).toEqual([410,234,620,376])
  await page.screenshot({path:info.outputPath('10A-signature-warning.png'),animations:'disabled'})
  await warning.getByRole('button',{name:'Review page'}).click()
  await expect(warning).toBeHidden()
  await organizer.getByRole('button',{name:'Rotate right',exact:true}).click()
  await warning.getByRole('button',{name:'Continue'}).click()
  await expect(organizer.getByRole('alertdialog',{name:'Review page action'})).toBeVisible()
  await expect(organizer.getByRole('alertdialog',{name:'Review page action'}).getByRole('button',{name:'Rotate 1 page'})).toBeFocused()
  await organizer.getByRole('alertdialog',{name:'Review page action'}).getByRole('button',{name:'Cancel'}).click()
  const current = (await (await request.get(`${backend}/matters/${matterId}/documents`,{headers})).json())[0]
  expect(current.revision).toBe(markedDoc.revision)
})
