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
  recordByEdid: (id, edid, resolve) =>
    ipcRenderer.invoke(CH.recordByEdid, id, edid, resolve ?? 'stub'),
  recordById: (id, target, resolve) =>
    ipcRenderer.invoke(CH.recordById, id, target, resolve ?? 'stub'),
  referencedBy: (id, formid) => ipcRenderer.invoke(CH.referencedBy, id, formid),
  referencedById: (id, target, depth) =>
    ipcRenderer.invoke(CH.referencedById, id, target, depth),
  parseFormId: (s) => ipcRenderer.invoke(CH.parseFormId, s),
  listTypeChildren: (id, sig, offset, limit) =>
    ipcRenderer.invoke(CH.listTypeChildren, id, sig, offset, limit),
  listGroupChildren: (id, groupOffset, offset, limit) =>
    ipcRenderer.invoke(CH.listGroupChildren, id, groupOffset, offset, limit),
  search: (id, pattern, types, field, limit) =>
    ipcRenderer.invoke(CH.search, id, pattern, types, field, limit),
  filterTypeRecords: (id, sig, path, op, value, limit) =>
    ipcRenderer.invoke(CH.filterTypeRecords, id, sig, path, op, value, limit),
  listTypeFieldPaths: (id, sig) => ipcRenderer.invoke(CH.listTypeFieldPaths, id, sig),
}

contextBridge.exposeInMainWorld('api', api)
