// Replaces the "no wiring test" gap tracked by issue #16, scoped specifically to
// CONTRACT's argument-order guarantee (the one thing TypeScript's `keyof EsmDatabase`
// typing on `entry.method` cannot itself verify).

import { describe, it, expect, mock, vi, beforeEach } from 'bun:test'
import { CONTRACT } from '../shared/ipc-contract'
import { CH } from '../shared/api-types'

type IpcHandler = (event: unknown, ...args: unknown[]) => unknown

const handlers = new Map<string, IpcHandler>()
const showOpenDialog = vi.fn()
const openDatabaseFn = vi.fn()
const parseFormIdFn = vi.fn()
const registryGet = vi.fn()
const registryAdd = vi.fn()
const registryRemove = vi.fn()
const registryListAll = vi.fn()

mock.module('electron', () => ({
  ipcMain: {
    handle: (channel: string, handler: IpcHandler) => {
      handlers.set(channel, handler)
    },
  },
  dialog: {
    showOpenDialog,
  },
}))

mock.module('./addon', () => ({
  napi: {
    EsmDatabase: {
      openDatabase: openDatabaseFn,
    },
    parseFormId: parseFormIdFn,
  },
}))

mock.module('./db-registry', () => ({
  get: registryGet,
  add: registryAdd,
  remove: registryRemove,
  listAll: registryListAll,
}))

const { registerIpc } = await import('./ipc')

const DB_ID = 'db-1'
const DB_PATH = '/tmp/SeventySix.esm'

function makeSpyDb() {
  return {
    fileInfo: vi.fn().mockReturnValue({ masters: [], record_count: 1 }),
    listGroups: vi.fn().mockReturnValue([]),
    listTypeRecords: vi.fn().mockReturnValue([]),
    search: vi.fn().mockReturnValue([]),
    filterTypeRecords: vi.fn().mockReturnValue({ matches: [], truncated: false }),
    listTypeFieldPaths: vi.fn().mockReturnValue([]),
    listTypeChildren: vi.fn().mockReturnValue([]),
    listGroupChildren: vi.fn().mockReturnValue([]),
    recordByFormid: vi.fn().mockReturnValue({ fields: {} }),
    recordByEdid: vi.fn().mockResolvedValue({ fields: {} }),
    recordById: vi.fn().mockResolvedValue({ fields: {} }),
    referencedById: vi.fn().mockResolvedValue({ rows: [] }),
    recordRaw: vi.fn().mockResolvedValue({ subrecords: [] }),
    coverageReport: vi.fn().mockResolvedValue({ types: [] }),
    diff: vi.fn().mockResolvedValue({ added: [], removed: [], changed: [] }),
  }
}

type SpyDb = ReturnType<typeof makeSpyDb>

/** Distinctive, validator-legal raw args for each CONTRACT method (after the db id). */
function rawArgsFor(method: (typeof CONTRACT)[number]['method']): unknown[] {
  const argsByMethod: Partial<Record<(typeof CONTRACT)[number]['method'], unknown[]>> = {
    fileInfo: [],
    listGroups: [],
    listTypeRecords: ['WEAP', 10, 20],
    recordByFormid: ['0xMARK00', 'stub'],
    recordByEdid: ['MarkerEdid', 'full'],
    recordById: ['MarkerTarget', 'none'],
    referencedById: ['MarkerRef', 3],
    listTypeChildren: ['NPC_', 5, 15],
    listGroupChildren: [100, 2, 8],
    search: ['*Rifle*', ['WEAP', 'ARMO'], 'edid', 50],
    filterTypeRecords: ['ARMO', 'EDID', 'eq', 'MarkerVal', 25],
    listTypeFieldPaths: ['WEAP'],
    recordRaw: ['0xRAW000'],
    coverageReport: ['WEAP', 100],
  }
  const args = argsByMethod[method]
  if (!args) throw new Error(`unexpected CONTRACT method: ${String(method)}`)
  return args
}

describe('registerIpc wiring', () => {
  let spyDb: SpyDb
  let spyDbNew: SpyDb

  beforeEach(() => {
    handlers.clear()
    vi.clearAllMocks()
    spyDb = makeSpyDb()
    spyDbNew = makeSpyDb()
    registryGet.mockImplementation((id: string) => {
      if (id === DB_ID) return { db: spyDb, path: DB_PATH, info: { masters: [] } }
      if (id === 'db-2') return { db: spyDbNew, path: '/tmp/new.esm', info: { masters: [] } }
      return undefined
    })
    registerIpc()
  })

  describe('CONTRACT table handlers', () => {
    it('forwards every channel through validate() into the matching db method in order', async () => {
      await Promise.all(
        CONTRACT.map(async (entry) => {
          const rawArgs = rawArgsFor(entry.method)
          const expected = entry.validate(rawArgs)
          const handler = handlers.get(entry.channel)
          expect(handler).toBeDefined()

          await Promise.resolve(handler!({}, DB_ID, ...rawArgs))

          const spy = spyDb[entry.method as keyof SpyDb] as ReturnType<typeof vi.fn>
          expect(spy).toHaveBeenCalledTimes(1)
          expect(spy.mock.calls[0]).toEqual(expected)
        }),
      )
    })

    it('applies validator defaults when optional args are omitted (resolve → stub)', async () => {
      const entry = CONTRACT.find((e) => e.method === 'recordByFormid')!
      const handler = handlers.get(entry.channel)!
      await Promise.resolve(handler({}, DB_ID, '0xMARK00'))
      expect(spyDb.recordByFormid).toHaveBeenCalledWith('0xMARK00', 'stub')
    })

    it('throws when the registry id is unknown', () => {
      const entry = CONTRACT[0]!
      const handler = handlers.get(entry.channel)!
      expect(() => handler({}, 'missing-id')).toThrow('no database with id missing-id')
    })
  })

  describe('hand-written handlers', () => {
    it('openFileDialog passes ESM filters/openFile and returns null when canceled', async () => {
      showOpenDialog.mockResolvedValueOnce({ canceled: true, filePaths: [] })
      const result = await handlers.get(CH.openFileDialog)!({})
      expect(showOpenDialog).toHaveBeenCalledWith({
        filters: [{ name: 'ESM Files', extensions: ['esm'] }],
        properties: ['openFile'],
      })
      expect(result).toBeNull()
    })

    it('openFileDialog returns the first path when not canceled', async () => {
      showOpenDialog.mockResolvedValueOnce({
        canceled: false,
        filePaths: ['/tmp/picked.esm'],
      })
      const result = await handlers.get(CH.openFileDialog)!({})
      expect(result).toBe('/tmp/picked.esm')
    })

    it('openFolderDialog passes openDirectory and returns null when canceled', async () => {
      showOpenDialog.mockResolvedValueOnce({ canceled: true, filePaths: [] })
      const result = await handlers.get(CH.openFolderDialog)!({})
      expect(showOpenDialog).toHaveBeenCalledWith({
        properties: ['openDirectory'],
      })
      expect(result).toBeNull()
    })

    it('openDatabase opens via napi, registers via add, and returns {id,path,info}', async () => {
      const info = { masters: [], record_count: 42 }
      spyDb.fileInfo.mockReturnValueOnce(info)
      openDatabaseFn.mockResolvedValueOnce(spyDb)
      registryAdd.mockReturnValueOnce('42')

      const result = await handlers.get(CH.openDatabase)!({}, DB_PATH)

      expect(openDatabaseFn).toHaveBeenCalledWith(DB_PATH)
      expect(spyDb.fileInfo).toHaveBeenCalledTimes(1)
      expect(registryAdd).toHaveBeenCalledWith(spyDb, DB_PATH, info)
      expect(result).toEqual({ id: '42', path: DB_PATH, info })
    })

    it('closeDatabase removes the id from the registry', () => {
      handlers.get(CH.closeDatabase)!({}, DB_ID)
      expect(registryRemove).toHaveBeenCalledWith(DB_ID)
    })

    it('listOpen returns the registry list mapped to {id,path,info}', () => {
      registryListAll.mockReturnValueOnce([
        { id: '1', path: '/a.esm', info: { n: 1 } },
        { id: '2', path: '/b.esm', info: { n: 2 } },
      ])
      const result = handlers.get(CH.listOpen)!({})
      expect(registryListAll).toHaveBeenCalledTimes(1)
      expect(result).toEqual([
        { id: '1', path: '/a.esm', info: { n: 1 } },
        { id: '2', path: '/b.esm', info: { n: 2 } },
      ])
    })

    it('parseFormId forwards to napi.parseFormId', () => {
      parseFormIdFn.mockReturnValueOnce('0x0000463F')
      const result = handlers.get(CH.parseFormId)!({}, '463F')
      expect(parseFormIdFn).toHaveBeenCalledWith('463F')
      expect(result).toBe('0x0000463F')
    })

    it('diff looks up both ids and calls old.diff(newDb, …)', async () => {
      const result = await handlers.get(CH.diff)!({}, DB_ID, 'db-2', 'WEAP', 'stub', true, ['CELL'])
      expect(registryGet).toHaveBeenCalledWith(DB_ID)
      expect(registryGet).toHaveBeenCalledWith('db-2')
      expect(spyDb.diff).toHaveBeenCalledWith(spyDbNew, 'WEAP', 'stub', true, ['CELL'])
      expect(spyDbNew.diff).not.toHaveBeenCalled()
      expect(result).toEqual({ added: [], removed: [], changed: [] })
    })

    it('diff throws when the old id is unknown', async () => {
      await expect(
        handlers.get(CH.diff)!({}, 'missing', 'db-2', undefined, 'none', false, []),
      ).rejects.toThrow('no database with id missing')
    })

    it('diff throws when the new id is unknown', async () => {
      await expect(
        handlers.get(CH.diff)!({}, DB_ID, 'missing', undefined, 'none', false, []),
      ).rejects.toThrow('no database with id missing')
    })
  })
})
