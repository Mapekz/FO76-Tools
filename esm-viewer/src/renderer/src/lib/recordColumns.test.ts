import { describe, it, expect } from 'bun:test'
import { buildRecordColumns, columnLabels } from './recordColumns'
import { makeDbHandle, makeRecordResult, mockRun } from '../../../test-support/fixtures'

describe('columnLabels', () => {
  it('uses plain basenames when every open file has a unique one', () => {
    const labels = columnLabels(['/data/A.esm', '/data/B.esm'])
    expect(labels.get('/data/A.esm')).toBe('A.esm')
    expect(labels.get('/data/B.esm')).toBe('B.esm')
  })

  it('prefixes the parent directory when two files share a basename', () => {
    const labels = columnLabels(['/data/20260619/SeventySix.esm', '/data/20260702/SeventySix.esm'])
    expect(labels.get('/data/20260619/SeventySix.esm')).toBe('20260619/SeventySix.esm')
    expect(labels.get('/data/20260702/SeventySix.esm')).toBe('20260702/SeventySix.esm')
  })
})

describe('buildRecordColumns', () => {
  it('builds a single column when only one DB is open', async () => {
    const db = makeDbHandle('db1', '/data/SeventySix.esm')
    const rec = makeRecordResult('0x00012345', { editor_id: 'SomeEdid' })
    const api = mockRun(async () => rec)

    const result = await buildRecordColumns('SomeEdid', 'db1', [db], api)

    expect(result.active).toBe(rec)
    expect(result.columns).toEqual([{ dbId: 'db1', fileName: 'SeventySix.esm', record: rec }])
    expect(api.run).toHaveBeenCalledWith('db1', {
      op: 'record',
      sel: { kind: 'auto', value: 'SomeEdid' },
      depth: 'stub',
    })
  })

  it('drops a column for an open DB that rejects (FormID not present there)', async () => {
    const dbA = makeDbHandle('dbA', '/data/A.esm')
    const dbB = makeDbHandle('dbB', '/data/B.esm')
    const recA = makeRecordResult('0x00012345', { editor_id: 'Foo' })
    const api = mockRun(async (id) => {
      if (id === 'dbA') return recA
      throw new Error('FormID not found')
    })

    const result = await buildRecordColumns('Foo', 'dbA', [dbA, dbB], api)

    expect(result.columns).toEqual([{ dbId: 'dbA', fileName: 'A.esm', record: recA }])
    // The fan-out probes every other DB by the resolved FormID, not the raw target.
    expect(api.run).toHaveBeenCalledWith('dbB', {
      op: 'record',
      sel: { kind: 'auto', value: '0x00012345' },
      depth: 'stub',
    })
  })

  it('disambiguates column labels when two open DBs share a basename', async () => {
    const dbOld = makeDbHandle('dbOld', '/data/20260619/SeventySix.esm')
    const dbNew = makeDbHandle('dbNew', '/data/20260702/SeventySix.esm')
    const recOld = makeRecordResult('0x00012345', { editor_id: 'Foo' })
    const recNew = makeRecordResult('0x00012345', { editor_id: 'Foo' })
    const api = mockRun(async (id) => (id === 'dbOld' ? recOld : recNew))

    const result = await buildRecordColumns('Foo', 'dbOld', [dbOld, dbNew], api)

    expect(result.columns).toEqual([
      { dbId: 'dbOld', fileName: '20260619/SeventySix.esm', record: recOld },
      { dbId: 'dbNew', fileName: '20260702/SeventySix.esm', record: recNew },
    ])
  })
})
