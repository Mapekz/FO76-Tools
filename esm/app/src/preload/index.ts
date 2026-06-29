import { contextBridge, ipcRenderer } from 'electron'
import { CH } from '../shared/api-types'
import type { Fo76Api } from '../shared/api-types'

const api: Fo76Api = {
  openFileDialog: () => ipcRenderer.invoke(CH.openFileDialog),
  openFolderDialog: () => ipcRenderer.invoke(CH.openFolderDialog),
  openDatabase: (path) => ipcRenderer.invoke(CH.openDatabase, path),
  closeDatabase: (id) => ipcRenderer.invoke(CH.closeDatabase, id),
  listOpen: () => ipcRenderer.invoke(CH.listOpen),
  fileInfo: (id) => ipcRenderer.invoke(CH.fileInfo, id),
  listGroups: (id) => ipcRenderer.invoke(CH.listGroups, id),
  listTypeRecords: (id, sig, offset, limit) =>
    ipcRenderer.invoke(CH.listTypeRecords, id, sig, offset, limit),
  recordByFormid: (id, formid, resolve) =>
    ipcRenderer.invoke(CH.recordByFormid, id, formid, resolve ?? 'stub'),
  recordByEdid: (id, edid) => ipcRenderer.invoke(CH.recordByEdid, id, edid),
  recordById: (id, target, resolve) =>
    ipcRenderer.invoke(CH.recordById, id, target, resolve ?? 'stub'),
  referencedBy: (id, formid) => ipcRenderer.invoke(CH.referencedBy, id, formid),
  referencedById: (id, target) => ipcRenderer.invoke(CH.referencedById, id, target),
  parseFormId: (s) => ipcRenderer.invoke(CH.parseFormId, s),
}

contextBridge.exposeInMainWorld('api', api)
