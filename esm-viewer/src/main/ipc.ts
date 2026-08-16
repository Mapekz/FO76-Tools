import { ipcMain, dialog } from 'electron'
import { napi } from './addon'
import * as registry from './db-registry'
import { CH } from '../shared/api-types'
import { CONTRACT } from '../shared/ipc-contract'
import { validateOptionalText, validateBodies, validateSigArray } from './ipc-validators'

// `./ipc` re-exports the validators as the canonical import point for
// `ipc-validators.test.ts` and `../shared/ipc-contract.ts`.
export * from './ipc-validators'

function wrap(fn: () => unknown) {
  try {
    return fn()
  } catch (e: unknown) {
    throw new Error(e instanceof Error ? e.message : String(e), { cause: e })
  }
}

export function registerIpc(): void {
  ipcMain.handle(CH.openFileDialog, async () => {
    const { canceled, filePaths } = await dialog.showOpenDialog({
      filters: [{ name: 'ESM Files', extensions: ['esm'] }],
      properties: ['openFile'],
    })
    return canceled ? null : filePaths[0]
  })

  ipcMain.handle(CH.openFolderDialog, async () => {
    const { canceled, filePaths } = await dialog.showOpenDialog({
      properties: ['openDirectory'],
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
    registry.listAll().map(({ id, path, info }) => ({ id, path, info })),
  )

  ipcMain.handle(CH.parseFormId, (_e, s: string) => {
    return wrap(() => napi.parseFormId(s))
  })

  // Table-driven: registers one handler per entry in the shared descriptor
  // table (`../shared/ipc-contract.ts`). Each handler does the same thing the
  // hand-written blocks above/below do explicitly: registry lookup +
  // not-found guard + validate + wrap() + the addon call.
  for (const entry of CONTRACT) {
    ipcMain.handle(entry.channel, (_e, id: string, ...rawArgs: unknown[]) => {
      const dbEntry = registry.get(id)
      if (!dbEntry) throw new Error(`no database with id ${id}`)
      const args = entry.validate(rawArgs)
      return wrap(() => {
        // Cast unavoidable: CONTRACT is heterogeneous (each method has its own
        // arity/argument types), so there's no single call signature TS can
        // check generically here — `entry.validate` is each method's own
        // type-safe boundary, already run above.
        const method = dbEntry.db[entry.method] as (...a: unknown[]) => unknown
        return method.apply(dbEntry.db, args)
      })
    })
  }

  ipcMain.handle(
    CH.diff,
    async (
      _e,
      oldId: string,
      newId: string,
      recordType: unknown,
      bodies: unknown,
      suppressNoise: unknown,
      excludeTypes: unknown,
    ) => {
      const entryOld = registry.get(oldId)
      const entryNew = registry.get(newId)
      if (!entryOld) throw new Error(`no database with id ${oldId}`)
      if (!entryNew) throw new Error(`no database with id ${newId}`)
      const validRecordType = validateOptionalText('recordType', recordType, 4)
      const validBodies = validateBodies(bodies)
      if (typeof suppressNoise !== 'boolean') {
        throw new Error(`invalid suppressNoise: expected boolean, got ${String(suppressNoise)}`)
      }
      const validExcludeTypes = validateSigArray(excludeTypes ?? [])
      return await entryOld.db.diff(
        entryNew.db,
        validRecordType,
        validBodies,
        suppressNoise,
        validExcludeTypes,
      )
    },
  )
}
