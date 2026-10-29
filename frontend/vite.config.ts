import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// Multi-page build: the public landing (/) and the operator console (/console/). Output goes straight to
// build/www; `npm run build` then writes .br/.gz next to every compressible file so nginx serves them
// with brotli_static/gzip_static and never compresses on the fly (docs/09-deployment.md).
// Backend address for `npm run dev` / `vite preview` (tools/e2e_console.sh uses another port).
const backend = process.env.AEROLINK_BACKEND ?? '127.0.0.1:8080';

export default defineConfig({
  plugins: [svelte()],
  build: {
    outDir: '../build/www',
    emptyOutDir: true,
    target: 'es2022',
    sourcemap: false,
    modulePreload: { polyfill: false },
    rollupOptions: {
      input: {
        landing: 'index.html',
        console: 'console/index.html',
      },
    },
  },
  server: {
    // Development: backend on 127.0.0.1:8080 with AEROLINK_SECURE_COOKIES=false and
    // AEROLINK_ORIGIN=http://localhost:5173.
    proxy: {
      '/api': `http://${backend}`,
      '/ws': { target: `ws://${backend}`, ws: true },
    },
  },
});
