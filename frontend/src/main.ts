import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import { installNativeFrameFallback } from './lib/nativeAnimationFrames'
import './design-tokens.css'
import './styles.css'
import './reader-design.css'
import './documents-design.css'
import './membership-design.css'
import './build-design.css'
import './validation-design.css'
import './structure-design.css'

import './supplement-design.css'

if ('__TAURI_INTERNALS__' in window && /Macintosh/.test(navigator.userAgent)) installNativeFrameFallback(window)
createApp(App).use(createPinia()).mount('#app')

import "./metadata-design.css"

import './output-options.css'

import './volume-planning.css'
