import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { fileURLToPath } from 'node:url';

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  root: fileURLToPath(new URL('./ui', import.meta.url)),
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 5174 } : undefined,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: {
    outDir: fileURLToPath(new URL('./dist', import.meta.url)),
    emptyOutDir: true,
    // WebView2 (Chromium) on Windows, WKWebView on macOS 10.15+ (Safari 13+).
    target: ['es2020', 'safari13', 'chrome105'],
    minify: 'esbuild',
    cssMinify: true,
    sourcemap: false,
    reportCompressedSize: true,
  },
});
