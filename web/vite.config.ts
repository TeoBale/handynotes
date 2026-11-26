import { fileURLToPath } from 'node:url'
import react from '@vitejs/plugin-react'
import { defineConfig, loadEnv, searchForWorkspaceRoot } from 'vite'

const webRoot = fileURLToPath(new URL('.', import.meta.url))
const repoRoot = fileURLToPath(new URL('..', import.meta.url))

export default defineConfig(({ mode }) => {
  const webEnv = loadEnv(mode, webRoot)
  const repoEnv = loadEnv(mode, repoRoot, 'CONVEX_URL')

  return {
    plugins: [react()],
    define: {
      'import.meta.env.VITE_CONVEX_URL': JSON.stringify(webEnv.VITE_CONVEX_URL || repoEnv.CONVEX_URL || ''),
    },
    resolve: { dedupe: ['convex'] },
    server: {
      fs: {
        allow: [searchForWorkspaceRoot(webRoot), fileURLToPath(new URL('../convex/_generated', import.meta.url))],
      },
    },
  }
})
