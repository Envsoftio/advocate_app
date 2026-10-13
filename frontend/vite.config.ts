import { defineConfig } from 'vitest/config'
import vue from '@vitejs/plugin-vue'

export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: { strictPort: true },
  test: { environment: 'node', exclude: ['e2e/**', 'node_modules/**'] },
})
