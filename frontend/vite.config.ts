import { defineConfig } from 'vitest/config'
import vue from '@vitejs/plugin-vue'

export default defineConfig({
  plugins: [vue()],
  worker: {
    format: 'es',
    // Vite defaults worker entry signatures to false after applying rollupOptions.
    // PDF.js also imports this entry as a module for its main-thread fallback,
    // so retain WorkerMessageHandler through an options hook instead.
    plugins: () => [{
      name: 'preserve-pdf-worker-fallback-export',
      options: options => ({ ...options, preserveEntrySignatures: 'strict' }),
    }],
  },
  clearScreen: false,
  server: { strictPort: true },
  test: { environment: 'node', exclude: ['e2e/**', 'node_modules/**'] },
})
