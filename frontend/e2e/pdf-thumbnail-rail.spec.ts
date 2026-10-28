import { openDocumentActions, openDocumentBrowser } from './navigation'
import { expect, test } from '@playwright/test'
import { execFileSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { resolve } from 'node:path'

const base='http://127.0.0.1:8768',headers={Authorization:'Bearer e2e-isolated-token'}
const backend=resolve(import.meta.dirname,'../../backend')
const python=process.env.COURTBUNDLE_TEST_PYTHON || [resolve(backend,'.venv312/bin/python'),resolve(backend,'.venv/bin/python')].find(existsSync)||'python3'
function fixture(count:number){return execFileSync(python,['-c',`import pymupdf,sys
pdf=pymupdf.open()
for i in range(${count}):
 p=pdf.new_page(width=840 if i%3==1 else 420,height=300 if i%3==1 else 595)
 p.insert_text((25,45),f'Rail page {i+1}',fontsize=16)
 if i%7==2:p.set_rotation(90)
sys.stdout.buffer.write(pdf.tobytes(deflate=True))`])}

test('full-screen thumbnail rail stays bounded through 10, 100 and 1000 pages, keyboard navigation and reordering',async({page,request})=>{
  test.setTimeout(180_000)
  const errors:string[]=[];page.on('pageerror',error=>errors.push(error.message))
  const created=await request.post(`${base}/matters`,{headers,data:{name:'E2E Virtual Thumbnail Rail',court_profile:'general-demo'}})
  expect(created.ok()).toBeTruthy()
  const matter=(await created.json()).id
  const documents=new Map<number,string>()
  for(const count of [10,100,1000]) {
    const uploaded=await request.post(`${base}/matters/${matter}/documents?included=true`,{headers,multipart:{files:{name:`rail-${count}.pdf`,mimeType:'application/pdf',buffer:fixture(count)}}})
    expect(uploaded.ok()).toBeTruthy();documents.set(count,(await uploaded.json())[0].id)
  }
  await page.setViewportSize({width:1440,height:1000})
  await page.goto('/')
  await expect(page.locator('.application-matter').filter({hasText:'E2E Virtual Thumbnail Rail'})).toBeVisible()
  await openDocumentBrowser(page)
  await page.locator(`.doc-item[data-document-id="${documents.get(1000)}"]`).click()
  await expect(page.locator('.pdf-placement-surface')).not.toHaveClass(/is-stale/)
  await openDocumentActions(page)
  await page.getByRole('button',{name:'Full screen',exact:true}).click()
  await openDocumentBrowser(page)
  const rail=page.locator('.virtual-thumbnail-rail'),jump=page.getByRole('spinbutton',{name:'Go to page',exact:true})
  async function sharpPage() {
    await expect(page.locator('.pdf-page-surface')).toHaveAttribute('aria-busy','false')
    await expect.poll(()=>page.locator('.pdf-paper').evaluate(canvas=>(canvas as HTMLCanvasElement).width)).toBeGreaterThan(0)
  }
  const metrics:Array<Record<string,number>>=[]
  const cdp=await page.context().newCDPSession(page)
  for(const count of [10,100,1000]) {
    const firstStart=performance.now()
    await openDocumentBrowser(page)
    await page.locator(`.doc-item[data-document-id="${documents.get(count)}"]`).click()
    await expect(page.locator('.page-counter')).toContainText(`/ ${count}`)
    await expect(page.locator('.pdf-placement-surface')).not.toHaveClass(/is-stale/)
    await sharpPage()
    await expect(rail.locator('canvas[data-page="1"]')).toHaveAttribute('data-rendered','true')
    const firstRenderMs=Math.round(performance.now()-firstStart)
    expect(await rail.locator('.thumbnail-button').count()).toBeLessThan(20)
    await rail.locator('canvas[data-page="1"]').evaluate(canvas=>{(window as any).retiredThumbnail=canvas})
    const jumpStart=performance.now()
    await jump.fill(String(count));await jump.press('Enter')
    await expect(jump).toHaveValue(String(count))
    await expect(page.locator(`.reader-page-frame[data-reader-page="${count}"] .pdf-paper`)).toBeVisible()
    await expect(page.locator('.pdf-placement-surface')).not.toHaveClass(/is-stale/)
    await sharpPage()
    await expect(rail.locator(`canvas[data-page="${count}"]`)).toHaveAttribute('data-rendered','true')
    await expect(rail.locator(`[data-thumbnail-page="${count}"]`)).toBeInViewport()
    const jumpMs=Math.round(performance.now()-jumpStart)
    if(count>10) {
      await expect(rail.locator('canvas[data-page="1"]')).toHaveCount(0)
      expect(await page.evaluate(()=>({width:(window as any).retiredThumbnail.width,connected:(window as any).retiredThumbnail.isConnected}))).toEqual({width:0,connected:false})
    }
    const oldWidth=(await page.locator('.pdf-paper').boundingBox())!.width
    const zoomStart=performance.now()
    await page.getByRole('combobox',{name:'Zoom',exact:true}).selectOption('125')
    await expect.poll(async()=>Math.abs((await page.locator('.pdf-paper').boundingBox())!.width-oldWidth)).toBeGreaterThan(1)
    await expect(page.locator('.pdf-placement-surface')).not.toHaveClass(/is-stale/)
    await sharpPage()
    const zoomMs=Math.round(performance.now()-zoomStart)
    await page.getByRole('button',{name:'Search',exact:true}).click()
    const searchStart=performance.now()
    await page.getByRole('searchbox',{name:'Find text in this PDF'}).fill(`Rail page ${count}`)
    await expect(page.locator('.pdf-findbar')).toContainText('1 match · 1/1')
    const searchMs=Math.round(performance.now()-searchStart)
    await page.getByRole('button',{name:'Close PDF search'}).click()
    const canvases=await rail.locator('canvas').evaluateAll(elements=>({mounted:elements.length,pixels:elements.reduce((sum,element)=>sum+(element as HTMLCanvasElement).width*(element as HTMLCanvasElement).height,0)}))
    expect(canvases.mounted).toBeLessThan(20);expect(canvases.pixels).toBeLessThanOrEqual(canvases.mounted*256_000)
    const clipping=await rail.locator('canvas[data-rendered="true"]').evaluateAll(elements=>elements.some(canvas=>{
      const image=canvas.parentElement!.getBoundingClientRect(),box=canvas.getBoundingClientRect()
      return box.left<image.left-1||box.right>image.right+1||box.top<image.top-1||box.bottom>image.bottom+1
    }))
    expect(clipping).toBe(false)
    const memory=await cdp.send('Runtime.getHeapUsage')
    const allCanvases=await page.locator('canvas').evaluateAll(elements=>({count:elements.length,pixels:elements.reduce((sum,element)=>sum+(element as HTMLCanvasElement).width*(element as HTMLCanvasElement).height,0)}))
    metrics.push({pages:count,firstRenderMs,jumpMs,zoomMs,searchMs,thumbnailCanvases:canvases.mounted,thumbnailPixels:canvases.pixels,
      allMountedCanvases:allCanvases.count,allMountedCanvasPixels:allCanvases.pixels,jsHeapUsedBytes:memory.usedSize,jsHeapTotalBytes:memory.totalSize})
    await page.getByRole('combobox',{name:'Zoom',exact:true}).selectOption('fit-width')
  }
  console.log(`PDF_READER_DEVELOPER_HOST_METRICS ${JSON.stringify(metrics)}`)
  await test.info().attach('reader-developer-host-metrics.json',{body:JSON.stringify({notes:'Single Chrome developer-host run. JS heap excludes PDF worker and native/canvas allocations. Timings include UI/test scheduling; source import excluded.',metrics},null,2),contentType:'application/json'})
  await rail.locator('[data-thumbnail-page="1000"]').focus()
  await page.keyboard.press('Home')
  await expect(jump).toHaveValue('1')
  await expect(rail.locator('[data-thumbnail-page="1"]')).toBeFocused()
  await page.keyboard.press('ArrowDown')
  await expect(jump).toHaveValue('2')
  await expect(rail.locator('[data-thumbnail-page="2"]')).toBeFocused()
  await page.keyboard.press('End')
  await expect(jump).toHaveValue('1000')
  await expect(rail.locator('[data-thumbnail-page="1000"]')).toBeFocused()
  await expect(rail.locator('canvas:not([data-rendered="true"])')).toHaveCount(0)
  await page.screenshot({path:test.info().outputPath('virtual-thumbnail-rail-last-page.png'),fullPage:true})
  // Manual rail scrolling keeps the reader in place and leaves a keyboard entry point.
  await rail.evaluate(element=>{element.scrollTop=500*176})
  await expect.poll(()=>rail.locator('.thumbnail-button[tabindex="0"]').count()).toBe(1)
  await expect(jump).toHaveValue('1000')
  expect(await rail.locator('.thumbnail-button').count()).toBeLessThan(20)
  await jump.fill('1');await jump.press('Enter')
  await expect(rail.locator('canvas[data-page="1"]')).toHaveAttribute('data-rendered','true')
  await expect(rail.locator('canvas:not([data-rendered="true"])')).toHaveCount(0)
  await page.screenshot({path:test.info().outputPath('virtual-thumbnail-rail-first-page.png'),fullPage:true})
  async function firstPages() {
    const response=await request.get(`${base}/documents/${documents.get(1000)}/pdf`,{headers})
    return JSON.parse(execFileSync(python,['-c',`import pymupdf,sys,json
with pymupdf.open(stream=sys.stdin.buffer.read(),filetype='pdf') as pdf: print(json.dumps([pdf[i].get_text().strip() for i in range(3)]))`],{input:await response.body(),encoding:'utf8'}))
  }
  await rail.locator('[data-thumbnail-page="1"]').focus()
  await page.keyboard.press('Alt+ArrowDown')
  await expect.poll(firstPages).toEqual(['Rail page 2','Rail page 1','Rail page 3'])
  await expect(jump).toHaveValue('2')
  await expect(page.locator('.pdf-placement-surface')).not.toHaveClass(/is-stale/)
  await expect(rail.locator('[data-thumbnail-page="2"]')).toBeFocused()
  await rail.locator('[data-thumbnail-page="2"]').dragTo(rail.locator('[data-thumbnail-page="1"]'))
  await expect.poll(firstPages).toEqual(['Rail page 1','Rail page 2','Rail page 3'])
  await expect(jump).toHaveValue('1')
  expect(errors).toEqual([])
})
