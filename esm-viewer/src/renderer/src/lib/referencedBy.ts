/** Thin, testable wrapper around `Fo76Api.referencedById` — shared by
 * `App.tsx`'s `loadRecord` (initial load) and `ReferencedByPanel`'s depth
 * selector (re-fetch at a new depth), so both go through one call site. */

import type { Fo76Api, RefListResult } from '../../../shared/api-types'

export async function fetchReferencedBy(
  dbId: string,
  target: string,
  depth: number,
  api: Pick<Fo76Api, 'referencedById'>
): Promise<RefListResult> {
  return api.referencedById(dbId, target, depth)
}
