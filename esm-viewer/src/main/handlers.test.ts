import { describe, it, expect, vi, beforeEach } from 'bun:test'
import { CH } from '../shared/api-types'
import type { EsmHost } from './addon'
import { DbRegistry } from './db-registry'
import { createHandlers, type HandlerDeps } from './handlers'
import type { FileInfo } from '../shared/api-types'

const info: FileInfo = {
  path: '/x.esm',
  version: 1,
  record_count: 0,
  next_object_id: 0,
  author: null,
  description: null,
  masters: [],
  flags: 0,
  is_esm: true,
  is_localized: false,
}

/** The host keys a file by its canonical path: here a folder and the
 * `.esm` inside it both resolve to `/real/<name>.esm`. */
function canonical(path: string): string {
  const name = path.endsWith('.esm') ? path.split('/').pop()! : 'A.esm'
  return `/real/${name}`
}

function setup() {
  const host = {
    open: vi.fn(async (path: string) => ({ ...info, path: canonical(path) })),
    run: vi.fn(async () => ({ ok: true })),
    close: vi.fn(),
  }
  const deps: HandlerDeps = {
    host: host as unknown as EsmHost,
    registry: new DbRegistry(),
    parseFormId: vi.fn((s: string) => `0x${s.padStart(8, '0')}`),
    showOpenDialog: vi.fn(async () => ({ canceled: true, filePaths: [] as string[] })),
  }
  return { host, deps, handlers: createHandlers(deps) }
}

describe('createHandlers', () => {
  let t: ReturnType<typeof setup>
  beforeEach(() => {
    t = setup()
  })

  it('openDatabase opens through the host and registers the handle', async () => {
    const handle = await t.handlers[CH.openDatabase]!('/data/A.esm')
    expect(t.host.open).toHaveBeenCalledWith('/data/A.esm')
    expect(handle).toEqual({ id: '1', path: '/data/A.esm', info: { ...info, path: '/real/A.esm' } })
    expect(t.handlers[CH.listOpen]!()).toEqual([handle])
  })

  it('run validates the op and runs it against the id’s file', async () => {
    await t.handlers[CH.openDatabase]!('/data/A.esm')
    await t.handlers[CH.run]!('1', { op: 'referenced_by', depth: 0, limit: 0 })
    expect(t.host.run).toHaveBeenCalledWith('/real/A.esm', {
      op: 'referenced_by',
      depth: 1,
      limit: 0,
    })
  })

  it('run rejects an unknown id and an invalid op', async () => {
    await t.handlers[CH.openDatabase]!('/data/A.esm')
    expect(() => t.handlers[CH.run]!('nope', { op: 'list_groups' })).toThrow(
      'no database with id nope',
    )
    expect(() => t.handlers[CH.run]!('1', { op: 'diff' })).toThrow(/invalid op/)
  })

  it('diff runs on the first database with the second one’s file', async () => {
    await t.handlers[CH.openDatabase]!('/data/A.esm')
    await t.handlers[CH.openDatabase]!('/data/B.esm')
    await t.handlers[CH.diff]!('1', '2', { record_type: 'WEAP', options: { bodies: 'none' } })
    expect(t.host.run).toHaveBeenCalledWith('/real/A.esm', {
      op: 'diff',
      b: '/real/B.esm',
      record_type: 'WEAP',
      options: { bodies: 'none' },
    })
  })

  it('closeDatabase closes the host database only when no id still names its file', async () => {
    await t.handlers[CH.openDatabase]!('/data/A.esm')
    await t.handlers[CH.openDatabase]!('/data')
    t.handlers[CH.closeDatabase]!('1')
    expect(t.host.close).not.toHaveBeenCalled()
    t.handlers[CH.closeDatabase]!('2')
    expect(t.host.close).toHaveBeenCalledWith('/real/A.esm')
  })

  /** Hold the host's next open until the returned `release` resolves it
   * (or `fail` rejects it). */
  function holdNextOpen() {
    let release!: () => void
    let fail!: () => void
    t.host.open.mockImplementationOnce(
      (path: string) =>
        new Promise((resolve, reject) => {
          release = () => resolve({ ...info, path: canonical(path) })
          fail = () => reject(new Error('open failed'))
        }),
    )
    return { release: () => release(), fail: () => fail() }
  }

  it('closing a file’s last id while an alias opens keeps the file open', async () => {
    await t.handlers[CH.openDatabase]!('/data/A.esm')
    const held = holdNextOpen()
    const reopening = t.handlers[CH.openDatabase]!('/data') as Promise<unknown>
    t.handlers[CH.closeDatabase]!('1')
    held.release()
    await reopening
    expect(t.host.close).not.toHaveBeenCalled()
    await t.handlers[CH.run]!('2', { op: 'file_info' })
    expect(t.host.run).toHaveBeenCalledWith('/real/A.esm', { op: 'file_info' })
  })

  it.each(['release', 'fail'] as const)(
    'a close deferred by an open in flight runs once the open settles (%s)',
    async (settle) => {
      await t.handlers[CH.openDatabase]!('/data/A.esm')
      const held = holdNextOpen()
      const opening = t.handlers[CH.openDatabase]!('/data/B.esm') as Promise<unknown>
      t.handlers[CH.closeDatabase]!('1')
      expect(t.host.close).not.toHaveBeenCalled()
      held[settle]()
      await opening.catch(() => {})
      expect(t.host.close).toHaveBeenCalledWith('/real/A.esm')
    },
  )

  it('the file dialog passes its filter and returns null when canceled', async () => {
    expect(await t.handlers[CH.openFileDialog]!()).toBeNull()
    expect(t.deps.showOpenDialog).toHaveBeenCalledWith({
      filters: [{ name: 'ESM Files', extensions: ['esm'] }],
      properties: ['openFile'],
    })
  })

  it('parseFormId forwards to the addon', () => {
    expect(t.handlers[CH.parseFormId]!('463F')).toBe('0x0000463F')
  })
})
