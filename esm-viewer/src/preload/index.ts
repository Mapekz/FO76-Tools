import { contextBridge, ipcRenderer } from 'electron'
import { CH } from '../shared/api-types'
import type { Fo76Api } from '../shared/api-types'

// Pure pass-throughs: validation happens in the main process, where the trust
// boundary is.
const api: Fo76Api = {
  openFileDialog: () => ipcRenderer.invoke(CH.openFileDialog),
  openFolderDialog: () => ipcRenderer.invoke(CH.openFolderDialog),
  openDatabase: (path) => ipcRenderer.invoke(CH.openDatabase, path),
  closeDatabase: (id) => ipcRenderer.invoke(CH.closeDatabase, id),
  listOpen: () => ipcRenderer.invoke(CH.listOpen),
  parseFormId: (s) => ipcRenderer.invoke(CH.parseFormId, s),
  run: (id, op) => ipcRenderer.invoke(CH.run, id, op),
  diff: (oldId, newId, request) => ipcRenderer.invoke(CH.diff, oldId, newId, request),
}

contextBridge.exposeInMainWorld('api', api)
