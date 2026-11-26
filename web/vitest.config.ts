import { defineConfig } from 'vitest/config'

export default defineConfig({
  define: { 'import.meta.env.VITE_CONVEX_URL': JSON.stringify('https://fixture.convex.cloud') },
  test: { environment: 'jsdom' },
})
