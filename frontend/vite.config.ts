import { fileURLToPath, URL } from 'node:url'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vitest/config'

// `BACKEND_URL=http://127.0.0.1:8799 npm run dev` apunta el proxy a otro backend (pruebas).
const BACKEND = process.env.BACKEND_URL ?? 'http://127.0.0.1:8787'

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
  },
  server: {
    port: 5180,
    strictPort: true,
    proxy: {
      // `ws: true` cubre /api/ws; el SSE del chat pasa tal cual por el mismo proxy.
      '/api': { target: BACKEND, changeOrigin: false, ws: true },
    },
  },
  preview: {
    port: 5180,
    proxy: { '/api': { target: BACKEND, ws: true } },
  },
  build: {
    target: 'es2023',
    sourcemap: false,
    chunkSizeWarningLimit: 700,
  },
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
    css: false,
    include: ['src/**/*.test.{ts,tsx}'],
  },
})
