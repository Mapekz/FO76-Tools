import { ipcMain, dialog } from 'electron'
import { napi } from './addon'
import * as registry from './db-registry'
import { CH } from '../shared/api-types'

function validateResolve(v: unknown): 'none' | 'stub' | 'full' {
  if (v === 'none' || v === 'stub' || v === 'full') return v
  throw new Error(`invalid resolve value: expected none|stub|full, got ${String(v)}`)
}

function validateSig(v: unknown): string {
  if (typeof v === 'string' && /^[A-Z_0-9]{1,4}$/.test(v)) return v
  throw new Error(`invalid record signature: ${String(v)}`)
}

function validateUint(name: string, v: unknown, max = 100_000): number {
  if (typeof v === 'number' && Number.isInteger(v) && v >= 0 && v <= max) return v
  throw new Error(`invalid ${name}: expected integer 0–${max}, got ${String(v)}`)
}

function validateTarget(v: unknown): string {
  if (typeof v === 'string' && v.length > 0 && v.length <= 512) return v
  throw new Error(`invalid target: must be a non-empty string`)
}

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
    const info = wrap(() => db.fileInfo())
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
    return wrap(() => entry.db.fileInfo())
  })

  ipcMain.handle(CH.listGroups, (_e, id: string) => {
    const entry = registry.get(id)
    if (!entry) throw new Error(`no database with id ${id}`)
    return wrap(() => entry.db.listGroups())
  })

  ipcMain.handle(CH.listTypeRecords, (_e, id: string, sig: unknown, offset: unknown, limit: unknown) => {
    const entry = registry.get(id)
    if (!entry) throw new Error(`no database with id ${id}`)
    const validSig = validateSig(sig)
    const validOffset = validateUint('offset', offset)
    const validLimit = validateUint('limit', limit)
    return wrap(() => entry.db.listTypeRecords(validSig, validOffset, validLimit))
  })

  ipcMain.handle(CH.recordByFormid, (_e, id: string, formid: unknown, resolve: unknown = 'stub') => {
    const entry = registry.get(id)
    if (!entry) throw new Error(`no database with id ${id}`)
    const validFormid = validateTarget(formid)
    const validResolve = validateResolve(resolve)
    return wrap(() => entry.db.recordByFormid(validFormid, validResolve))
  })

  ipcMain.handle(CH.recordByEdid, async (_e, id: string, edid: unknown) => {
    const entry = registry.get(id)
    if (!entry) throw new Error(`no database with id ${id}`)
    const validEdid = validateTarget(edid)
    return await entry.db.recordByEdid(validEdid)
  })

  ipcMain.handle(CH.recordById, async (_e, id: string, target: unknown, resolve: unknown = 'stub') => {
    const entry = registry.get(id)
    if (!entry) throw new Error(`no database with id ${id}`)
    const validTarget = validateTarget(target)
    const validResolve = validateResolve(resolve)
    return await entry.db.recordById(validTarget, validResolve)
  })

  ipcMain.handle(CH.referencedBy, async (_e, id: string, formid: unknown) => {
    const entry = registry.get(id)
    if (!entry) throw new Error(`no database with id ${id}`)
    const validFormid = validateTarget(formid)
    return await entry.db.referencedBy(validFormid)
  })

  ipcMain.handle(CH.referencedById, async (_e, id: string, target: unknown, depth?: number) => {
    const entry = registry.get(id)
    if (!entry) throw new Error(`no database with id ${id}`)
    const validTarget = validateTarget(target)
    const clampedDepth = Math.max(1, Math.min(depth ?? 1, 6))
    return await entry.db.referencedById(validTarget, clampedDepth)
  })

  ipcMain.handle(CH.parseFormId, (_e, s: string) => {
    return wrap(() => napi.parseFormId(s))
  })
}
