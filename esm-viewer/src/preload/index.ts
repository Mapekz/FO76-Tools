import { contextBridge, ipcRenderer } from 'electron'
import { CH } from '../shared/api-types'
import type { Fo76Api } from '../shared/api-types'
import { CONTRACT } from '../shared/ipc-contract'

// Every `CONTRACT` entry is a pure pass-through: `(id, ...args) =>
// ipcRenderer.invoke(channel, id, ...args)` — no validation here, that's
// main-process-side (the trust boundary lives on the receiving end, not the
// sender). `TableMethod` is everything in `Fo76Api` NOT hand-written as a
// special case below, so adding/removing a method from either group surfaces
// as a real type error instead of silently drifting.
type SpecialMethod =
  | 'openFileDialog'
  | 'openFolderDialog'
  | 'openDatabase'
  | 'closeDatabase'
  | 'listOpen'
  | 'parseFormId'
  | 'diff'
type TableMethod = Exclude<keyof Fo76Api, SpecialMethod>

function buildTableForwards(): Pick<Fo76Api, TableMethod> {
  const forwards: Record<string, (...args: unknown[]) => Promise<unknown>> = {}
  for (const entry of CONTRACT) {
    forwards[entry.method] = (...args: unknown[]) => ipcRenderer.invoke(entry.channel, ...args)
  }
  // Cast unavoidable: the loop above builds one heterogeneous object from a
  // table whose entries each have a distinct arity/signature, so there's no
  // single call shape TS can check per-key here. The `const api: Fo76Api =
  // {...}` assembly below is what actually verifies this object's shape
  // structurally against `Fo76Api` (missing/mistyped keys still fail there).
  return forwards as unknown as Pick<Fo76Api, TableMethod>
}

const api: Fo76Api = {
  openFileDialog: () => ipcRenderer.invoke(CH.openFileDialog),
  openFolderDialog: () => ipcRenderer.invoke(CH.openFolderDialog),
  openDatabase: (path) => ipcRenderer.invoke(CH.openDatabase, path),
  closeDatabase: (id) => ipcRenderer.invoke(CH.closeDatabase, id),
  listOpen: () => ipcRenderer.invoke(CH.listOpen),
  parseFormId: (s) => ipcRenderer.invoke(CH.parseFormId, s),
  diff: (oldId, newId, recordType, bodies, suppressNoise, excludeTypes) =>
    ipcRenderer.invoke(CH.diff, oldId, newId, recordType, bodies, suppressNoise, excludeTypes),
  ...buildTableForwards(),
}

// Table forwards are unconditional pass-throughs; these three additionally
// default a missing/undefined `resolve` to `'stub'` before invoking, same as
// before this refactor — preserved here as a thin wrapper over the table
// entry rather than folded into the shared (main+preload) contract, since the
// default is preload-only sugar (main's own validators apply the identical
// default independently either way).
api.recordByFormid = (id, formid, resolve) =>
  ipcRenderer.invoke(CH.recordByFormid, id, formid, resolve ?? 'stub')
api.recordByEdid = (id, edid, resolve) =>
  ipcRenderer.invoke(CH.recordByEdid, id, edid, resolve ?? 'stub')
api.recordById = (id, target, resolve) =>
  ipcRenderer.invoke(CH.recordById, id, target, resolve ?? 'stub')

contextBridge.exposeInMainWorld('api', api)
