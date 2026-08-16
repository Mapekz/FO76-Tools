import { describe, it, expect } from 'bun:test'
import type * as DbRegistry from './db-registry'
import type { TypedEsmDatabase } from './addon'

// ipc.test.ts calls `mock.module('./db-registry', ...)`, which — since Bun
// has no way to un-mock a module — permanently replaces that specifier for
// the rest of this test process once ipc.test.ts has been imported. A
// distinct specifier (the `?real` suffix) resolves to the same file but
// isn't the one ipc.test.ts mocked, so this gets the genuine implementation
// regardless of which test file Bun evaluates first. The specifier is built
// at runtime (rather than a string literal) so tsc doesn't try to statically
// resolve the `?real` suffix as a module path; the type-only import above
// supplies the real types via the `typeof DbRegistry` cast.
const cacheBust = 'real'
const { add, get, listAll, remove } = (await import(
  `./db-registry.ts?${cacheBust}`
)) as typeof DbRegistry

// The registry never calls into the db handle itself — it just stores and
// returns whatever's passed in — so a bare object stands in for a real
// TypedEsmDatabase here.
const fakeDb = {} as TypedEsmDatabase

describe('db-registry', () => {
  it('add assigns a fresh id and get returns the stored entry', () => {
    const id = add(fakeDb, '/data/A.esm', { record_count: 1 })
    expect(get(id)).toEqual({ db: fakeDb, path: '/data/A.esm', info: { record_count: 1 } })
  })

  it('assigns distinct ids across successive add calls', () => {
    const id1 = add(fakeDb, '/data/B.esm', {})
    const id2 = add(fakeDb, '/data/C.esm', {})
    expect(id1).not.toBe(id2)
  })

  it('get returns undefined for an unknown id', () => {
    expect(get('no-such-id')).toBeUndefined()
  })

  it('remove drops the entry so get and listAll no longer see it', () => {
    const id = add(fakeDb, '/data/D.esm', {})
    remove(id)
    expect(get(id)).toBeUndefined()
    expect(listAll().some((e) => e.id === id)).toBe(false)
  })

  it('listAll reflects every currently-registered entry', () => {
    const id = add(fakeDb, '/data/E.esm', { foo: 'bar' })
    const entry = listAll().find((e) => e.id === id)
    expect(entry).toEqual({ id, path: '/data/E.esm', info: { foo: 'bar' } })
  })
})
