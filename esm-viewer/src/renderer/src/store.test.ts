import { afterAll, beforeEach, describe, it, expect } from 'bun:test'
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

const realWindow = (globalThis as { window?: unknown }).window
afterAll(() => {
  ;(globalThis as { window?: unknown }).window = realWindow
})

/** A fake `window.api` whose `record` op answers with a record tagged by its
 * file, after `delay(target, op, id)` ms, and fails for a file in `missingIn`;
 * `referenced_by` answers one row tagged by its file and depth, and fails
 * for a file in `refsFailIn`; `record_raw` answers a dump tagged by its file. */
function installApi(
  delay: (target: string, op: string, id: string) => number = () => 0,
  missingIn: string[] = [],
  refsFailIn: string[] = [],
) {
  const api = mockRun(async (id: DbId, op: RunnableOp) => {
    const target = 'sel' in op ? String(op.sel.value) : ''
    await new Promise((resolve) => setTimeout(resolve, delay(target, op.op, id)))
    if (op.op === 'record') {
      if (missingIn.includes(id)) throw new Error(`${target} not in ${id}`)
      return makeRecordResult(target, { editor_id: `${id}:${target}` })
    }
    if (op.op === 'record_raw') {
      const header = makeRecordResult(target).header
      return { header, subrecords: [{ signature: `${id}:${target}`, size: 0, hex: '' }] }
    }
    if (refsFailIn.includes(id)) throw new Error(`refs failed in ${id}`)
    const depth = op.op === 'referenced_by' ? op.depth : 0
    const form_id = depth === 1 ? `${id}-ref` : `${id}-ref-d${depth}`
    return { ...refList(target), rows: [{ form_id }] }
  })
  ;(globalThis as { window?: unknown }).window = { api }
  return api
}

const tick = (ms = 5) => new Promise((resolve) => setTimeout(resolve, ms))
const db1Only = [makeDbHandle('db1', '/data/A.esm')]

beforeEach(() => {
  useStore.setState({
    activeDbId: 'db1',
    activeRecord: null,
    recordColumns: [],
    referencedBy: [],
    referencedByDepth: 1,
    referencedByError: null,
    raw: { view: null, loading: false, error: null },
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

  it('ignores a file that is not open, before touching history', async () => {
    const api = installApi()
    await useStore.getState().navigate('db1', '0x1')
    api.run.mockClear()

    await useStore.getState().navigate('db9', '0x9')

    const s = useStore.getState()
    expect(s.nav).toEqual({ entries: [{ dbId: 'db1', formid: '0x1' }], index: 0 })
    expect(s.activeRecord?.editor_id).toBe('db1:0x1')
    expect(api.run).not.toHaveBeenCalled()
  })
})

describe('load races', () => {
  it('Back during a pending navigation shows the Back entry', async () => {
    installApi((target) => (target === '0x2' ? 30 : 0))
    const { navigate, goBack } = useStore.getState()
    await navigate('db1', '0x1')
    const pending = navigate('db2', '0x2')
    await goBack()
    await pending
    expect(useStore.getState().activeRecord?.editor_id).toBe('db1:0x1')
    expect(useStore.getState().nav.entries).toEqual([{ dbId: 'db1', formid: '0x1' }])
  })

  it('a failed navigation leaves history and the shown record alone', async () => {
    installApi(undefined, ['db2'])
    const { navigate } = useStore.getState()
    await navigate('db1', '0x1')
    await navigate('db2', '0x2')
    const s = useStore.getState()
    expect(s.activeRecord?.editor_id).toBe('db1:0x1')
    expect(s.nav).toEqual({ entries: [{ dbId: 'db1', formid: '0x1' }], index: 0 })
  })

  it('refs of a superseded load never land', async () => {
    installApi((target, op) => (target === '0x1' && op === 'referenced_by' ? 30 : 0))
    const { navigate } = useStore.getState()
    const slow = navigate('db1', '0x1')
    await new Promise((resolve) => setTimeout(resolve, 5))
    await navigate('db2', '0x2')
    await slow
    expect(useStore.getState().referencedBy).toEqual([{ form_id: 'db2-ref' } as never])
  })
})

describe('selectFile', () => {
  it('loads the shown record’s copy in the selected file', async () => {
    installApi()
    await useStore.getState().navigate('db1', '0x1')
    await useStore.getState().selectFile('db2')
    const s = useStore.getState()
    expect(s.activeDbId).toBe('db2')
    expect(s.activeRecord?.editor_id).toBe('db2:0x1')
    expect(s.nav.entries.at(-1)).toEqual({ dbId: 'db2', formid: '0x1' })
  })

  it('clears the record view when the selected file lacks the record', async () => {
    installApi(() => 0, ['db2'])
    await useStore.getState().navigate('db1', '0x1')
    await useStore.getState().selectFile('db2')
    const s = useStore.getState()
    expect(s.activeDbId).toBe('db2')
    expect(s.activeRecord).toBeNull()
    expect(s.referencedBy).toEqual([])
  })

  it('enters history as the record shows, before its refs arrive', async () => {
    installApi((_, op, id) => (op === 'referenced_by' && id === 'db2' ? 50 : 0))
    await useStore.getState().navigate('db1', '0x1')
    const pending = useStore.getState().selectFile('db2')
    await tick(10)
    const s = useStore.getState()
    expect(s.activeRecord?.editor_id).toBe('db2:0x1')
    expect(s.nav.entries.at(-1)).toEqual({ dbId: 'db2', formid: '0x1' })
    await pending
  })

  it('keeps the record when only its refs lookup fails', async () => {
    installApi(() => 0, [], ['db2'])
    await useStore.getState().navigate('db1', '0x1')
    await useStore.getState().selectFile('db2')
    const s = useStore.getState()
    expect(s.activeDbId).toBe('db2')
    expect(s.activeRecord?.editor_id).toBe('db2:0x1')
    expect(s.referencedBy).toEqual([])
    expect(s.referencedByError).toBe('refs failed in db2')
    expect(s.nav.entries.at(-1)).toEqual({ dbId: 'db2', formid: '0x1' })
  })

  it('supersedes a pending load when nothing is shown yet', async () => {
    installApi((target) => (target === '0x1' ? 30 : 0))
    const pending = useStore.getState().navigate('db1', '0x1')
    await useStore.getState().selectFile('db2')
    await pending
    const s = useStore.getState()
    expect(s.activeDbId).toBe('db2')
    expect(s.activeRecord).toBeNull()
  })
})

describe('fileClosed', () => {
  it('abandons a load pending for the closed file and prunes its history', async () => {
    installApi((target) => (target === '0x2' ? 30 : 0))
    const { navigate } = useStore.getState()
    await navigate('db1', '0x1')
    const pending = navigate('db2', '0x2')
    useStore.getState().fileClosed('db2', [makeDbHandle('db1', '/data/A.esm')])
    await pending
    const s = useStore.getState()
    expect(s.activeDbId).toBe('db1')
    expect(s.activeRecord?.editor_id).toBe('db1:0x1')
    expect(s.recordColumns.map((c) => c.dbId)).toEqual(['db1'])
    expect(s.nav).toEqual({ entries: [{ dbId: 'db1', formid: '0x1' }], index: 0 })
  })

  it('clears the record view when it showed the closed file', async () => {
    installApi()
    await useStore.getState().navigate('db2', '0x2')
    useStore.getState().fileClosed('db2', [makeDbHandle('db1', '/data/A.esm')])
    const s = useStore.getState()
    expect(s.activeDbId).toBe('db1')
    expect(s.activeRecord).toBeNull()
  })

  it('closing the shown file shows the surviving entry under the cursor', async () => {
    installApi()
    const { navigate } = useStore.getState()
    await navigate('db1', '0x1')
    await navigate('db2', '0x2')
    await useStore.getState().fileClosed('db2', db1Only)
    const s = useStore.getState()
    expect(s.nav).toEqual({ entries: [{ dbId: 'db1', formid: '0x1' }], index: 0 })
    expect(s.activeDbId).toBe('db1')
    expect(s.activeRecord?.editor_id).toBe('db1:0x1')
    expect(s.referencedBy).toEqual([{ form_id: 'db1-ref' } as never])
  })

  it('Back after closing the shown file reaches the entry before the survivor', async () => {
    installApi()
    const { navigate } = useStore.getState()
    await navigate('db1', '0x1')
    await navigate('db1', '0x3')
    await navigate('db2', '0x2')
    await useStore.getState().fileClosed('db2', db1Only)
    expect(useStore.getState().activeRecord?.editor_id).toBe('db1:0x3')
    await useStore.getState().goBack()
    expect(useStore.getState().activeRecord?.editor_id).toBe('db1:0x1')
  })

  it('drops a closed secondary file from a pending load', async () => {
    installApi((_, op, id) => (op === 'record' && id === 'db2' ? 30 : 0))
    const pending = useStore.getState().navigate('db1', '0x1')
    await tick()
    await useStore.getState().fileClosed('db2', db1Only)
    await pending
    const s = useStore.getState()
    expect(s.activeRecord?.editor_id).toBe('db1:0x1')
    expect(s.recordColumns.map((c) => c.dbId)).toEqual(['db1'])
  })

  it('leaves no way to navigate into the closed file', async () => {
    const api = installApi()
    const { navigate } = useStore.getState()
    await navigate('db1', '0x1')
    await useStore.getState().fileClosed('db2', db1Only)
    api.run.mockClear()
    await navigate('db2', '0x2')
    expect(useStore.getState().nav).toEqual({
      entries: [{ dbId: 'db1', formid: '0x1' }],
      index: 0,
    })
    expect(api.run).not.toHaveBeenCalled()
  })
})

describe('changeReferencedByDepth', () => {
  it('re-fetches the shown record’s refs at the new depth', async () => {
    installApi()
    await useStore.getState().navigate('db1', '0x1')
    await useStore.getState().changeReferencedByDepth(2)
    const s = useStore.getState()
    expect(s.referencedByDepth).toBe(2)
    expect(s.referencedBy).toEqual([{ form_id: 'db1-ref-d2' } as never])
  })

  it('a late depth-change response never lands on the next record', async () => {
    let slowRefs = false
    installApi((_, op) => (op === 'referenced_by' && slowRefs ? 30 : 0))
    await useStore.getState().navigate('db1', '0x1')
    slowRefs = true
    const late = useStore.getState().changeReferencedByDepth(2)
    slowRefs = false
    await useStore.getState().navigate('db2', '0x2')
    await late
    expect(useStore.getState().referencedBy).toEqual([{ form_id: 'db2-ref-d2' } as never])
  })

  it('the newest depth wins over a slower earlier one', async () => {
    let slowRefs = false
    installApi((_, op) => (op === 'referenced_by' && slowRefs ? 30 : 0))
    await useStore.getState().navigate('db1', '0x1')
    slowRefs = true
    const late = useStore.getState().changeReferencedByDepth(2)
    slowRefs = false
    await useStore.getState().changeReferencedByDepth(3)
    await late
    expect(useStore.getState().referencedBy).toEqual([{ form_id: 'db1-ref-d3' } as never])
  })
})

describe('loadRaw', () => {
  it('loads the shown record’s raw dump from its file', async () => {
    installApi()
    await useStore.getState().navigate('db2', '0x2')
    await useStore.getState().loadRaw()
    const { raw } = useStore.getState()
    expect(raw.loading).toBe(false)
    expect(raw.view?.subrecords[0].signature).toBe('db2:0x2')
  })

  it('a raw dump for an earlier record never lands under the next one', async () => {
    installApi((_, op) => (op === 'record_raw' ? 30 : 0))
    await useStore.getState().navigate('db1', '0x1')
    const late = useStore.getState().loadRaw()
    await useStore.getState().navigate('db2', '0x2')
    await late
    const s = useStore.getState()
    expect(s.activeRecord?.editor_id).toBe('db2:0x2')
    expect(s.raw).toEqual({ view: null, loading: false, error: null })
  })
})
