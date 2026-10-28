import { expect, type Page } from '@playwright/test'
// Explicit interactions with the new shell; do not bypass drawers or menus with DOM clicks.
export async function openWorkspaceNavigation(page:Page) {
  const toggle=page.getByRole('button',{name:'Workspace navigation',exact:true})
  if(await toggle.getAttribute('aria-expanded')!=='true')await toggle.click()
  await expect(page.locator('.workspace-drawer')).toBeVisible()
}
export async function openDocumentBrowser(page:Page) {
  await closeWorkspaceNavigation(page)
  const panel=page.locator('.docs-panel')
  if(!await panel.isVisible()) {
    // Search opens the document switcher without leaving the active tool draft.
    await page.locator('.viewer-root').focus()
    await page.keyboard.press('/')
  }
  await expect(panel).toBeVisible()
}
export async function closeDocumentBrowser(page:Page) {
  const close=page.getByRole('button',{name:'Close document browser',exact:true})
  if(await close.isVisible())await close.click()
}
export async function openDocumentActions(page:Page) {
  await closeWorkspaceNavigation(page)
  const menu=page.locator('.reader-more')
  if(await menu.getAttribute('open')===null)await menu.locator('summary').click()
}
export async function closeDocumentActions(page:Page) {
  const menu=page.locator('.reader-more')
  if(await menu.getAttribute('open')!==null)await menu.locator('summary').click()
}
export async function openReaderView(page:Page) {
  await closeWorkspaceNavigation(page)
  const menu=page.locator('.reader-view-menu').filter({visible:true}).first()
  if(await menu.getAttribute('open')===null)await menu.locator('summary').click()
}
export async function openWorkspaces(page:Page) {
  await closeWorkspaceNavigation(page)
  const toggle=page.getByRole('button',{name:'Open workspaces',exact:true})
  if(await toggle.getAttribute('aria-expanded')!=='true')await toggle.click()
}

export async function closeWorkspaceNavigation(page:Page) {
  const toggle=page.getByRole('button',{name:'Workspace navigation',exact:true})
  if(await toggle.isVisible() && await toggle.getAttribute('aria-expanded')==='true')await toggle.click()
}

// New imports are staged. Existing export journeys explicitly select their sources.
export async function includeAllDocuments(page:Page) {
  await closeWorkspaceNavigation(page)
  await page.locator('.application-status').getByRole('button',{name:'Bundle membership',exact:true}).click()
  const picker=page.locator('.bundle-membership')
  await expect(picker.getByRole('button',{name:'Apply membership',exact:true})).toBeEnabled()
  for(const checkbox of await picker.getByRole('checkbox').all())await checkbox.check()
  await picker.getByRole('button',{name:'Apply membership',exact:true}).click()
  await expect(picker).toBeHidden()
}

export async function openBuildTool(page:Page,name='Plan volumes') {
  const preset=page.getByRole('dialog',{name:'Bundle presets'});if(await preset.isVisible())await preset.getByRole('button',{name:'Cancel',exact:true}).click();
  if(await page.locator('.bundle-structure').isVisible())await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click();
  await expect(page.locator('.build-workspace')).toBeVisible()
  await expect(page.locator('.build-workspace')).not.toHaveAttribute('data-screen','storage')
  await expect(page.locator('.section-enter-active')).toHaveCount(0)
  const activeDialog=page.locator('.build-tool-dialog[open]')
  if(await activeDialog.count())await activeDialog.getByRole('button',{name:'Close build tool'}).click()
  const settings=page.locator('.build-workspace')
  if(!await settings.getByRole('button',{name,exact:true}).isVisible()){
    const back=settings.getByRole('button',{name:'Build settings',exact:true})
    if(await back.isVisible())await back.click()
    else await settings.getByRole('button',{name:'Rebuild',exact:true}).click()
  }
  await settings.getByRole('button',{name,exact:true}).click()
}
export async function submitBundleBuild(page:Page){
 if(await page.locator('.bundle-structure').isVisible())await page.locator('.matter-rail').getByRole('button',{name:'Build bundle',exact:true}).click();
 const tool=page.locator('.build-tool-dialog[open]')
 if(await tool.count())await tool.getByRole('button',{name:'Close build tool'}).click()
 await page.locator('.build-workspace').getByRole('button',{name:'Build bundle',exact:true}).click()
}
export async function openOutputTools(page:Page){
 const details=page.locator('.build-extra-tools')
 if(await details.getAttribute('open')===null)await details.locator('summary').click()
}
