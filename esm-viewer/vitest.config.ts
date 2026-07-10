// Separate from `electron.vite.config.ts` (which configures electron-vite's
// three build targets, not a plain Vite/Vitest project) — this project has no
// DOM-mounting tests, so the default `node` environment is enough.
import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['src/**/*.test.ts'],
  },
})
