import { ipcMain, dialog } from 'electron'
import { napi } from './addon'
import * as registry from './db-registry'
import { CH } from '../shared/api-types'

function wrap(fn: () => unknown) {
  try {
    return fn()
  } catch (e: unknown) {
    throw new Error(e instanceof Error ? e.message : String(e))
  }
}

export function registerIpc(): void {
  ipcMain.handle(CH.openFileDialog, async () => {
    const { canceled, filePaths } = await dialog.showOpenDialog({
      filters: [{ name: 'ESM Files', extensions: ['esm'] }],
      properties: ['openFile']
    })
    return canceled ? null : filePaths[0]
  })

  ipcMain.handle(CH.openFolderDialog, async () => {
    const { canceled, filePaths } = await dialog.showOpenDialog({
      properties: ['openDirectory']
    })
    return canceled ? null : filePaths[0]
  })

  ipcMain.handle(CH.openDatabase, async (_e, path: string) => {
    const db = await napi.EsmDatabase.openDatabase(path)
    const info = wrap(() => (db as Record<string, () => unknown>).fileInfo())
    const id = registry.add(db, path, info)
    return { id, path, info }
  })

  ipcMain.handle(CH.closeDatabase, (_e, id: string) => {
    registry.remove(id)
  })

  ipcMain.handle(CH.listOpen, () =>
    registry.listAll().map(({ id, path, info }) => ({ id, path, info }))
  )

  ipcMain.handle(CH.fileInfo, (_e, id: string) => {
    const entry = registry.get(id)
    if (!entry) throw new Error(`no database with id ${id}`)
    return wrap(() => (entry.db as Record<string, () => unknown>).fileInfo())
  })

  ipcMain.handle(CH.listGroups, (_e, id: string) => {
    const entry = registry.get(id)
    if (!entry) throw new Error(`no database with id ${id}`)
    return wrap(() => (entry.db as Record<string, () => unknown>).listGroups())
  })

  ipcMain.handle(CH.listTypeRecords, (_e, id: string, sig: string, offset: number, limit: number) => {
    const entry = registry.get(id)
    if (!entry) throw new Error(`no database with id ${id}`)
    return wrap(() => (entry.db as Record<string, (...args: unknown[]) => unknown>).listTypeRecords(sig, offset, limit))
  })

  ipcMain.handle(CH.recordByFormid, (_e, id: string, formid: string, resolve = 'stub') => {
    const entry = registry.get(id)
    if (!entry) throw new Error(`no database with id ${id}`)
    return wrap(() => (entry.db as Record<string, (...args: unknown[]) => unknown>).recordByFormid(formid, resolve))
  })

  ipcMain.handle(CH.recordByEdid, (_e, id: string, edid: string) => {
    const entry = registry.get(id)
    if (!entry) throw new Error(`no database with id ${id}`)
    return wrap(() => (entry.db as Record<string, (...args: unknown[]) => unknown>).recordByEdid(edid))
  })

  ipcMain.handle(CH.recordById, (_e, id: string, target: string, resolve = 'stub') => {
    const entry = registry.get(id)
    if (!entry) throw new Error(`no database with id ${id}`)
    return wrap(() => (entry.db as Record<string, (...args: unknown[]) => unknown>).recordById(target, resolve))
  })

  ipcMain.handle(CH.referencedBy, (_e, id: string, formid: string) => {
    const entry = registry.get(id)
    if (!entry) throw new Error(`no database with id ${id}`)
    return wrap(() => (entry.db as Record<string, (...args: unknown[]) => unknown>).referencedBy(formid))
  })

  ipcMain.handle(CH.referencedById, (_e, id: string, target: string, depth?: number) => {
    const entry = registry.get(id)
    if (!entry) throw new Error(`no database with id ${id}`)
    const clampedDepth = Math.max(1, Math.min(depth ?? 1, 6))
    return wrap(() =>
      (entry.db as Record<string, (...args: unknown[]) => unknown>).referencedById(
        target,
        clampedDepth,
      ),
    )
  })

  ipcMain.handle(CH.parseFormId, (_e, s: string) => {
    return wrap(() => (napi as Record<string, (...args: unknown[]) => unknown>).parseFormId(s))
  })
}
