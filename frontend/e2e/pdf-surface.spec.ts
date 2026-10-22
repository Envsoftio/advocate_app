import { expect, test } from '@playwright/test'
import { execFileSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { resolve } from 'node:path'

function syntheticPdf() {
  const backend = resolve(import.meta.dirname, '../../backend')
  const python = process.env.COURTBUNDLE_TEST_PYTHON || [
    resolve(backend, '.venv312/bin/python'), resolve(backend, '.venv/bin/python'),
  ].find(existsSync) || 'python3'
  return execFileSync(python, ['-c', `import pymupdf,base64
pdf=pymupdf.open()
pdf.new_page(width=420,height=540)
pdf.new_page(width=420,height=540)
p=pdf[0]
p.insert_text((36,48),'Needle Needle',fontsize=18)
p.insert_text((36,90),'Alpha',fontsize=18)
w=pymupdf.get_text_length('Alpha',fontname='helv',fontsize=18)
p.insert_text((36+w,90),'Beta',fontsize=18,fontname='hebo')
p.insert_link({'kind':pymupdf.LINK_GOTO,'from':pymupdf.Rect(36,120,180,144),'page':1})
for i,uri in enumerate(['https://example.com/review','mailto:reviewer@example.com','javascript:alert(1)','file:///private/test.pdf']):
 p.insert_link({'kind':pymupdf.LINK_URI,'from':pymupdf.Rect(36,160+i*40,200,184+i*40),'uri':uri})
p.set_rotation(90)
pdf[1].insert_text((36,48),'Second page',fontsize=18)
print(base64.b64encode(pdf.tobytes()).decode())
`], {encoding:'utf8'}).trim()
}

test('shared PDF surface preserves rotated search geometry, safe links, and editor events', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const fixture = syntheticPdf()
  // Test-only interception keeps the component isolated without adding an app route.
  await page.route('**/reader-surface-test', route => route.fulfill({contentType:'text/html',body:`
    <!doctype html><html><body style="margin:0"><div id="surface"></div>
    <script type="module">
    import {createApp,h,reactive,markRaw} from '/node_modules/.vite/deps/vue.js';
    import * as pdfjs from '/node_modules/.vite/deps/pdfjs-dist.js';
    import Surface from '/src/components/PdfPageSurface.vue';
    import {findPdfTextMatches} from '/src/lib/pdfSearch.ts';
    pdfjs.GlobalWorkerOptions.workerSrc='/node_modules/pdfjs-dist/build/pdf.worker.min.mjs';
    const bytes=Uint8Array.from(atob('${fixture}'),character=>character.charCodeAt(0));
    const pdf=markRaw(await pdfjs.getDocument({data:bytes}).promise);
    const state=reactive({pageNumber:1,scale:4/3,active:true,interactive:true,searchFragments:[],activeMatchIndex:0});
    const events={ready:[],internal:[],external:[],pointers:0,clicks:0,errors:[]};
    let surface;
    const app=createApp({render:()=>h(Surface,{...state,pdf,ref:value=>surface=value,
      onReady:value=>events.ready.push(value),onError:value=>events.errors.push(value),
      onInternalLink:value=>events.internal.push(value),onExternalLink:value=>events.external.push(value),
      onPointerdown:()=>events.pointers++,onClick:()=>events.clicks++},
      {default:()=>h('button',{id:'overlay',style:'position:absolute;left:10px;top:10px;z-index:5'},'Editor overlay')})});
    app.mount('#surface');
    window.readerTest={state,events,async find(query){
      const content=await (await pdf.getPage(state.pageNumber)).getTextContent();
      const result=findPdfTextMatches(content.items,query,{page:state.pageNumber});
      state.searchFragments=result.matches.flatMap(match=>match.fragments);
      return result.matches;
    },scrollToMatch:index=>surface.scrollToMatch(index)};
    </script></body></html>`}))
  await page.goto('/reader-surface-test')
  await expect.poll(() => page.evaluate(() => (window as any).readerTest?.events.ready.length || 0)).toBe(1)
  await expect(page.locator('.pdf-text-layer span').first()).toBeVisible()
  const geometry = await page.locator('.pdf-page-surface').boundingBox()
  expect(geometry?.width).toBeCloseTo(720, 0)
  expect(geometry?.height).toBeCloseTo(560, 0)
  await expect(page.locator('.pdf-text-layer')).toHaveAttribute('data-main-rotation', '90')

  const repeated = await page.evaluate(() => (window as any).readerTest.find('Needle'))
  expect(repeated).toHaveLength(2)
  await expect(page.locator('.pdf-search-highlight')).toHaveCount(2)
  const highlights = await page.locator('.pdf-search-highlight').evaluateAll(elements => elements.map(element => {
    const r=element.getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,height:r.height}
  }))
  expect(highlights[0].height).toBeLessThan(highlights[1].y-highlights[0].y)
  expect(highlights[0].x).toBeCloseTo(highlights[1].x, 0)
  expect(highlights[0].width).toBeGreaterThan(10)
  const textBounds = await page.locator('.pdf-text-layer span').filter({hasText:'Needle Needle'}).boundingBox()
  expect(highlights[0].height).toBeLessThan((textBounds?.height || 0)*.6)
  expect(highlights[0].x).toBeGreaterThanOrEqual(0)
  expect(highlights[0].y+highlights[0].height).toBeLessThanOrEqual(geometry!.height)
  await expect(page.locator('.pdf-search-highlight.is-active')).toHaveAttribute('data-pdf-match', '0')
  await page.evaluate(() => { (window as any).readerTest.state.activeMatchIndex=1 })
  await expect(page.locator('.pdf-search-highlight.is-active')).toHaveAttribute('data-pdf-match', '1')

  const acrossRuns = await page.evaluate(() => (window as any).readerTest.find('AlphaBeta'))
  expect(acrossRuns).toHaveLength(1)
  expect(acrossRuns[0].fragments).toHaveLength(2)
  await expect(page.locator('.pdf-search-highlight')).toHaveCount(2)

  await expect(page.locator('.pdf-native-link')).toHaveCount(3)
  await page.getByRole('button', {name:'Go to PDF page 2',exact:true}).click()
  await page.getByRole('button', {name:'Open https://example.com/review',exact:true}).click()
  expect(await page.evaluate(() => (window as any).readerTest.events.internal)).toEqual([2])
  expect(await page.evaluate(() => (window as any).readerTest.events.external)).toEqual(['https://example.com/review'])
  expect(await page.evaluate(() => (window as any).readerTest.events.clicks)).toBe(0)

  await page.evaluate(() => { (window as any).readerTest.state.interactive=false })
  await expect(page.locator('.pdf-native-link')).toHaveCount(0)
  expect(await page.locator('.pdf-text-layer').evaluate(element => getComputedStyle(element).pointerEvents)).toBe('none')
  await page.getByRole('button', {name:'Editor overlay',exact:true}).click()
  expect(await page.evaluate(() => (window as any).readerTest.events.pointers)).toBe(1)
  expect(await page.evaluate(() => (window as any).readerTest.events.clicks)).toBe(1)

  await page.evaluate(() => { (window as any).readerTest.state.active=false })
  await expect.poll(() => page.locator('canvas.pdf-paper').evaluate(canvas => (canvas as HTMLCanvasElement).width)).toBe(0)
  await expect(page.locator('.pdf-text-layer span')).toHaveCount(0)
  expect((await page.locator('.pdf-page-surface').boundingBox())?.height).toBeCloseTo(560, 0)
  await page.evaluate(() => {
    const test=(window as any).readerTest;test.state.active=true;test.state.pageNumber=2;test.state.scale=1;
  })
  await expect.poll(() => page.evaluate(() => (window as any).readerTest.events.ready.at(-1)?.page)).toBe(2)
  await expect(page.locator('.pdf-text-layer')).toHaveAttribute('data-main-rotation', '0')
  await expect(page.locator('.pdf-text-layer span').filter({hasText:'Second page'})).toHaveCount(1)
  expect(await page.evaluate(() => (window as any).readerTest.events.errors)).toEqual([])
  expect(errors).toEqual([])
})
