import type { DbHandle, DbId, FileInfo } from '../shared/api-types'

/** The databases the renderer has open, by the opaque id it holds. Several
 * ids may name the same file; the host keeps one database per path. */
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

  isOpen(path: string): boolean {
    return [...this.entries.values()].some((h) => h.path === path)
  }

  listAll(): DbHandle[] {
    return [...this.entries.values()]
  }
}
