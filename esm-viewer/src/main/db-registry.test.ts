import { describe, it, expect } from 'bun:test'
import { DbRegistry } from './db-registry'
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

describe('DbRegistry', () => {
  it('assigns distinct ids and returns the stored handle', () => {
    const registry = new DbRegistry()
    const a = registry.add('/data/A.esm', info)
    const b = registry.add('/data/B.esm', info)
    expect(a.id).not.toBe(b.id)
    expect(registry.get(a.id)).toEqual({ id: a.id, path: '/data/A.esm', info })
  })

  it('returns undefined for an unknown id', () => {
    expect(new DbRegistry().get('no-such-id')).toBeUndefined()
  })

  it('remove returns the handle and forgets it', () => {
    const registry = new DbRegistry()
    const a = registry.add('/data/A.esm', info)
    expect(registry.remove(a.id)).toEqual(a)
    expect(registry.get(a.id)).toBeUndefined()
    expect(registry.listAll()).toEqual([])
  })

  it('isOpen reports whether any id still names a path', () => {
    const registry = new DbRegistry()
    const a = registry.add('/data/A.esm', info)
    const again = registry.add('/data/A.esm', info)
    registry.remove(a.id)
    expect(registry.isOpen('/data/A.esm')).toBe(true)
    registry.remove(again.id)
    expect(registry.isOpen('/data/A.esm')).toBe(false)
  })
})
