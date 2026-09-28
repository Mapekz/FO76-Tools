import { describe, it, expect, vi, type Mock } from 'bun:test'
import { loadAllTypeRecords, loadGroupChildrenPage, loadTypeChildrenPage } from './recordLoad'
import type { GroupChild, RecordRow, RunnableOp } from '../../../shared/api-types'
import { makeGroupChild, makeRow, mockRun } from '../../../test-support/fixtures'

// Every suite here drives the ARMO record-type group, so its rows/children are
// ARMO ones — passed explicitly rather than inherited from the shared
// builders' defaults.
function armoRow(formId: string): RecordRow {
  return makeRow({ form_id: formId, record_type: 'ARMO' })
}

function armoChild(formId: string): GroupChild {
  return makeGroupChild({ form_id: formId, record_type: 'ARMO' })
}

describe('loadAllTypeRecords', () => {
  it('accumulates multiple chunks and reports progress after each one', async () => {
    const chunk1 = [armoRow('0x01'), armoRow('0x02')]
    const chunk2 = [armoRow('0x03')]
    const api = mockRun()
    api.run.mockResolvedValueOnce(chunk1).mockResolvedValueOnce(chunk2)

    const onChunk = vi.fn<(accumulated: RecordRow[]) => void>()
    await loadAllTypeRecords(api, 'db1', 'ARMO', 3, 2, onChunk)

    expect(api.run).toHaveBeenNthCalledWith(1, 'db1', {
      op: 'list_type_records',
      sig: 'ARMO',
      offset: 0,
      limit: 2,
    })
    expect(api.run).toHaveBeenNthCalledWith(2, 'db1', {
      op: 'list_type_records',
      sig: 'ARMO',
      offset: 2,
      limit: 2,
    })
    expect(onChunk).toHaveBeenNthCalledWith(1, chunk1)
    expect(onChunk).toHaveBeenNthCalledWith(2, [...chunk1, ...chunk2])
  })

  it('stops once the accumulated offset reaches total', async () => {
    const chunk = [armoRow('0x01'), armoRow('0x02')]
    const api = mockRun()
    api.run.mockResolvedValueOnce(chunk)

    const onChunk = vi.fn<(accumulated: RecordRow[]) => void>()
    await loadAllTypeRecords(api, 'db1', 'ARMO', 2, 2000, onChunk)

    expect(api.run).toHaveBeenCalledTimes(1)
    expect(onChunk).toHaveBeenCalledTimes(1)
    expect(onChunk).toHaveBeenCalledWith(chunk)
  })

  it('breaks defensively on an empty chunk instead of looping forever', async () => {
    const api = mockRun()
    api.run.mockResolvedValueOnce([])

    const onChunk = vi.fn<(accumulated: RecordRow[]) => void>()
    await loadAllTypeRecords(api, 'db1', 'ARMO', 100, 10, onChunk)

    expect(api.run).toHaveBeenCalledTimes(1)
    expect(onChunk).not.toHaveBeenCalled()
  })
})

// `loadTypeChildrenPage` and `loadGroupChildrenPage` are one paging algorithm
// over two different keys — a record-type signature vs a GRUP's byte offset —
// reached through two different ops. Each row wires its own `run` mock so the
// shared body stays key-agnostic.
type PageCase = [
  name: string,
  op: (offset: number, limit: number) => RunnableOp,
  setup: () => {
    spy: Mock<(...args: never[]) => Promise<unknown>>
    fetch: (current: GroupChild[], pageSize: number) => Promise<GroupChild[]>
  },
]

const pageCases: PageCase[] = [
  [
    'loadTypeChildrenPage',
    (offset, limit) => ({ op: 'list_type_children', sig: 'WRLD', offset, limit }),
    () => {
      const api = mockRun()
      return {
        spy: api.run,
        fetch: (current, pageSize) => loadTypeChildrenPage(api, 'db1', 'WRLD', current, pageSize),
      }
    },
  ],
  [
    'loadGroupChildrenPage',
    (offset, limit) => ({ op: 'list_group_children', group_offset: 4096, offset, limit }),
    () => {
      const api = mockRun()
      return {
        spy: api.run,
        fetch: (current, pageSize) => loadGroupChildrenPage(api, 'db1', 4096, current, pageSize),
      }
    },
  ],
]

describe.each(pageCases)('%s', (_name, op, setup) => {
  it('fetches the first page when current is empty', async () => {
    const page1 = [armoChild('0x01')]
    const { spy, fetch } = setup()
    spy.mockResolvedValueOnce(page1)

    const result = await fetch([], 100)

    expect(spy).toHaveBeenCalledWith('db1', op(0, 100))
    expect(result).toEqual(page1)
  })

  it('appends the next page after the current offset', async () => {
    const current = [armoChild('0x01'), armoChild('0x02')]
    const page2 = [armoChild('0x03')]
    const { spy, fetch } = setup()
    spy.mockResolvedValueOnce(page2)

    const result = await fetch(current, 2)

    expect(spy).toHaveBeenCalledWith('db1', op(2, 2))
    expect(result).toEqual([...current, ...page2])
  })
})
