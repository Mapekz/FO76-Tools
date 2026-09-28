import { describe, it, expect } from 'bun:test'
import { validateDiffRequest, validateOp, validateRefDepth, validateUint } from './ipc-validators'

describe('validateUint', () => {
  it('passes a valid non-negative integer through unchanged', () => {
    expect(validateUint('offset', 0)).toBe(0)
    expect(validateUint('offset', 42)).toBe(42)
  })

  it('uses a default max of 100_000', () => {
    expect(validateUint('limit', 100_000)).toBe(100_000)
    expect(() => validateUint('limit', 100_001)).toThrow(
      'invalid limit: expected integer 0–100000, got 100001',
    )
  })

  it('throws on a negative, fractional or non-number value', () => {
    for (const bad of [-1, 1.5, '5']) {
      expect(() => validateUint('offset', bad)).toThrow(/invalid offset/)
    }
  })
})

describe('validateRefDepth', () => {
  it('defaults an absent depth to one hop', () => {
    expect(validateRefDepth(undefined)).toBe(1)
    expect(validateRefDepth(null)).toBe(1)
  })

  it('clamps integers into 1..=6', () => {
    expect(validateRefDepth(0)).toBe(1)
    expect(validateRefDepth(3)).toBe(3)
    expect(validateRefDepth(99)).toBe(6)
  })

  it('rejects non-integers instead of letting them reach the engine as an unbounded walk', () => {
    for (const bad of ['3', 2.5, Number.NaN, {}]) {
      expect(() => validateRefDepth(bad)).toThrow(/invalid depth/)
    }
  })
})

describe('validateOp', () => {
  it('passes a runnable op through', () => {
    const op = { op: 'list_groups' }
    expect(validateOp(op)).toEqual({ op: 'list_groups' })
  })

  it('rejects diff, unknown ops and non-objects', () => {
    for (const bad of [{ op: 'diff' }, { op: 'drop_everything' }, {}, null, 'record', []]) {
      expect(() => validateOp(bad)).toThrow(/invalid op/)
    }
  })

  it('bounds limit on any op that has one', () => {
    expect(() => validateOp({ op: 'search', pattern: '*', limit: 1e9 })).toThrow(/invalid limit/)
  })

  it('bounds a referenced_by walk to 1..=6 hops', () => {
    expect(validateOp({ op: 'referenced_by', depth: 0, limit: 0 })).toMatchObject({ depth: 1 })
    expect(validateOp({ op: 'referenced_by', depth: 50, limit: 0 })).toMatchObject({ depth: 6 })
    expect(() => validateOp({ op: 'referenced_by', depth: 'x', limit: 0 })).toThrow(/invalid depth/)
  })

  it('does not mutate its input', () => {
    const op = { op: 'referenced_by', depth: 0, limit: 0 }
    validateOp(op)
    expect(op.depth).toBe(0)
  })
})

describe('validateDiffRequest', () => {
  it('defaults a missing record_type and options', () => {
    expect(validateDiffRequest({})).toEqual({ record_type: null, options: {} })
  })

  it('rejects a record_type longer than a signature', () => {
    expect(() => validateDiffRequest({ record_type: 'WEAPON' })).toThrow(/invalid record_type/)
  })
})
