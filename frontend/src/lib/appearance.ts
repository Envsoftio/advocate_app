export type Theme = 'system' | 'light' | 'dark'
export type UiScale = 90 | 100 | 110 | 125

const key = 'courtbundle.appearance.v1'
export const defaultAppearance: { theme: Theme, scale: UiScale } = { theme: 'system', scale: 100 }
const systemDark = window.matchMedia('(prefers-color-scheme: dark)')
function syncEffectiveTheme() {
  const choice = document.documentElement.dataset.theme
  document.documentElement.dataset.effectiveTheme = choice === 'dark' || (choice === 'system' && systemDark.matches) ? 'dark' : 'light'
}
systemDark.addEventListener('change', syncEffectiveTheme)

export function readAppearance(): { theme: Theme, scale: UiScale } {
  try {
    const value = JSON.parse(localStorage.getItem(key) || '{}')
    return {
      theme: ['system', 'light', 'dark'].includes(value.theme) ? value.theme : 'system',
      scale: [90, 100, 110, 125].includes(value.scale) ? value.scale : 100,
    }
  } catch { return { ...defaultAppearance } }
}

export function applyAppearance(theme: Theme, scale: UiScale) {
  document.documentElement.dataset.theme = theme
  syncEffectiveTheme()
  document.documentElement.style.setProperty('--ui-scale', String(scale / 100))
  document.documentElement.style.zoom = `${scale}%`
  try { localStorage.setItem(key, JSON.stringify({ theme, scale })) } catch { /* private storage */ }
}
