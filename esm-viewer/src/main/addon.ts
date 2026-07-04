import { createRequire } from 'module'
import type { EsmDatabase } from '@fo76/esm-napi'
const require = createRequire(import.meta.url)
const napi = require('@fo76/esm-napi') as {
  EsmDatabase: typeof EsmDatabase
  parseFormId: (s: string) => string
}
export { napi }
export type { EsmDatabase }
