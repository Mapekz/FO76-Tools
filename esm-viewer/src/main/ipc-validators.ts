// Pure argument validators for the Electron main-process IPC trust boundary —
// the last line of defense before untyped renderer input reaches the native
// `EsmDatabase` addon. Deliberately free of any Electron import so that:
//   - they're unit-testable directly under vitest's `node` environment
//     (importing `electron` outside the Electron runtime is unreliable), and
//   - `../shared/ipc-contract.ts` can import them without pulling Electron
//     into shared code.
// `src/main/ipc.ts` re-exports all of these (`export * from './ipc-validators'`)
// so existing call sites and tests can import from either module.

export function validateResolve(v: unknown): 'none' | 'stub' | 'full' {
  if (v === 'none' || v === 'stub' || v === 'full') return v
  throw new Error(`invalid resolve value: expected none|stub|full, got ${String(v)}`)
}

export function validateSig(v: unknown): string {
  if (typeof v === 'string' && /^[A-Z_0-9]{1,4}$/.test(v)) return v
  throw new Error(`invalid record signature: ${String(v)}`)
}

export function validateSigArray(v: unknown): string[] {
  if (!Array.isArray(v)) throw new Error(`invalid record signature list: ${String(v)}`)
  return v.map((sig) => validateSig(sig))
}

export function validateSearchField(v: unknown): 'edid' | 'name' | 'both' {
  if (v === 'edid' || v === 'name' || v === 'both') return v
  throw new Error(`invalid search field: expected edid|name|both, got ${String(v)}`)
}

export function validateBodies(v: unknown): 'none' | 'stub' | 'full' {
  if (v === 'none' || v === 'stub' || v === 'full') return v
  throw new Error(`invalid bodies value: expected none|stub|full, got ${String(v)}`)
}

export function validateFilterOp(v: unknown): 'exists' | 'eq' | 'contains' | 'gt' | 'lt' | 'gte' | 'lte' {
  if (v === 'exists' || v === 'eq' || v === 'contains' || v === 'gt' || v === 'lt' || v === 'gte' || v === 'lte') {
    return v
  }
  throw new Error(`invalid filter op: expected exists|eq|contains|gt|lt|gte|lte, got ${String(v)}`)
}

export function validateOptionalText(name: string, v: unknown, max = 512): string | undefined {
  if (v === undefined || v === null) return undefined
  if (typeof v === 'string' && v.length <= max) return v
  throw new Error(`invalid ${name}: must be a string of length <= ${max}`)
}

export function validateUint(name: string, v: unknown, max = 100_000): number {
  if (typeof v === 'number' && Number.isInteger(v) && v >= 0 && v <= max) return v
  throw new Error(`invalid ${name}: expected integer 0–${max}, got ${String(v)}`)
}

export function validateTarget(v: unknown): string {
  if (typeof v === 'string' && v.length > 0 && v.length <= 512) return v
  throw new Error(`invalid target: must be a non-empty string`)
}
