/** Pure logic for building one xEdit-style value column per open file that
 * contains a given record — shared by `App.tsx`'s `loadRecord` and its tests.
 * No React, no Zustand: callers pass in `openDbs` and the subset of `Fo76Api`
 * actually needed, so this can be exercised with a fake `api` in unit tests. */

import type { DbHandle, Fo76Api, RecordResult } from '../../../shared/api-types'
import type { RecordColumn } from '../store'

function basename(path: string): string {
  return path.split(/[\\/]/).pop() ?? path
}

/** Column-header labels for a set of open files. Plain basename normally, but
 * when several open files share one (the common case here: comparing dated
 * versions of SeventySix.esm), prefix the parent directory so the columns are
 * tellable apart — e.g. "20260619/SeventySix.esm" vs "20260702/SeventySix.esm". */
export function columnLabels(paths: string[]): Map<string, string> {
  const counts = new Map<string, number>()
  for (const p of paths) {
    const b = basename(p)
    counts.set(b, (counts.get(b) ?? 0) + 1)
  }
  const labels = new Map<string, string>()
  for (const p of paths) {
    const parts = p.split(/[\\/]/)
    const b = parts.pop() ?? p
    const dir = parts.pop()
    labels.set(p, (counts.get(b) ?? 0) > 1 && dir ? `${dir}/${b}` : b)
  }
  return labels
}

/** Resolve `target` (FormID or EditorID) in `dbId`, then fan out that canonical
 * FormID to every other open DB to build one column per file that has it.
 * A DB that rejects (the FormID isn't present there) simply drops its column. */
export async function buildRecordColumns(
  target: string,
  dbId: string,
  openDbs: DbHandle[],
  api: Pick<Fo76Api, 'recordById'>
): Promise<{ active: RecordResult; columns: RecordColumn[] }> {
  const rec = await api.recordById(dbId, target, 'stub')
  const formId = rec.header.form_id

  const others = openDbs.filter((db) => db.id !== dbId)
  const settled = await Promise.allSettled(
    others.map((db) => api.recordById(db.id, formId, 'stub'))
  )

  // recordById rejects when the FormID is absent from that file — that
  // rejection IS the "not in this file" signal, so the column is dropped.
  const otherResults = new Map<string, PromiseSettledResult<RecordResult>>()
  others.forEach((db, i) => otherResults.set(db.id, settled[i]))

  const labels = columnLabels(openDbs.map((db) => db.path))
  const columns: RecordColumn[] = []
  for (const db of openDbs) {
    if (db.id === dbId) {
      columns.push({ dbId: db.id, fileName: labels.get(db.path) ?? basename(db.path), record: rec })
      continue
    }
    const result = otherResults.get(db.id)
    if (result?.status === 'fulfilled') {
      columns.push({
        dbId: db.id,
        fileName: labels.get(db.path) ?? basename(db.path),
        record: result.value,
      })
    }
  }

  return { active: rec, columns }
}
