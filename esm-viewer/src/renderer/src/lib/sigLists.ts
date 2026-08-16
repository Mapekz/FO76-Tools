/** Pure helpers for building record-type-signature lists — the free-text
 * comma-separated input parser shared by `SearchPanel` and `DiffPanel`, and
 * the "record_type groups with at least one child" listing shared by
 * `FilterPanel` and `CoveragePanel`. No React, no Zustand: callers pass in
 * the subset of `Fo76Api` actually needed, so this can be exercised with a
 * fake `api` in unit tests (mirrors `recordLoad.ts`). */

import type { Fo76Api } from '../../../shared/api-types'

/** Parses a free-text comma-separated list of record-type signatures (e.g.
 * "weap, armo ,,ammo") into normalized, deduplicated-by-nothing uppercase
 * signatures, dropping blank entries. */
export function parseSigList(text: string): string[] {
  return text
    .split(',')
    .map((s) => s.trim().toUpperCase())
    .filter((s) => s.length > 0)
}

/** Lists every record-type signature with at least one record in `dbId`,
 * sorted alphabetically. Filters `listGroups`' output down to `record_type`
 * groups with a non-zero `child_count` — RecordTree.tsx needs every group
 * kind for its own tree, so that call site keeps its own inline variant. */
export async function listRecordTypeSigs(
  api: Pick<Fo76Api, 'listGroups'>,
  dbId: string,
): Promise<string[]> {
  const groups = await api.listGroups(dbId)
  return groups
    .filter((g) => g.label.kind === 'record_type' && g.child_count > 0)
    .map((g) => (g.label.kind === 'record_type' ? g.label.sig : ''))
    .filter((s) => s.length > 0)
    .toSorted()
}
