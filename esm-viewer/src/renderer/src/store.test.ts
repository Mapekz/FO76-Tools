import { beforeEach, describe, it, expect } from 'bun:test'
import { useStore } from './store'
import type { DbId, RefListResult, RunnableOp } from '../../shared/api-types'
import { makeDbHandle, makeRecordResult, mockRun } from '../../test-support/fixtures'

function refList(target: string): RefListResult {
  return {
    target,
    rows: [],
    total: 0,
    capped: false,
    requested_depth: 1,
    effective_depth: null,
    depth_capped: false,
    frontier_remaining: 0,
    per_depth_totals: [],
    shown_max_depth: 1,
  }
}

/** A fake `window.api` whose `record` op answers from every open file with a
 * record tagged by its file, after `delay(target)` ms. */
function installApi(delay: (target: string) => number = () => 0) {
  const api = mockRun(async (id: DbId, op: RunnableOp) => {
    const target = 'sel' in op ? String(op.sel.value) : ''
    await new Promise((resolve) => setTimeout(resolve, delay(target)))
    if (op.op === 'record') return makeRecordResult(target, { editor_id: `${id}:${target}` })
    return refList(target)
  })
  ;(globalThis as { window?: unknown }).window = { api }
  return api
}

beforeEach(() => {
  useStore.setState({
    activeDbId: 'db1',
    activeRecord: null,
    recordColumns: [],
    nav: { entries: [], index: -1 },
    openDbs: [makeDbHandle('db1', '/data/A.esm'), makeDbHandle('db2', '/data/B.esm')],
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

describe('navigate', () => {
  it('shows the record from the navigated file and fetches its refs there', async () => {
    const api = installApi()

    await useStore.getState().navigate('db2', '0x2')

    const s = useStore.getState()
    expect(s.activeDbId).toBe('db2')
    expect(s.activeRecord?.editor_id).toBe('db2:0x2')
    expect(s.recordColumns.map((c) => c.dbId)).toEqual(['db1', 'db2'])
    expect(api.run).toHaveBeenCalledWith('db2', expect.objectContaining({ op: 'referenced_by' }))
  })

  it('Back reloads the previous entry from its own file', async () => {
    installApi()
    const { navigate, goBack } = useStore.getState()
    await navigate('db1', '0x1')
    await navigate('db2', '0x2')

    await goBack()

    const s = useStore.getState()
    expect(s.activeDbId).toBe('db1')
    expect(s.activeRecord?.editor_id).toBe('db1:0x1')
  })

  it('a slow earlier load never overwrites a later one', async () => {
    installApi((target) => (target === '0x1' ? 30 : 0))
    const { navigate } = useStore.getState()

    const slow = navigate('db1', '0x1')
    await navigate('db2', '0x2')
    await slow

    expect(useStore.getState().activeRecord?.editor_id).toBe('db2:0x2')
  })
})
