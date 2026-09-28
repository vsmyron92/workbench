/// <reference types="vitest/config" />
import { fileURLToPath } from 'node:url'
import react from '@vitejs/plugin-react'
import { defineConfig, type Plugin, type ProxyOptions } from 'vite'

/**
 * three's DRACOLoader names its WebAssembly decoders by default
 * (`new URL('../libs/draco/…', import.meta.url)`), so Vite would ship ~600 kB of
 * decoders nothing loads: the Workspace 3D viewer uses the asm.js decoder, which it
 * names itself, because the app's CSP allows no WebAssembly.
 */
function dropDracoDefaults(): Plugin {
  const DEFAULT_URL = /new URL\(\s*(['"])\.\.\/libs\/draco\/[^'"]+\1\s*,\s*import\.meta\.url\s*\)\.toString\(\)/g
  return {
    name: 'workbench:drop-draco-defaults',
    enforce: 'pre',
    transform(code, id) {
      if (!id.split('?')[0].endsWith('/three/examples/jsm/loaders/DRACOLoader.js')) return null
      const out = code.replace(DEFAULT_URL, "''")
      if (out === code) this.warn('DRACOLoader default decoder URLs not found: WebAssembly decoders may be bundled')
      return { code: out, map: null }
    },
  }
}

// The Rust server serves the built app itself; this proxy is for `npm run dev` only.
// changeOrigin stays false so the backend sees the browser's Host (its Host/Origin checks accept loopback).
const BACKEND = process.env.WORKBENCH_BACKEND ?? 'http://127.0.0.1:7777'
const proxy: Record<string, ProxyOptions> = {
  '/api': { target: BACKEND, ws: true, changeOrigin: false },
  '/auth': { target: BACKEND, changeOrigin: false },
  '/pair': { target: BACKEND, changeOrigin: false },
  '/mcp': { target: BACKEND, changeOrigin: false },
  // Workspace card files (sandboxed reports, thumbnails, PDFs): served by the backend only.
  '/view': { target: BACKEND, changeOrigin: false },
}

export default defineConfig({
  plugins: [react(), dropDracoDefaults()],
  resolve: { alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) } },
  server: { host: '127.0.0.1', port: Number(process.env.WORKBENCH_VITE_PORT ?? 5173), strictPort: true, proxy },
  preview: { host: '127.0.0.1', port: 4173, strictPort: true, proxy },
  worker: { format: 'es' },
  build: {
    target: 'es2022',
    chunkSizeWarningLimit: 8000,
    sourcemap: false,
  },
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
  },
})
