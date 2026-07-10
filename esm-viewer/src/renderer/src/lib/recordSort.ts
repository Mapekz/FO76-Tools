/** Pure sort logic for the flat per-record-type table in `RecordTree.tsx`.
 * No React: callers own the sort state and call `sortRows` to derive the
 * displayed order. */

import type { RecordRow } from '../../../shared/api-types'

export type SortColumn = 'form_id' | 'editor_id' | 'name'
export type SortDirection = 'asc' | 'desc'

export interface SortState {
  column: SortColumn
  direction: SortDirection
}

/** `form_id` is a fixed-width hex string (e.g. "0x0000463F") so plain string
 * comparison matches numeric order. `editor_id`/`name` are nullable — null or
 * empty-string values sort to the end regardless of direction. */
export function compareRows(a: RecordRow, b: RecordRow, sort: SortState): number {
  const av = a[sort.column]
  const bv = b[sort.column]
  const aEmpty = av === null || av === ''
  const bEmpty = bv === null || bv === ''
  if (aEmpty && bEmpty) return 0
  if (aEmpty) return 1
  if (bEmpty) return -1
  const cmp = av! < bv! ? -1 : av! > bv! ? 1 : 0
  return sort.direction === 'asc' ? cmp : -cmp
}

export function sortRows(rows: RecordRow[], sort: SortState): RecordRow[] {
  return [...rows].sort((a, b) => compareRows(a, b, sort))
}
