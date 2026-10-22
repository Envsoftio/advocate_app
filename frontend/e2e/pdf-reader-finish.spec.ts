import { expect, test } from '@playwright/test'
import { execFileSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { resolve } from 'node:path'
const base='http://127.0.0.1:8768',headers={Authorization:'Bearer e2e-isolated-token'}
const backend=resolve(import.meta.dirname,'../../backend')
const python=process.env.COURTBUNDLE_TEST_PYTHON || [resolve(backend,'.venv312/bin/python'),resolve(backend,'.venv/bin/python')].find(existsSync)||'python3'
function fixture(tiny=false){return execFileSync(python,['-c',`import pymupdf,sys
pdf=pymupdf.open()
for i in range(${tiny?120:12}):
 p=pdf.new_page(width=${tiny?72:595},height=${tiny?72:842})
 p.insert_text((${tiny?5:36},${tiny?15:65}),f'Synthetic page {i+1}',fontsize=${tiny?5:18})
sys.stdout.buffer.write(pdf.tobytes())`])}

test('reader persists revision-scoped positions after reload and keyboard dialogs restore focus',async({page,request})=>{
  const errors:string[]=[];page.on('pageerror',error=>errors.push(error.message))
  const matter=await (await request.post(`${base}/matters`,{headers,data:{name:'E2E Reader Persistence',court_profile:'general-demo'}})).json()
  const document=(await (await request.post(`${base}/matters/${matter.id}/documents`,{headers,multipart:{files:{name:'reader-restart.pdf',mimeType:'application/pdf',buffer:fixture()}}})).json())[0]
  await page.goto('/')
  await page.locator('.doc-item').filter({hasText:'reader-restart'}).click()
  await expect(page.getByRole('spinbutton',{name:'Go to page',exact:true})).toBeEnabled()
  await page.getByRole('combobox',{name:'Reading mode'}).selectOption('continuous')
  await page.getByRole('combobox',{name:'Zoom',exact:true}).selectOption('125')
  await page.getByRole('spinbutton',{name:'Go to page',exact:true}).fill('7')
  await page.getByRole('spinbutton',{name:'Go to page',exact:true}).press('Enter')
  await expect(page.getByRole('spinbutton',{name:'Go to page',exact:true})).toHaveValue('7')
  await page.locator('.canvas-scroll').evaluate(element=>{element.scrollTop+=150})
  const key=`${document.id}:1`
  await expect.poll(async()=>{
    const state=await (await request.get(`${base}/reader-position`,{headers,params:{key}})).json()
    return state?.page===7 && state?.zoom==='125' && state?.mode==='continuous' && state?.y>.05
  }).toBeTruthy()
  const stored=await (await request.get(`${base}/reader-position`,{headers,params:{key}})).json()
  await page.reload()
  await page.locator('.doc-item').filter({hasText:'reader-restart'}).click()
  await expect(page.getByRole('spinbutton',{name:'Go to page',exact:true})).toHaveValue('7')
  await expect(page.getByRole('combobox',{name:'Zoom',exact:true})).toHaveValue('125')
  await expect(page.getByRole('combobox',{name:'Reading mode'})).toHaveValue('continuous')
  await expect.poll(()=>page.locator('.canvas-scroll').evaluate(element=>{
    const wrap=element.getBoundingClientRect(), frame=element.querySelector('[data-reader-page="7"]')!.getBoundingClientRect()
    return (wrap.top+20-frame.top)/frame.height
  })).toBeCloseTo(stored.y,1)
  const help=page.getByRole('button',{name:'PDF keyboard shortcuts',exact:true})
  await help.focus();await page.keyboard.press('Enter')
  const dialog=page.getByRole('dialog',{name:'PDF keyboard shortcuts'})
  await expect(dialog).toBeVisible()
  await expect(dialog.getByRole('button',{name:'Close shortcuts'})).toBeFocused()
  await page.keyboard.press('Escape');await expect(dialog).not.toBeVisible();await expect(help).toBeFocused()
  await page.getByRole('tab',{name:'Edit',exact:true}).focus();await page.keyboard.press('ArrowRight')
  await expect(page.getByRole('tab',{name:'Annotate',exact:true})).toBeFocused()
  await expect(page.getByRole('tab',{name:'Annotate',exact:true})).toHaveAttribute('aria-selected','true')
  await page.keyboard.press('Home');await expect(page.getByRole('tab',{name:'Edit',exact:true})).toBeFocused()
  expect(errors).toEqual([])
})

test('continuous reading covers the viewport for tiny pages with bounded raster memory',async({page,request})=>{
  const matter=await (await request.post(`${base}/matters`,{headers,data:{name:'E2E Tiny Pages',court_profile:'general-demo'}})).json()
  await request.post(`${base}/matters/${matter.id}/documents`,{headers,multipart:{files:{name:'tiny-pages.pdf',mimeType:'application/pdf',buffer:fixture(true)}}})
  await page.setViewportSize({width:1440,height:1100});await page.goto('/')
  await page.locator('.doc-item').filter({hasText:'tiny-pages'}).click()
  await expect(page.getByRole('spinbutton',{name:'Go to page',exact:true})).toBeEnabled()
  await page.getByRole('button',{name:'Full screen',exact:true}).click()
  await page.getByRole('combobox',{name:'Reading mode'}).selectOption('continuous')
  await page.getByRole('combobox',{name:'Zoom',exact:true}).selectOption('25')
  await page.getByRole('spinbutton',{name:'Go to page',exact:true}).fill('50')
  await page.getByRole('spinbutton',{name:'Go to page',exact:true}).press('Enter')
  await expect.poll(()=>page.locator('.canvas-scroll').evaluate(element=>{
    const box=element.getBoundingClientRect(), frames=[...element.querySelectorAll('.reader-page-frame')].map(frame=>frame.getBoundingClientRect())
    return frames.length>12 && frames[0].top<=box.top+24 && frames.at(-1)!.bottom>=box.bottom-24
  })).toBeTruthy()
  expect(await page.locator('.reader-page-frame').count()).toBeLessThan(80)
  const pixels=await page.locator('.reader-page-frame canvas').evaluateAll(items=>items.reduce((n,item)=>n+(item as HTMLCanvasElement).width*(item as HTMLCanvasElement).height,0))
  expect(pixels).toBeLessThanOrEqual(32_000_000)
  await page.screenshot({path:test.info().outputPath('tiny-continuous-pages.png'),fullPage:true})
})

test('single-page navigation and zoom persist without scrolling or unmounting',async({page,request})=>{
  const matter=await (await request.post(`${base}/matters`,{headers,data:{name:'E2E Reader No Scroll',court_profile:'general-demo'}})).json()
  const document=(await (await request.post(`${base}/matters/${matter.id}/documents`,{headers,multipart:{files:{name:'reader-no-scroll.pdf',mimeType:'application/pdf',buffer:fixture()}}})).json())[0]
  await page.goto('/')
  await page.locator('.doc-item').filter({hasText:'reader-no-scroll'}).click()
  const number=page.getByRole('spinbutton',{name:'Go to page',exact:true})
  await expect(number).toBeEnabled()
  await page.getByRole('combobox',{name:'Zoom',exact:true}).selectOption('125')
  await number.fill('9');await number.press('Enter')
  await expect(page.locator('.pdf-page-surface[data-page="9"]')).toHaveAttribute('aria-busy','false')
  await expect(page.getByRole('combobox',{name:'Reading mode'})).toHaveValue('single')
  expect(await page.locator('.canvas-scroll').evaluate(element=>element.scrollTop)).toBe(0)
  const key=`${document.id}:1`
  await expect.poll(async()=>{
    const state=await (await request.get(`${base}/reader-position`,{headers,params:{key}})).json()
    return state?.page===9 && state?.zoom==='125' && state?.mode==='single' && state?.y<.001
  }).toBeTruthy()
  // The save above happened while the reader was still mounted, without scroll.
  await page.reload()
  await page.locator('.doc-item').filter({hasText:'reader-no-scroll'}).click()
  await expect(number).toHaveValue('9')
  await expect(page.getByRole('combobox',{name:'Zoom',exact:true})).toHaveValue('125')
  await expect(page.getByRole('combobox',{name:'Reading mode'})).toHaveValue('single')
  await expect(page.locator('.pdf-page-surface[data-page="9"]')).toHaveAttribute('aria-busy','false')
  expect(await page.locator('.canvas-scroll').evaluate(element=>element.scrollTop)).toBeLessThan(1)
})
