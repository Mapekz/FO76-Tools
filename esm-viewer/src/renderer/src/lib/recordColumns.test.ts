import { describe, it, expect, vi } from 'vitest'
import { buildRecordColumns, columnLabels } from './recordColumns'
import type { DbHandle, RecordResult } from '../../../shared/api-types'

function makeDb(id: string, path: string): DbHandle {
  return {
    id,
    path,
    info: {
      path,
      version: 1,
      record_count: 0,
      next_object_id: 0,
      author: null,
      description: null,
      masters: [],
      flags: 0,
      is_esm: true,
      is_localized: false,
    },
  }
}

function makeRecord(formId: string, editorId: string | null = null): RecordResult {
  return {
    header: {
      signature: 'ARMO',
      form_id: formId,
      flags: 0,
      form_version: 44,
      data_size: 0,
      offset: 0,
    },
    editor_id: editorId,
    fields: {},
  }
}

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
    const db = makeDb('db1', '/data/SeventySix.esm')
    const rec = makeRecord('0x00012345', 'SomeEdid')
    const api = { recordById: vi.fn(async () => rec) }

    const result = await buildRecordColumns('SomeEdid', 'db1', [db], api)

    expect(result.active).toBe(rec)
    expect(result.columns).toEqual([{ dbId: 'db1', fileName: 'SeventySix.esm', record: rec }])
    expect(api.recordById).toHaveBeenCalledWith('db1', 'SomeEdid', 'stub')
  })

  it('drops a column for an open DB that rejects (FormID not present there)', async () => {
    const dbA = makeDb('dbA', '/data/A.esm')
    const dbB = makeDb('dbB', '/data/B.esm')
    const recA = makeRecord('0x00012345', 'Foo')
    const api = {
      recordById: vi.fn(async (id: string) => {
        if (id === 'dbA') return recA
        throw new Error('FormID not found')
      }),
    }

    const result = await buildRecordColumns('Foo', 'dbA', [dbA, dbB], api)

    expect(result.columns).toEqual([{ dbId: 'dbA', fileName: 'A.esm', record: recA }])
    // The fan-out probes every other DB by the resolved FormID, not the raw target.
    expect(api.recordById).toHaveBeenCalledWith('dbB', '0x00012345', 'stub')
  })

  it('disambiguates column labels when two open DBs share a basename', async () => {
    const dbOld = makeDb('dbOld', '/data/20260619/SeventySix.esm')
    const dbNew = makeDb('dbNew', '/data/20260702/SeventySix.esm')
    const recOld = makeRecord('0x00012345', 'Foo')
    const recNew = makeRecord('0x00012345', 'Foo')
    const api = {
      recordById: vi.fn(async (id: string) => (id === 'dbOld' ? recOld : recNew)),
    }

    const result = await buildRecordColumns('Foo', 'dbOld', [dbOld, dbNew], api)

    expect(result.columns).toEqual([
      { dbId: 'dbOld', fileName: '20260619/SeventySix.esm', record: recOld },
      { dbId: 'dbNew', fileName: '20260702/SeventySix.esm', record: recNew },
    ])
  })
})
