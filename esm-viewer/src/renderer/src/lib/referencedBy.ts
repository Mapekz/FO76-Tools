/** Fetch the records referencing `target` out to `depth` hops — shared by
 * the store's record load (initial load) and `ReferencedByPanel`'s depth
 * selector (re-fetch at a new depth), so both go through one call site. */

import { sel, type Fo76Api, type RefListResult } from '../../../shared/api-types'

export async function fetchReferencedBy(
  dbId: string,
  target: string,
  depth: number,
  api: Pick<Fo76Api, 'run'>,
): Promise<RefListResult> {
  return api.run(dbId, {
    op: 'referenced_by',
    sel: sel(target),
    limit: 0,
    depth,
    type_filter: null,
    paths: false,
    sort: 'formid',
  })
}
