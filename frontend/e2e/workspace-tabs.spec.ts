import { closeWorkspaceNavigation, openDocumentActions, openDocumentBrowser, openWorkspaceNavigation, openWorkspaces } from './navigation'
import { expect, test } from '@playwright/test'
import { execFileSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { resolve } from 'node:path'

const backend = 'http://127.0.0.1:8768'
const headers = { Authorization: 'Bearer e2e-isolated-token' }
const backendRoot = resolve(import.meta.dirname, '../../backend')
const python = process.env.COURTBUNDLE_TEST_PYTHON || [
  resolve(backendRoot, '.venv312/bin/python'), resolve(backendRoot, '.venv/bin/python'),
].find(existsSync) || 'python3'

function documentBytes(label:string) {
  return execFileSync(python, ['-c', `import pymupdf,sys
with pymupdf.open() as pdf:
 for index in range(3):
  page=pdf.new_page()
  page.insert_text((40,80),f'{sys.argv[1]} page {index+1}')
 sys.stdout.buffer.write(pdf.tobytes())
`, label])
}

test('matter and document tabs keep their selection, reader position and unsaved details', async ({page, request}) => {
  const first = await (await request.post(`${backend}/matters`, {headers,data:{
    name:'E2E UX01 First Matter',court_profile:'general-demo',
  }})).json()
  const second = await (await request.post(`${backend}/matters`, {headers,data:{
    name:'E2E UX01 Second Matter',court_profile:'general-demo',
  }})).json()
  for (const label of ['alpha-tabs','bravo-tabs']) {
    const response = await request.post(`${backend}/matters/${first.id}/documents?included=true`, {headers,multipart:{
      files:{name:`${label}.pdf`,mimeType:'application/pdf',buffer:documentBytes(label)},
    }})
    expect(response.ok()).toBeTruthy()
  }
  await page.goto('/')
  await openWorkspaceNavigation(page)
  await page.locator('.matter-link').filter({hasText:'E2E UX01 First Matter'}).click()
  await openDocumentBrowser(page)
  await page.locator('.doc-item').filter({hasText:'alpha-tabs'}).click()
  await expect(page.locator('.reader-document strong')).toHaveText('alpha-tabs')
  await page.getByRole('spinbutton', {name:'Go to page'}).fill('2')
  await page.getByRole('spinbutton', {name:'Go to page'}).press('Enter')
  await expect(page.getByRole('spinbutton', {name:'Go to page'})).toHaveValue('2')
  await page.getByRole('combobox', {name:'Zoom',exact:true}).selectOption('75')
  await openDocumentActions(page)
  await page.getByRole('button', {name:'Document details',exact:true}).click()
  await page.getByRole('textbox', {name:'Display title'}).fill('Unsaved alpha title')
  await openDocumentBrowser(page)
  await page.locator('.doc-item').filter({hasText:'bravo-tabs'}).click()
  await expect(page.locator('.reader-document strong')).toHaveText('bravo-tabs')
  await openWorkspaceNavigation(page)
  await page.locator('.matter-link').filter({hasText:'E2E UX01 Second Matter'}).click()
  await expect(page.locator('.application-matter')).toHaveText('E2E UX01 Second Matter')
  await openWorkspaces(page)
  await page.locator('.open-workspace-tabs .open-workspace-tab').filter({hasText:'alpha-tabs'})
    .locator('.open-workspace-tab-select').click()
  await expect(page.locator('.reader-document strong')).toHaveText('alpha-tabs')
  await expect(page.getByRole('textbox', {name:'Display title'})).toHaveValue('Unsaved alpha title')
  await expect(page.getByRole('spinbutton', {name:'Go to page'})).toHaveValue('2')
  await expect(page.getByRole('combobox', {name:'Zoom',exact:true})).toHaveValue('75')
  await openWorkspaces(page)
  await page.getByRole('button', {name:'Close alpha-tabs tab'}).click()
  await expect(page.getByRole('alertdialog', {name:'Unsaved work in this tab'})).toBeVisible()
  await page.getByRole('button', {name:'Keep editing'}).click()
  await expect(page.getByRole('textbox', {name:'Display title'})).toHaveValue('Unsaved alpha title')
  const firstDocs = await (await request.get(`${backend}/matters/${first.id}/documents`,{headers})).json()
  expect(firstDocs.find((item:{title:string}) => item.title==='alpha-tabs')).toBeTruthy()
  await page.getByRole('textbox', {name:'Display title'}).fill('alpha-tabs')
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', {name:'Edit',exact:true}).click()
  await page.getByRole('button', {name:'Add text',exact:true}).click()
  await page.locator('.pdf-paper').click({position:{x:120,y:180}})
  await page.getByRole('textbox', {name:/Add text on page/}).fill('Unfinished tab text')
  await openWorkspaces(page)
  await page.locator('.open-workspace-tabs .open-workspace-tab').filter({hasText:'bravo-tabs'})
    .locator('.open-workspace-tab-select').click()
  await expect(page.getByRole('alertdialog', {name:'Unsaved work in this tab'})).toBeVisible()
  await page.getByRole('button', {name:'Keep editing'}).click()
  await expect(page.locator('.reader-document strong')).toHaveText('alpha-tabs')
  await expect(page.getByRole('textbox', {name:/Add text on page/})).toHaveValue('Unfinished tab text')
  await openWorkspaces(page)
  await page.locator('.open-workspace-tabs .open-workspace-tab').filter({hasText:'bravo-tabs'})
    .locator('.open-workspace-tab-select').click()
  await page.getByRole('button', {name:'Discard changes'}).click()
  await expect(page.locator('.reader-document strong')).toHaveText('bravo-tabs')
  void second
})
