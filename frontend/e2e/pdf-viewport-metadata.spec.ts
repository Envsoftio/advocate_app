import { expect, test } from '@playwright/test'
import { execFileSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { resolve } from 'node:path'

function fixture() {
  const backend=resolve(import.meta.dirname,'../../backend')
  const python=process.env.COURTBUNDLE_TEST_PYTHON || [resolve(backend,'.venv312/bin/python'),resolve(backend,'.venv/bin/python')].find(existsSync)||'python3'
  return execFileSync(python,['-c',`import pymupdf,base64
pdf=pymupdf.open()
for i in range(50):
 p=pdf.new_page(width=800 if i==49 else 400,height=300 if i==49 else 600)
 p.insert_text((30,50),f'Selectable page {i+1}',fontsize=18)
print(base64.b64encode(pdf.tobytes()).decode())`],{encoding:'utf8'}).trim()
}

test('distant mixed-size jumps measure their target before painting while background metadata is delayed',async({page})=>{
  const errors:string[]=[];page.on('pageerror',error=>errors.push(error.message))
  const bytes=fixture()
  await page.route('**/viewport-metadata-test',route=>route.fulfill({contentType:'text/html',body:`
    <!doctype html><html><body style="margin:0"><div id="test" style="width:900px;height:600px;display:flex"></div>
    <script type="module">
    import {createApp,h,reactive,markRaw} from '/node_modules/.vite/deps/vue.js';
    import * as pdfjs from '/node_modules/.vite/deps/pdfjs-dist.js';
    import Viewport from '/src/components/PdfReaderViewport.vue';
    pdfjs.GlobalWorkerOptions.workerSrc='/node_modules/pdfjs-dist/build/pdf.worker.min.mjs';
    const pdf=markRaw(await pdfjs.getDocument({data:Uint8Array.from(atob('${bytes}'),c=>c.charCodeAt(0))}).promise);
    let releaseBackground,releaseTarget;
    const background=new Promise(resolve=>releaseBackground=resolve),target=new Promise(resolve=>releaseTarget=resolve);
    const getPage=pdf.getPage.bind(pdf),requested=[];
    pdf.getPage=async number=>{requested.push(number);if(number===2)await background;if(number===50)await target;return getPage(number)};
    const state=reactive({page:1,zoom:'100',mode:'single'}),ready=[],failures=[];
    let reader;
    createApp({render:()=>h(Viewport,{pdf,...state,ref:value=>reader=value,'onUpdate:page':value=>state.page=value,
      onReady:value=>ready.push(value),onError:value=>failures.push(value)})}).mount('#test');
    window.viewportTest={state,requested,ready,failures,releaseBackground,releaseTarget,
      waitForPage:number=>reader.waitForPage(number),capture:()=>reader.capture()};
    </script></body></html>`}))
  await page.goto('/viewport-metadata-test')
  await expect.poll(()=>page.evaluate(()=>(window as any).viewportTest?.ready.length)).toBe(1)
  await expect.poll(()=>page.evaluate(()=>(window as any).viewportTest.requested.includes(2))).toBeTruthy()
  await page.evaluate(()=>{(window as any).viewportTest.state.page=50})
  await expect.poll(()=>page.evaluate(()=>(window as any).viewportTest.requested.includes(50))).toBeTruthy()
  await expect(page.locator('[data-reader-page="50"]')).toHaveCount(1)
  expect(await page.locator('[data-reader-page="50"] canvas').evaluate(canvas=>(canvas as HTMLCanvasElement).width)).toBe(0)
  expect(await page.evaluate(()=>(window as any).viewportTest.ready.some((item:any)=>item.page===50))).toBeFalsy()
  await page.evaluate(()=>{(window as any).viewportTest.releaseTarget()})
  expect(await page.evaluate(()=>(window as any).viewportTest.waitForPage(50))).toBeTruthy()
  const frame=await page.locator('[data-reader-page="50"]').boundingBox()
  const bitmap=await page.locator('[data-reader-page="50"] canvas').boundingBox()
  expect(frame!.width/frame!.height).toBeCloseTo(8/3,3)
  expect(bitmap!.width/bitmap!.height).toBeCloseTo(8/3,3)
  expect(await page.evaluate(()=>(window as any).viewportTest.requested.includes(5))).toBeFalsy()
  expect(await page.evaluate(()=>(window as any).viewportTest.failures)).toEqual([])
  await page.evaluate(()=>{(window as any).viewportTest.releaseBackground()})
  expect(errors).toEqual([])
})

test('budget changes preserve text selection when effective raster dimensions stay the same',async({page})=>{
  const bytes=fixture()
  await page.route('**/surface-budget-test',route=>route.fulfill({contentType:'text/html',body:`
    <!doctype html><html><body style="margin:0"><div id="test"></div>
    <script type="module">
    import {createApp,h,reactive,markRaw,nextTick} from '/node_modules/.vite/deps/vue.js';
    import * as pdfjs from '/node_modules/.vite/deps/pdfjs-dist.js';
    import Surface from '/src/components/PdfPageSurface.vue';
    pdfjs.GlobalWorkerOptions.workerSrc='/node_modules/pdfjs-dist/build/pdf.worker.min.mjs';
    const pdf=markRaw(await pdfjs.getDocument({data:Uint8Array.from(atob('${bytes}'),c=>c.charCodeAt(0))}).promise);
    const state=reactive({maxRasterPixels:8000000}),ready=[];
    createApp({render:()=>h(Surface,{pdf,pageNumber:1,scale:1,...state,onReady:value=>ready.push(value)})}).mount('#test');
    window.budgetTest={state,ready,async change(value){state.maxRasterPixels=value;await nextTick();await new Promise(requestAnimationFrame)}};
    </script></body></html>`}))
  await page.goto('/surface-budget-test')
  await expect(page.locator('.pdf-text-layer span')).toHaveCount(1)
  await page.evaluate(()=>{
    const text=document.querySelector('.pdf-text-layer span')!.firstChild!,range=document.createRange();
    range.selectNodeContents(text);getSelection()!.removeAllRanges();getSelection()!.addRange(range);
    (window as any).selectedTextNode=text;
  })
  await page.evaluate(()=>(window as any).budgetTest.change(6400000))
  await expect(page.locator('.pdf-page-surface')).toHaveAttribute('aria-busy','false')
  expect(await page.evaluate(()=>(window as any).budgetTest.ready.length)).toBe(1)
  expect(await page.evaluate(()=>getSelection()!.toString())).toBe('Selectable page 1')
  expect(await page.evaluate(()=>document.querySelector('.pdf-text-layer span')!.firstChild===(window as any).selectedTextNode)).toBeTruthy()
  // Crossing the real raster threshold still repaints and enforces the limit.
  await page.evaluate(()=>(window as any).budgetTest.change(10000))
  await expect.poll(()=>page.evaluate(()=>(window as any).budgetTest.ready.length)).toBe(2)
  expect(await page.locator('canvas.pdf-paper').evaluate(canvas=>(canvas as HTMLCanvasElement).width*(canvas as HTMLCanvasElement).height)).toBeLessThanOrEqual(10000)
})
