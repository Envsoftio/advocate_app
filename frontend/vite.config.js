import { readdirSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { defineConfig } from 'vitest/config';
import vue from '@vitejs/plugin-vue';

/** @returns {import('vite').Plugin} */
function pdfAssets() {
  const require = createRequire(import.meta.url);
  const root = dirname(require.resolve('pdfjs-dist/package.json'));
  const files = ['LICENSE'];
  for (const folder of ['cmaps', 'standard_fonts']) {
    for (const name of readdirSync(join(root, folder))) files.push(`${folder}/${name}`);
  }
  return {
    name: 'package-pdfjs-assets',
    configureServer(server) {
      const filesByUrl = new Map(files.map((file) => [`/pdfjs/${file}`, join(root, file)]));
      server.middlewares.use((request, response, next) => {
        const file = filesByUrl.get(request.url?.split('?')[0] ?? '');
        if (!file) return next();
        response.setHeader('Content-Type', 'application/octet-stream');
        response.end(readFileSync(file));
      });
    },
    generateBundle() {
      for (const file of files) {
        this.emitFile({
          type: 'asset',
          fileName: `pdfjs/${file}`,
          source: readFileSync(join(root, file)),
        });
      }
    },
  };
}

export default defineConfig({
  plugins: [vue(), pdfAssets()],
  worker: {
    format: 'es',
    // Vite defaults worker entry signatures to false after applying rollupOptions.
    // PDF.js also imports this entry as a module for its main-thread fallback,
    // so retain WorkerMessageHandler through an options hook instead.
    plugins: () => [
      {
        name: 'preserve-pdf-worker-fallback-export',
        options: (options) => ({ ...options, preserveEntrySignatures: 'strict' }),
      },
    ],
  },
  clearScreen: false,
  server: { strictPort: true },
  test: {
    environment: 'node',
    exclude: ['e2e/**', 'node_modules/**', 'dist/**', 'src-tauri/target/**'],
  },
});
