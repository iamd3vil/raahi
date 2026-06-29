import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// Raahi admin UI: a Svelte 5 SPA built to ../ui/build and served by the admin API.
// In dev, proxy API + SSE to the running Raahi admin server.
export default defineConfig({
  plugins: [svelte()],
  build: {
    outDir: 'build',
    emptyOutDir: true,
  },
  server: {
    port: 5173,
    proxy: {
      '/api': { target: 'http://127.0.0.1:9080', changeOrigin: true },
      '/healthz': { target: 'http://127.0.0.1:9080', changeOrigin: true },
    },
  },
});
