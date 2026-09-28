import { describe, it, expect } from 'bun:test'
import { fetchReferencedBy } from './referencedBy'
import type { RefListResult } from '../../../shared/api-types'
import { mockRun } from '../../../test-support/fixtures'

describe('fetchReferencedBy', () => {
  it('runs an unlimited referenced_by walk at the given depth and returns its result', async () => {
    const result: RefListResult = {
      target: '0x00012345',
      rows: [],
      total: 0,
      capped: false,
      requested_depth: 0,
      effective_depth: null,
      depth_capped: false,
      frontier_remaining: 0,
      per_depth_totals: [],
      shown_max_depth: 0,
    }
    const api = mockRun(async () => result)

    const out = await fetchReferencedBy('db1', '0x00012345', 3, api)

    expect(out).toBe(result)
    expect(api.run).toHaveBeenCalledWith('db1', {
      op: 'referenced_by',
      sel: { kind: 'auto', value: '0x00012345' },
      limit: 0,
      depth: 3,
      type_filter: null,
      paths: false,
      sort: 'formid',
    })
  })
})
