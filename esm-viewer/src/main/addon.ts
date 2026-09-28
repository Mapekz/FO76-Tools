import { createRequire } from 'module'
import type { FileInfo, OpResult, RunnableOp } from '../shared/api-types'
import type { Op } from '../shared/generated/Op'

/** The native `EsmHost` with its JSON results typed by the generated
 * `Op`/`OpOutput` contract instead of `unknown`. */
export interface EsmHost {
  open(path: string): Promise<FileInfo>
  run<T extends Op>(esm: string, op: T): Promise<OpResult<T>>
  close(esm: string): void
}

interface Addon {
  EsmHost: new () => EsmHost
  parseFormId(s: string): string
}

const require = createRequire(import.meta.url)
const addon = require('@fo76/esm-napi') as Addon

export function createHost(): EsmHost {
  return new addon.EsmHost()
}

export const parseFormId = addon.parseFormId

export type { RunnableOp }
