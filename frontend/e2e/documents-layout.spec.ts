import { closeDocumentActions, closeDocumentBrowser, openDocumentActions } from './navigation'
import { expect, test } from '@playwright/test'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'

// Browser pixels, including the persisted interface zoom. A DOM-visible canvas alone
// does not catch the old zero-height/clipped reader when Details adds another grid row.
async function visiblePaper(page: import('@playwright/test').Page) {
  return page.locator('.preview-panel .pdf-paper').first().evaluate(el => {
    const paper = el.getBoundingClientRect()
    let left = Math.max(0, paper.left), top = Math.max(0, paper.top)
    let right = Math.min(innerWidth, paper.right), bottom = Math.min(innerHeight, paper.bottom)
    for (let parent = el.parentElement; parent; parent = parent.parentElement) {
      const style = getComputedStyle(parent), rect = parent.getBoundingClientRect()
      if (/(auto|scroll|hidden|clip)/.test(style.overflowX)) { left = Math.max(left, rect.left); right = Math.min(right, rect.right) }
      if (/(auto|scroll|hidden|clip)/.test(style.overflowY)) { top = Math.max(top, rect.top); bottom = Math.min(bottom, rect.bottom) }
    }
    const center = document.elementFromPoint((left + right) / 2, (top + bottom) / 2)
    return { width: Math.max(0, right - left), height: Math.max(0, bottom - top), uncovered: !!center?.closest('.reader-page-frame') }
  })
}

for (const viewport of [{ width: 1440, height: 900 }, { width: 1024, height: 768 }]) {
  test(`New reader keeps usable paper, metadata and keyboard focus at ${viewport.width}`, async ({ page, request }) => {
    test.setTimeout(180_000);page.setDefaultTimeout(15_000)
    const base='http://127.0.0.1:8768',headers={Authorization:'Bearer e2e-isolated-token'}
    const matter=await (await request.post(`${base}/matters`,{headers,data:{name:`Synthetic Documents ${viewport.width}`,court_profile:'general-demo'}})).json()
    const uploaded=await request.post(`${base}/matters/${matter.id}/documents?included=true`,{headers,multipart:{files:{name:'01_petition.pdf',mimeType:'application/pdf',buffer:await readFile(resolve(import.meta.dirname,'../../samples/01_petition.pdf'))}}})
    expect(uploaded.ok()).toBeTruthy();const [document]=await uploaded.json()
    const title='Synthetic petition with a deliberately long document title for narrow reader header verification'
    expect((await request.patch(`${base}/documents/${document.id}`,{headers,data:{title}})).ok()).toBeTruthy()
    await page.setViewportSize(viewport);await page.goto('/')
    for(const theme of ['light','dark'])for(const scale of [90,100,110,125]) {
      await test.step(`${theme} ${scale}%`,async()=>{
        await page.evaluate(({theme,scale})=>localStorage.setItem('courtbundle.appearance.v1',JSON.stringify({theme,scale})),{theme,scale});await page.reload()
        await closeDocumentBrowser(page)
        await expect(page.locator('.pdf-paper')).toBeVisible()
        await page.getByRole('combobox',{name:'Zoom',exact:true}).selectOption('fit-width')
        const viewer=await page.locator('.viewer-root').elementHandle()
        const assertPaper=async()=>{
          await expect.poll(async()=>(await visiblePaper(page)).width).toBeGreaterThan(200)
          await expect.poll(async()=>(await visiblePaper(page)).height).toBeGreaterThan(180)
          expect((await visiblePaper(page)).uncovered).toBe(true)
          expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth && document.documentElement.scrollHeight<=innerHeight+1)).toBe(true)
        }
        await assertPaper()
        await expect(page.locator('.reader-document strong')).toHaveAttribute('title',title)
        expect(await page.locator('.reader-document').evaluate(el=>el.scrollWidth<=el.clientWidth)).toBe(true)
        await openDocumentActions(page)
        for(const name of ['Save working copy','Print working copy','Full screen','Document details'])await expect(page.getByRole('button',{name,exact:true})).toBeInViewport()
        await page.getByRole('button',{name:'Document details',exact:true}).click()
        const close=page.getByRole('button',{name:'Close document details',exact:true})
        await expect(close).toBeFocused();await expect(page.getByRole('textbox',{name:'Display title',exact:true})).toBeInViewport()
        await assertPaper()
        await page.keyboard.press('Escape')
        await expect(page.getByRole('button',{name:'More document actions'})).toBeFocused()
        await openDocumentActions(page);await page.getByRole('button',{name:'Document details',exact:true}).click()
        await close.press('Enter');await expect(page.getByRole('button',{name:'More document actions'})).toBeFocused()
        expect(await viewer!.evaluate(el=>el.isConnected)).toBe(true)
        await page.locator('.viewer-root').focus();await page.keyboard.press('/')
        await expect(page.getByRole('textbox',{name:'Search documents',exact:true})).toBeFocused()
        await closeDocumentBrowser(page)
        await page.getByRole('button',{name:'Next page',exact:true}).click();await expect(page.locator('.page-counter input')).toHaveValue('2')
        await page.getByRole('button',{name:'Previous page',exact:true}).click();await expect(page.locator('.page-counter input')).toHaveValue('1')
        const information=page.getByRole('button',{name:'Document information',exact:true})
        await information.click();await expect(page.getByRole('button',{name:'Close document information'})).toBeFocused();await assertPaper()
        const focus=page.getByRole('button',{name:'Focused reading',exact:true});await focus.click()
        await expect(page.getByRole('region',{name:'Document information'})).toBeHidden();await assertPaper()
        await page.keyboard.press('Escape');await expect(focus).toBeFocused()
        await expect(page.getByRole('region',{name:'Document information'})).toBeVisible()
        await page.getByRole('button',{name:'Close document information'}).click()
        await expect(information).toBeFocused();expect(await viewer!.evaluate(el=>el.isConnected)).toBe(true)
        await closeDocumentActions(page)
      })
    }
  })
}
