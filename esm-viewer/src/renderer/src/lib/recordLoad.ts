/** Pure record-loading algorithms for `RecordTree` — the flat chunked
 * auto-load-all for a record-type group, and the paged fetch used by the
 * WRLD/CELL hierarchical descent. No React, no Zustand, no `window.api`:
 * callers pass in the subset of `Fo76Api` actually needed, so this can be
 * exercised with a fake `api` in unit tests (mirrors `recordColumns.ts`). */

import type { Fo76Api, GroupChild, RecordRow } from '../../../shared/api-types'

/** Auto-loads every record of `sig` in bounded chunks so no single IPC
 * round-trip blocks Electron's main process for too long. Invokes `onChunk`
 * with the accumulated array after every successful chunk fetch, so the
 * caller can show partial progress as it streams in (e.g. via `setRows`).
 * Stops once `offset >= total`, or defensively on an empty chunk (avoids an
 * infinite loop on a short backend response). */
export async function loadAllTypeRecords(
  api: Pick<Fo76Api, 'listTypeRecords'>,
  dbId: string,
  sig: string,
  total: number,
  chunkSize: number,
  onChunk: (accumulated: RecordRow[]) => void
): Promise<void> {
  let offset = 0
  let acc: RecordRow[] = []
  while (offset < total) {
    const chunk = await api.listTypeRecords(dbId, sig, offset, chunkSize)
    if (chunk.length === 0) break // defensive: avoid an infinite loop on a short backend response
    acc = acc.concat(chunk)
    offset += chunk.length
    onChunk(acc)
  }
}

/** Fetch the next page of a WRLD/CELL group's top-level children (keyed by
 * record-type `sig`), appending to whatever page(s) the caller already has.
 * Pass `current = []` for the initial expand and the group's own accumulated
 * list for a subsequent "Load more…". */
export async function loadTypeChildrenPage(
  api: Pick<Fo76Api, 'listTypeChildren'>,
  dbId: string,
  sig: string,
  current: GroupChild[],
  pageSize: number
): Promise<GroupChild[]> {
  const next = await api.listTypeChildren(dbId, sig, current.length, pageSize)
  return [...current, ...next]
}

/** Fetch the next page of a nested group's children (keyed by the group's
 * byte offset within the file), appending to whatever page(s) the caller
 * already has. Pass `current = []` for a `GroupChildNode`'s initial expand
 * and its accumulated list for a subsequent "Load more…". */
export async function loadGroupChildrenPage(
  api: Pick<Fo76Api, 'listGroupChildren'>,
  dbId: string,
  groupOffset: number,
  current: GroupChild[],
  pageSize: number
): Promise<GroupChild[]> {
  const next = await api.listGroupChildren(dbId, groupOffset, current.length, pageSize)
  return [...current, ...next]
}
