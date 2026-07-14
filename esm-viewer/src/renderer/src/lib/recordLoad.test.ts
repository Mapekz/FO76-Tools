import { describe, it, expect, vi } from 'vitest'
import { loadAllTypeRecords, loadGroupChildrenPage, loadTypeChildrenPage } from './recordLoad'
import type { GroupChild, RecordRow } from '../../../shared/api-types'

function makeRow(formId: string): RecordRow {
  return { form_id: formId, record_type: 'ARMO', editor_id: null, name: null, offset: 0 }
}

function makeRecordChild(formId: string): GroupChild {
  return { node: 'record', form_id: formId, record_type: 'ARMO', editor_id: null, offset: 0 }
}

describe('loadAllTypeRecords', () => {
  it('accumulates multiple chunks and reports progress after each one', async () => {
    const chunk1 = [makeRow('0x01'), makeRow('0x02')]
    const chunk2 = [makeRow('0x03')]
    const listTypeRecords = vi
      .fn()
      .mockResolvedValueOnce(chunk1)
      .mockResolvedValueOnce(chunk2)
    const api = { listTypeRecords }

    const onChunk = vi.fn()
    await loadAllTypeRecords(api, 'db1', 'ARMO', 3, 2, onChunk)

    expect(listTypeRecords).toHaveBeenNthCalledWith(1, 'db1', 'ARMO', 0, 2)
    expect(listTypeRecords).toHaveBeenNthCalledWith(2, 'db1', 'ARMO', 2, 2)
    expect(onChunk).toHaveBeenNthCalledWith(1, chunk1)
    expect(onChunk).toHaveBeenNthCalledWith(2, [...chunk1, ...chunk2])
  })

  it('stops once the accumulated offset reaches total', async () => {
    const chunk = [makeRow('0x01'), makeRow('0x02')]
    const listTypeRecords = vi.fn().mockResolvedValueOnce(chunk)
    const api = { listTypeRecords }

    const onChunk = vi.fn()
    await loadAllTypeRecords(api, 'db1', 'ARMO', 2, 2000, onChunk)

    expect(listTypeRecords).toHaveBeenCalledTimes(1)
    expect(onChunk).toHaveBeenCalledTimes(1)
    expect(onChunk).toHaveBeenCalledWith(chunk)
  })

  it('breaks defensively on an empty chunk instead of looping forever', async () => {
    const listTypeRecords = vi.fn().mockResolvedValueOnce([])
    const api = { listTypeRecords }

    const onChunk = vi.fn()
    await loadAllTypeRecords(api, 'db1', 'ARMO', 100, 10, onChunk)

    expect(listTypeRecords).toHaveBeenCalledTimes(1)
    expect(onChunk).not.toHaveBeenCalled()
  })
})

describe('loadTypeChildrenPage', () => {
  it('fetches the first page when current is empty', async () => {
    const page1 = [makeRecordChild('0x01')]
    const listTypeChildren = vi.fn().mockResolvedValueOnce(page1)
    const api = { listTypeChildren }

    const result = await loadTypeChildrenPage(api, 'db1', 'WRLD', [], 100)

    expect(listTypeChildren).toHaveBeenCalledWith('db1', 'WRLD', 0, 100)
    expect(result).toEqual(page1)
  })

  it('appends the next page after the current offset', async () => {
    const current = [makeRecordChild('0x01'), makeRecordChild('0x02')]
    const page2 = [makeRecordChild('0x03')]
    const listTypeChildren = vi.fn().mockResolvedValueOnce(page2)
    const api = { listTypeChildren }

    const result = await loadTypeChildrenPage(api, 'db1', 'WRLD', current, 2)

    expect(listTypeChildren).toHaveBeenCalledWith('db1', 'WRLD', 2, 2)
    expect(result).toEqual([...current, ...page2])
  })
})

describe('loadGroupChildrenPage', () => {
  it('fetches the first page keyed by the group offset when current is empty', async () => {
    const page1 = [makeRecordChild('0x01')]
    const listGroupChildren = vi.fn().mockResolvedValueOnce(page1)
    const api = { listGroupChildren }

    const result = await loadGroupChildrenPage(api, 'db1', 4096, [], 100)

    expect(listGroupChildren).toHaveBeenCalledWith('db1', 4096, 0, 100)
    expect(result).toEqual(page1)
  })

  it('appends the next page after the current offset', async () => {
    const current = [makeRecordChild('0x01')]
    const page2 = [makeRecordChild('0x02'), makeRecordChild('0x03')]
    const listGroupChildren = vi.fn().mockResolvedValueOnce(page2)
    const api = { listGroupChildren }

    const result = await loadGroupChildrenPage(api, 'db1', 4096, current, 1)

    expect(listGroupChildren).toHaveBeenCalledWith('db1', 4096, 1, 1)
    expect(result).toEqual([...current, ...page2])
  })
})
