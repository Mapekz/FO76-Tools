/** Column-aligned decoded-field tree shared by `RecordTable` (xEdit-style
 * side-by-side record view) — builds one tree from N per-file `fields`
 * objects, keyed so identical paths land on the same row across columns. */

import { MARKERS } from '../../../shared/generated/markers.generated'

/** Sentinel for "this column's file has no value at this path" — distinct from
 * a real `null`/`undefined` field value, which columns can and do carry. */
export const MISSING = Symbol('missing')

export interface AlignedNode {
  /** Object key, or "[3]" for an array element. */
  label: string
  /** e.g. "Keywords.[3].formid" — collapse key and React key. */
  path: string
  isLeaf: boolean
  /** One per column, parallel to the caller's column list; may be `MISSING`. */
  values: unknown[]
  children: AlignedNode[]
  /** Leaf: values differ or presence mismatches. Branch: any descendant conflict. */
  conflict: boolean
  /** `coverageBadges()` per column — populated for object-kind nodes only. */
  badgesPerCol: string[][]
}

export function isFormIdStub(
  v: unknown
): v is { formid: string; editor_id?: string; record_type: string } {
  return typeof v === 'object' && v !== null && 'formid' in v && 'record_type' in v
}

function isNonEmptyObject(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v) && Object.keys(v).length > 0
}

/** Which coverage-gap badges (if any) apply directly to this object node
 * (not its descendants) — e.g. `{ "_raw": true, ... }`. */
export function coverageBadges(obj: Record<string, unknown>): string[] {
  const badges: string[] = []
  if (obj[MARKERS.UNKNOWN_RECORD] === true) badges.push('unknown record')
  if (obj[MARKERS.RAW] === true) badges.push('raw')
  if (isNonEmptyObject(obj[MARKERS.UNMAPPED])) badges.push('unmapped')
  if (obj[MARKERS.UNRESOLVED] === true) badges.push('unresolved')
  return badges
}

/** Recursively checks a decoded `fields` tree for any schema decode-coverage gap
 * marker (`_unknown_record`, `_raw`, `_unmapped`, `_unresolved`) — see
 * esm/CLAUDE.md "Decode output key conventions". Used to auto-default the
 * raw/decoded toggle and to drive inline coverage badges. */
export function hasCoverageMarkers(value: unknown): boolean {
  if (Array.isArray(value)) {
    return value.some((item) => hasCoverageMarkers(item))
  }
  if (typeof value === 'object' && value !== null) {
    const obj = value as Record<string, unknown>
    if (coverageBadges(obj).length > 0) return true
    return Object.values(obj).some((v) => hasCoverageMarkers(v))
  }
  return false
}

/** True only when the schema has no mapping for this record type at all
 * (top-level `_unknown_record`) — the byte dump is then the only useful view.
 * Unlike `hasCoverageMarkers`, this does NOT recurse: a record that's mostly
 * decoded but has a few nested `_raw`/`_unmapped` fields still counts as known. */
export function isUnknownRecordType(fields: unknown): boolean {
  return (
    typeof fields === 'object' &&
    fields !== null &&
    (fields as Record<string, unknown>)[MARKERS.UNKNOWN_RECORD] === true
  )
}

/** Structural deep-equal, except two FormID stubs compare by `formid` only —
 * `editor_id`/`record_type` come from each file's own resolution pass and must
 * not flag the *referencing* record as conflicting just because the target's
 * name differs (or is missing) in one of the open files. */
export function deepEqualForConflict(a: unknown, b: unknown): boolean {
  if (a === b) return true
  if (isFormIdStub(a) && isFormIdStub(b)) return a.formid === b.formid
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b)) return false
    if (a.length !== b.length) return false
    return a.every((v, i) => deepEqualForConflict(v, b[i]))
  }
  if (typeof a === 'object' && a !== null && typeof b === 'object' && b !== null) {
    const ao = a as Record<string, unknown>
    const bo = b as Record<string, unknown>
    const aKeys = Object.keys(ao)
    const bKeys = Object.keys(bo)
    if (aKeys.length !== bKeys.length) return false
    return aKeys.every((k) => k in bo && deepEqualForConflict(ao[k], bo[k]))
  }
  return false
}

type Kind = 'missing' | 'leaf' | 'object' | 'array'

/** MISSING values don't vote on kind. A FormID stub is object-shaped but is a
 * "leaf" kind alongside scalars — it's rendered as one clickable unit, not
 * descended into. */
function kindOf(v: unknown): Kind {
  if (v === MISSING) return 'missing'
  if (Array.isArray(v)) return 'array'
  if (typeof v === 'object' && v !== null) return isFormIdStub(v) ? 'leaf' : 'object'
  return 'leaf'
}

/** Key union across columns, preserving order: the first present column's
 * insertion order, then unseen keys from subsequent columns. */
function objectChildKeys(values: unknown[]): string[] {
  const seen = new Set<string>()
  const keys: string[] = []
  for (const v of values) {
    if (kindOf(v) !== 'object') continue
    for (const k of Object.keys(v as Record<string, unknown>)) {
      if (!seen.has(k)) {
        seen.add(k)
        keys.push(k)
      }
    }
  }
  return keys
}

function buildNode(label: string, path: string, values: unknown[]): AlignedNode {
  const kinds = values.map(kindOf)
  const presentKinds = new Set(kinds.filter((k): k is Exclude<Kind, 'missing'> => k !== 'missing'))
  const presenceMismatch = kinds.includes('missing') && presentKinds.size > 0

  if (presentKinds.size > 1) {
    // Mixed shape across columns (e.g. an object in one file's schema version,
    // a scalar in another's) — no sensible alignment. Force a leaf; ValueCell
    // falls back to a truncated JSON summary per cell.
    return {
      label,
      path,
      isLeaf: true,
      values,
      children: [],
      conflict: true,
      badgesPerCol: values.map(() => []),
    }
  }

  // presentKinds.size === 0 only when every column is MISSING, which can't
  // actually happen — a node is only built for a key some column had.
  const kind = presentKinds.size === 1 ? [...presentKinds][0] : 'leaf'

  if (kind === 'object') {
    const badgesPerCol = values.map((v) =>
      kindOf(v) === 'object' ? coverageBadges(v as Record<string, unknown>) : []
    )
    const keys = objectChildKeys(values)
    const children = keys.map((k) => {
      const childValues = values.map((v) => {
        if (kindOf(v) !== 'object') return MISSING
        const obj = v as Record<string, unknown>
        return k in obj ? obj[k] : MISSING
      })
      return buildNode(k, `${path}.${k}`, childValues)
    })
    const conflict = presenceMismatch || children.some((c) => c.conflict)
    return { label, path, isLeaf: false, values, children, conflict, badgesPerCol }
  }

  if (kind === 'array') {
    let maxLen = 0
    for (const v of values) {
      if (Array.isArray(v)) maxLen = Math.max(maxLen, v.length)
    }
    const children: AlignedNode[] = []
    for (let i = 0; i < maxLen; i++) {
      const childValues = values.map((v) => (Array.isArray(v) && i < v.length ? v[i] : MISSING))
      children.push(buildNode(`[${i}]`, `${path}.[${i}]`, childValues))
    }
    const conflict = presenceMismatch || children.some((c) => c.conflict)
    return {
      label,
      path,
      isLeaf: false,
      values,
      children,
      conflict,
      badgesPerCol: values.map(() => []),
    }
  }

  // Leaf: scalars and/or FormID stubs.
  const present = values.filter((v) => v !== MISSING)
  const allEqual = present.length <= 1 || present.every((v) => deepEqualForConflict(v, present[0]))
  return {
    label,
    path,
    isLeaf: true,
    values,
    children: [],
    conflict: presenceMismatch || !allEqual,
    badgesPerCol: values.map(() => []),
  }
}

/** Build the aligned row tree for `RecordTable` from one decoded `fields`
 * object per column (`null` = the FormID isn't present in that file). */
export function buildAlignedTree(fieldsByCol: (Record<string, unknown> | null)[]): AlignedNode[] {
  const values: unknown[] = fieldsByCol.map((f) => (f === null ? MISSING : f))
  const keys = objectChildKeys(values)
  return keys.map((k) => {
    const childValues = values.map((v) => {
      if (kindOf(v) !== 'object') return MISSING
      const obj = v as Record<string, unknown>
      return k in obj ? obj[k] : MISSING
    })
    return buildNode(k, k, childValues)
  })
}
