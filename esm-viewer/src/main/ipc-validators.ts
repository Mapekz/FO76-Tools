// Validation at the Electron main-process trust boundary: renderer input is
// checked here before it reaches the native addon. The engine parses every op
// strictly (unknown ops, missing fields and wrong types are errors), so this
// file adds only what the engine would accept but the viewer must not send:
// ops outside the viewer's set, unbounded limits, and unbounded refs walks.
// Free of Electron imports so it is unit-testable directly.

import type { DiffRequest, RunnableOp } from '../shared/api-types'

/** The ops the renderer may run. Adding an op to the engine fails the
 * `satisfies` check below until it is listed here or deliberately left out. */
const RUNNABLE_OPS = [
  'file_info',
  'record',
  'record_bulk',
  'record_raw',
  'list_type_records',
  'filter_type_records',
  'list_type_field_paths',
  'search',
  'referenced_by',
  'ref_path',
  'walk',
  'chase',
  'drop_table',
  'list_groups',
  'list_type_children',
  'list_group_children',
  'record_stub_at',
  'coverage',
] as const satisfies readonly RunnableOp['op'][]

type Unlisted = Exclude<RunnableOp['op'], (typeof RUNNABLE_OPS)[number]>
const everyOpListed: [Unlisted] extends [never] ? true : Unlisted = true
void everyOpListed

const RUNNABLE = new Set<string>(RUNNABLE_OPS)

export function validateUint(name: string, v: unknown, max = 100_000): number {
  if (typeof v === 'number' && Number.isInteger(v) && v >= 0 && v <= max) return v
  throw new Error(`invalid ${name}: expected integer 0–${max}, got ${String(v)}`)
}

/**
 * Referenced-by walk depth in hops. Absent means 1; integers clamp to
 * `1..=max`. Anything else is rejected, because the engine reads a depth of 0
 * as an unbounded walk.
 */
export function validateRefDepth(v: unknown, max = 6): number {
  if (v === undefined || v === null) return 1
  if (typeof v === 'number' && Number.isInteger(v)) return Math.max(1, Math.min(v, max))
  throw new Error(`invalid depth: expected an integer, got ${String(v)}`)
}

function plainObject(name: string, v: unknown): Record<string, unknown> {
  if (typeof v === 'object' && v !== null && !Array.isArray(v)) {
    return v as Record<string, unknown>
  }
  throw new Error(`invalid ${name}: expected an object`)
}

/** An op the renderer may run, with its limit and refs depth bounded. */
export function validateOp(v: unknown): RunnableOp {
  const op = { ...plainObject('op', v) }
  if (typeof op.op !== 'string' || !RUNNABLE.has(op.op)) {
    throw new Error(`invalid op: ${String(op.op)}`)
  }
  if ('limit' in op) op.limit = validateUint('limit', op.limit)
  if (op.op === 'referenced_by') op.depth = validateRefDepth(op.depth)
  return op as RunnableOp
}

export function validateDiffRequest(v: unknown): DiffRequest {
  const request = plainObject('diff request', v)
  const recordType = request.record_type ?? null
  if (recordType !== null && (typeof recordType !== 'string' || recordType.length > 4)) {
    throw new Error(`invalid record_type: ${String(recordType)}`)
  }
  return { record_type: recordType, options: plainObject('diff options', request.options ?? {}) }
}
