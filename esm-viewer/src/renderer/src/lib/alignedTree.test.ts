import { describe, it, expect } from 'vitest'
import { buildLeafNode, buildAlignedTree, deepEqualForConflict, MISSING } from './alignedTree'

describe('deepEqualForConflict', () => {
  it('compares FormID stubs by formid only, ignoring editor_id/record_type', () => {
    const a = { formid: '0x1', editor_id: 'Foo', record_type: 'ARMO' }
    const b = { formid: '0x1', editor_id: null, record_type: 'KYWD' }
    expect(deepEqualForConflict(a, b)).toBe(true)
  })

  it('treats FormID stubs with different formid as unequal', () => {
    const a = { formid: '0x1', editor_id: 'Foo', record_type: 'ARMO' }
    const b = { formid: '0x2', editor_id: 'Foo', record_type: 'ARMO' }
    expect(deepEqualForConflict(a, b)).toBe(false)
  })
})

describe('buildLeafNode (unified leaf-conflict rule)', () => {
  it('flags conflict when present values differ across columns', () => {
    const node = buildLeafNode('flags', 'flags', [1, 2])
    expect(node.conflict).toBe(true)
  })

  it('has no conflict when all present values are equal', () => {
    const node = buildLeafNode('flags', 'flags', [1, 1, 1])
    expect(node.conflict).toBe(false)
  })

  it('flags conflict on a presence mismatch even when present values agree', () => {
    const node = buildLeafNode('flags', 'flags', [1, MISSING, 1])
    expect(node.conflict).toBe(true)
  })

  it('compares FormID stubs by formid only', () => {
    const a = { formid: '0x1', editor_id: 'Foo', record_type: 'ARMO' }
    const b = { formid: '0x1', editor_id: null, record_type: 'ARMO' }
    const node = buildLeafNode('kw', 'kw', [a, b])
    expect(node.conflict).toBe(false)
  })

  it('flags conflict for FormID stubs whose formid differs', () => {
    const a = { formid: '0x1', editor_id: 'Foo', record_type: 'ARMO' }
    const b = { formid: '0x2', editor_id: 'Foo', record_type: 'ARMO' }
    const node = buildLeafNode('kw', 'kw', [a, b])
    expect(node.conflict).toBe(true)
  })
})

describe('buildAlignedTree leaf conflicts (exercised through the shared buildLeafNode path)', () => {
  it('flags a field that differs across columns', () => {
    const tree = buildAlignedTree([{ name: 'Alpha' }, { name: 'Beta' }])
    expect(tree.find((n) => n.label === 'name')?.conflict).toBe(true)
  })

  it('does not flag a field that is identical across columns', () => {
    const tree = buildAlignedTree([{ name: 'Alpha' }, { name: 'Alpha' }])
    expect(tree.find((n) => n.label === 'name')?.conflict).toBe(false)
  })

  it('flags a field missing from one column entirely', () => {
    const tree = buildAlignedTree([{ name: 'Alpha' }, {}])
    expect(tree.find((n) => n.label === 'name')?.conflict).toBe(true)
  })
})
