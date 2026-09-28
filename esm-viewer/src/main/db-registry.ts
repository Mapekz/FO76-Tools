import type { DbHandle, DbId, FileInfo } from '../shared/api-types'

/** The databases the renderer has open, by the opaque id it holds. Several
 * ids may name the same file, under different paths; the host keeps one
 * database per file, keyed by the canonical path its `FileInfo.path`
 * reports. */
export class DbRegistry {
  private readonly entries = new Map<DbId, DbHandle>()
  private nextId = 1

  add(path: string, info: FileInfo): DbHandle {
    const handle = { id: String(this.nextId++), path, info }
    this.entries.set(handle.id, handle)
    return handle
  }

  get(id: DbId): DbHandle | undefined {
    return this.entries.get(id)
  }

  /** Remove `id`, returning its handle if it existed. */
  remove(id: DbId): DbHandle | undefined {
    const handle = this.entries.get(id)
    this.entries.delete(id)
    return handle
  }

  /** Whether any id still names the file the host keys as `key`. */
  isOpen(key: string): boolean {
    return [...this.entries.values()].some((h) => h.info.path === key)
  }

  listAll(): DbHandle[] {
    return [...this.entries.values()]
  }
}
