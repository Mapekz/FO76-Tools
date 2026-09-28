import { describe, it, expect } from 'bun:test'
import { applyUnlessCancelled } from './applyUnlessCancelled'

/** A promise and the function that settles it. */
function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((r) => (resolve = r))
  return { promise, resolve }
}

describe('applyUnlessCancelled', () => {
  it('a late response for a superseded input never lands', async () => {
    const applied: string[] = []
    const a = deferred<string>()
    const b = deferred<string>()
    const cancelA = applyUnlessCancelled(a.promise, (v) => applied.push(v))
    cancelA() // the input changed: file A's effect cleans up
    applyUnlessCancelled(b.promise, (v) => applied.push(v))
    b.resolve('B')
    a.resolve('A')
    await Promise.resolve()
    await Promise.resolve()
    expect(applied).toEqual(['B'])
  })

  it('reports an error only while current', async () => {
    const errors: unknown[] = []
    applyUnlessCancelled(
      Promise.reject(new Error('x')),
      () => {},
      (e) => errors.push(e),
    )
    const cancel = applyUnlessCancelled(
      Promise.reject(new Error('y')),
      () => {},
      (e) => errors.push(e),
    )
    cancel()
    await new Promise((r) => setTimeout(r, 0))
    expect(errors.map((e) => (e as Error).message)).toEqual(['x'])
  })
})
