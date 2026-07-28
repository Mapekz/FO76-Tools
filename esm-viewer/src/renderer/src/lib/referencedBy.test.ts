import { describe, it, expect, vi } from 'vitest'
import { fetchReferencedBy } from './referencedBy'
import type { RefListResult } from '../../../shared/api-types'

describe('fetchReferencedBy', () => {
  it('passes its arguments through to api.referencedById and returns its result', async () => {
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
    const api = { referencedById: vi.fn<() => Promise<RefListResult>>(async () => result) }

    const out = await fetchReferencedBy('db1', '0x00012345', 3, api)

    expect(out).toBe(result)
    expect(api.referencedById).toHaveBeenCalledWith('db1', '0x00012345', 3)
  })
})
