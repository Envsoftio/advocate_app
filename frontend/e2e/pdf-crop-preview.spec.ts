import { closeWorkspaceNavigation } from './navigation'
import { expect, test, type Locator } from '@playwright/test'
import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { existsSync } from 'node:fs'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'

const backend = 'http://127.0.0.1:8768'
const headers = { Authorization: 'Bearer e2e-isolated-token' }
const backendRoot = resolve(import.meta.dirname, '../../backend')
const python = process.env.MATTERDRAFT_TEST_PYTHON || [
  resolve(backendRoot, '.venv312/bin/python'), resolve(backendRoot, '.venv/bin/python'),
].find(existsSync) || 'python3'
const hash = (bytes: Buffer) => createHash('sha256').update(bytes).digest('hex')

function syntheticPdf() {
  return execFileSync(python, ['-c', `import pymupdf,sys
with pymupdf.open() as pdf:
 for index,rotation in enumerate([0,90,180,270]):
  width,height=(400,600) if index<2 else (500,700)
  page=pdf.new_page(width=width,height=height)
  page.draw_rect(pymupdf.Rect(20,20,width-20,height-20),color=(0.2,0.4,0.7),width=2)
  page.insert_text((width/3,height/2),f'Synthetic crop page {index+1}',fontsize=12)
  page.set_rotation(rotation)
 sys.stdout.buffer.write(pdf.tobytes())
`])
}

function inspectPdf(bytes: Buffer) {
  return JSON.parse(execFileSync(python, ['-c', `import pymupdf,sys,json
with pymupdf.open(stream=sys.stdin.buffer.read(),filetype='pdf') as pdf:
 print(json.dumps([{'rotation':page.rotation,'crop':list(page.cropbox),'width':page.rect.width,'height':page.rect.height} for page in pdf]))
`], { input: bytes, encoding: 'utf8' })) as { rotation: number; crop: number[]; width: number; height: number }[]
}

async function visibleCrop(layer: Locator, keep: Locator) {
  const bounds = await layer.boundingBox(), crop = await keep.boundingBox()
  if (!bounds || !crop) throw new Error('Crop preview is not visible')
  return [(crop.x - bounds.x) / bounds.width * 100, (crop.y - bounds.y) / bounds.height * 100,
    crop.width / bounds.width * 100, crop.height / bounds.height * 100]
}

test('actual-page crop preview maps rotated drag/numeric selections to saved PDF dimensions', async ({ page, request }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const original = syntheticPdf()
  const created = await request.post(`${backend}/matters`, { headers, data: {
    name: 'E2E Rotated Crop Preview', court_profile: 'general-demo', max_pages_per_volume: 100,
  } })
  expect(created.ok()).toBeTruthy()
  const matterId = (await created.json()).id as string
  const uploaded = await request.post(`${backend}/matters/${matterId}/documents?included=true`, { headers,
    multipart: { files: { name: 'crop-rotations-synthetic.pdf', mimeType: 'application/pdf', buffer: original } },
  })
  expect(uploaded.ok()).toBeTruthy()
  const document = (await uploaded.json())[0]
  const documents = async () => (await (await request.get(`${backend}/matters/${matterId}/documents`, { headers })).json())[0]
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Rotated Crop Preview'})).toBeVisible()
  await closeWorkspaceNavigation(page)
  await page.getByRole('tab', { name: 'Pages', exact: true }).click()
  await page.getByRole('button', { name: 'Page tools', exact: true }).click()
  const tools = page.getByRole('dialog', { name: 'Page tools' })
  await tools.getByRole('button', { name: 'Crop margins', exact: true }).click()
  await tools.getByRole('textbox', { name: 'Pages to crop' }).fill('1-4')
  const preview = tools.getByRole('region', { name: 'Crop preview on the working PDF' })
  const selector = preview.getByRole('combobox', { name: 'Preview crop on source page' })
  const layer = preview.locator('.crop-drawing-layer')
  const keep = preview.locator('.crop-keep-area')
  const edge = (name: string) => tools.getByRole('spinbutton', { name: `${name} %`, exact: true })
  for (const [name, value] of Object.entries({ left: 10, top: 20, right: 75, bottom: 90 })) await edge(name).fill(String(value))
  const expected = [[10,20,65,70],[10,10,70,65],[25,10,65,70],[20,25,70,65]]
  for (let index = 0; index < 4; index++) {
    await selector.selectOption(String(index + 1))
    await expect(layer).toHaveAttribute('aria-disabled', 'false')
    const actual = await visibleCrop(layer, keep)
    actual.forEach((value, axis) => expect(value).toBeCloseTo(expected[index][axis], 0))
  }

  await selector.selectOption('2')
  await expect(layer).toHaveAttribute('aria-disabled', 'false')
  await layer.scrollIntoViewIfNeeded()
  const bounds = (await layer.boundingBox())!
  await page.mouse.move(bounds.x + bounds.width * .2, bounds.y + bounds.height * .15)
  await page.mouse.down()
  await page.mouse.move(bounds.x + bounds.width * .8, bounds.y + bounds.height * .85, { steps: 8 })
  await page.mouse.up()
  for (const [name, value] of Object.entries({ left: 15, top: 20, right: 85, bottom: 80 }))
    expect(Number(await edge(name).inputValue())).toBeCloseTo(value, 0)
  // A cancelled gesture changes neither the crop model nor the working revision.
  const beforeCancel = await Promise.all(['left','top','right','bottom'].map(name => edge(name).inputValue()))
  await page.mouse.move(bounds.x + bounds.width * .05, bounds.y + bounds.height * .05)
  await page.mouse.down()
  await page.mouse.move(bounds.x + bounds.width * .95, bounds.y + bounds.height * .95, { steps: 3 })
  await page.keyboard.press('Escape')
  await page.mouse.up()
  expect(await Promise.all(['left','top','right','bottom'].map(name => edge(name).inputValue()))).toEqual(beforeCancel)
  expect((await documents()).revision).toBe(1)

  // Numeric keyboard adjustment and the real page overlay remain synchronized.
  await edge('left').fill('18')
  expect((await visibleCrop(layer, keep))[1]).toBeCloseTo(18, 0)
  for (const [name, value] of Object.entries({ left: 15, top: 20, right: 85, bottom: 80 })) await edge(name).fill(String(value))
  await preview.screenshot({ path: test.info().outputPath('rotated-crop-preview.png') })
  const cropRequest = page.waitForRequest(request => request.url().endsWith('/pages/crop') && request.method() === 'POST')
  await tools.getByRole('button', { name: 'Crop selected pages', exact: true }).click()
  expect((await cropRequest).postDataJSON()).toMatchObject({ expected_revision: 1, pages: [0,1,2,3], left: .15, top: .2, right: .85, bottom: .8 })
  await expect.poll(async () => (await documents()).revision).toBe(2)
  await expect(tools.getByRole('button', { name: 'Crop selected pages', exact: true })).toBeEnabled()
  const working = await request.get(`${backend}/documents/${document.id}/pdf?revision=2`, { headers })
  expect(working.ok()).toBeTruthy()
  const metadata = inspectPdf(await working.body())
  expect(metadata).toEqual([
    { rotation: 0, crop: [60,120,340,480], width: 280, height: 360 },
    { rotation: 90, crop: [60,120,340,480], width: 360, height: 280 },
    { rotation: 180, crop: [75,140,425,560], width: 350, height: 420 },
    { rotation: 270, crop: [75,140,425,560], width: 420, height: 350 },
  ])
  expect((await documents()).source_sha256).toBe(hash(original))
  const sourcePath = resolve(process.env.MATTERDRAFT_E2E_DATA_DIR!, 'matters', matterId, 'originals', `${document.id}.pdf`)
  expect(hash(await readFile(sourcePath))).toBe(hash(original))

  await tools.getByRole('button', { name: 'Save selected pages', exact: true }).click()
  await tools.getByRole('textbox', { name: 'Pages to extract' }).fill('1-4')
  const downloaded = page.waitForEvent('download')
  await tools.getByRole('button', { name: 'Save extracted PDF', exact: true }).click()
  const saved = await downloaded
  expect(saved.suggestedFilename()).toBe('extracted-pages.pdf')
  expect(inspectPdf(await readFile(await saved.path()))).toEqual(metadata)
  expect(errors).toEqual([])
})
