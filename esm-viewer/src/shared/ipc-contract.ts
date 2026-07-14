// Single source of truth for the ~14 uniform "registry-keyed DB method" IPC
// forwards: one entry per method, pairing its channel name, its `EsmDatabase`
// addon method name, and an argument validator. Drives:
//   - the table-derived slice of `CH` (`src/shared/api-types.ts`)
//   - the preload bridge's generic forward loop (`src/preload/index.ts`)
//   - the main-process generic handler loop (`src/main/ipc.ts`)
//
// Deliberately excludes:
//   - `openFileDialog` / `openFolderDialog` / `openDatabase` / `closeDatabase`
//     / `listOpen` / `parseFormId` — no registry-keyed `id` argument, or
//     bespoke lifecycle/dialog logic that doesn't fit "look up one db, call
//     one method on it".
//   - `diff` — needs TWO registry lookups (old/new), and the resolved "new"
//     `EsmDatabase` itself becomes an argument to the "old" one's `.diff()`
//     call. That's a fundamentally different shape than every other method
//     here (one id in, validated primitives out), so it stays hand-written
//     rather than being bent to fit this table.
//
// `Fo76Api` stays hand-written in `api-types.ts` (not derived from this
// table) so the compiler still checks the preload bridge's shape against it.

import type { EsmDatabase } from '@fo76/esm-napi'
import {
  validateResolve,
  validateSig,
  validateSigArray,
  validateSearchField,
  validateFilterOp,
  validateOptionalText,
  validateUint,
  validateTarget,
} from '../main/ipc-validators'

export interface ContractEntry {
  /** Electron IPC channel name — this table is the one place these strings live. */
  channel: string
  /** `EsmDatabase` method name; also the `Fo76Api`/preload-bridge property name. */
  method: keyof EsmDatabase
  /**
   * Validates the full args tuple (everything after the registry `id`), in
   * calling order, returning the exact argument list to spread into the addon
   * method call. Throws on invalid input — this is the IPC trust boundary.
   */
  validate: (args: unknown[]) => unknown[]
}

export const CONTRACT: readonly ContractEntry[] = [
  { channel: 'file-info', method: 'fileInfo', validate: () => [] },
  { channel: 'list-groups', method: 'listGroups', validate: () => [] },
  {
    channel: 'list-type-records',
    method: 'listTypeRecords',
    validate: ([sig, offset, limit]) => [
      validateSig(sig),
      validateUint('offset', offset),
      validateUint('limit', limit),
    ],
  },
  {
    channel: 'record-by-formid',
    method: 'recordByFormid',
    validate: ([formid, resolve = 'stub']) => [validateTarget(formid), validateResolve(resolve)],
  },
  {
    channel: 'record-by-edid',
    method: 'recordByEdid',
    validate: ([edid, resolve = 'stub']) => [validateTarget(edid), validateResolve(resolve)],
  },
  {
    channel: 'record-by-id',
    method: 'recordById',
    validate: ([target, resolve = 'stub']) => [validateTarget(target), validateResolve(resolve)],
  },
  {
    channel: 'referenced-by-id',
    method: 'referencedById',
    // `depth` is intentionally NOT run through `validateUint` here — this
    // mirrors the pre-existing (loose) behavior: a non-number `depth` clamps
    // to `NaN` via `Math.max`/`Math.min`, same as before this refactor.
    validate: ([target, depth]) => [
      validateTarget(target),
      Math.max(1, Math.min((depth as number) ?? 1, 6)),
    ],
  },
  {
    channel: 'list-type-children',
    method: 'listTypeChildren',
    validate: ([sig, offset, limit]) => [
      validateSig(sig),
      validateUint('offset', offset),
      validateUint('limit', limit),
    ],
  },
  {
    channel: 'list-group-children',
    method: 'listGroupChildren',
    validate: ([groupOffset, offset, limit]) => [
      validateUint('groupOffset', groupOffset, Number.MAX_SAFE_INTEGER),
      validateUint('offset', offset),
      validateUint('limit', limit),
    ],
  },
  {
    channel: 'search',
    method: 'search',
    validate: ([pattern, types, field, limit]) => [
      validateOptionalText('pattern', pattern, 512) ?? '',
      validateSigArray(types),
      validateSearchField(field),
      validateUint('limit', limit),
    ],
  },
  {
    channel: 'filter-type-records',
    method: 'filterTypeRecords',
    validate: ([sig, path, op, value, limit]) => [
      validateSig(sig),
      validateOptionalText('path', path, 1024),
      validateFilterOp(op),
      validateOptionalText('value', value, 1024),
      validateUint('limit', limit),
    ],
  },
  {
    channel: 'list-type-field-paths',
    method: 'listTypeFieldPaths',
    validate: ([sig]) => [validateSig(sig)],
  },
  {
    channel: 'record-raw',
    method: 'recordRaw',
    validate: ([target]) => [validateTarget(target)],
  },
  {
    channel: 'coverage-report',
    method: 'coverageReport',
    validate: ([recordType, sample]) => [
      validateOptionalText('recordType', recordType, 4),
      validateUint('sample', sample),
    ],
  },
]
