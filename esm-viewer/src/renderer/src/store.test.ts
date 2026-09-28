import { beforeEach, describe, it, expect } from 'bun:test'
import { useStore } from './store'
import { makeRecordResult } from '../../test-support/fixtures'

beforeEach(() => {
  useStore.setState({
    activeDbId: 'db1',
    activeRecord: null,
    recordColumns: [],
    nav: { entries: [], index: -1 },
  })
})

describe('showRecord', () => {
  it('makes the record’s file the active file', () => {
    const rec = makeRecordResult('0x00012345')
    const columns = [{ dbId: 'db2', fileName: 'B.esm', record: rec }]

    useStore.getState().showRecord('db2', rec, columns)

    const s = useStore.getState()
    expect(s.activeDbId).toBe('db2')
    expect(s.activeRecord).toBe(rec)
    expect(s.recordColumns).toBe(columns)
  })
})

describe('nav history', () => {
  it('Back returns the entry with its own file', () => {
    const { navPush, navBack } = useStore.getState()
    navPush({ dbId: 'db1', formid: '0x1' })
    navPush({ dbId: 'db2', formid: '0x2' })

    expect(navBack()).toEqual({ dbId: 'db1', formid: '0x1' })
  })

  it('re-selecting the current entry does not grow history', () => {
    const { navPush } = useStore.getState()
    navPush({ dbId: 'db1', formid: '0x1' })
    navPush({ dbId: 'db1', formid: '0x1' })
    navPush({ dbId: 'db2', formid: '0x1' })

    expect(useStore.getState().nav).toEqual({
      entries: [
        { dbId: 'db1', formid: '0x1' },
        { dbId: 'db2', formid: '0x1' },
      ],
      index: 1,
    })
  })
})
