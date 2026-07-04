import type { DbId } from '../shared/api-types'
import type { EsmDatabase } from './addon'

interface DbEntry {
  db: EsmDatabase
  path: string
  info: unknown
}

const registry = new Map<DbId, DbEntry>()
let nextId = 1

export function add(db: EsmDatabase, path: string, info: unknown): DbId {
  const id = String(nextId++)
  registry.set(id, { db, path, info })
  return id
}

export function get(id: DbId): DbEntry | undefined {
  return registry.get(id)
}

export function remove(id: DbId): void {
  registry.delete(id)
}

export function listAll(): Array<{ id: DbId; path: string; info: unknown }> {
  return Array.from(registry.entries()).map(([id, { path, info }]) => ({ id, path, info }))
}
