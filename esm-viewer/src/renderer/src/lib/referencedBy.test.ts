import { describe, it, expect, vi } from 'vitest'
import { fetchReferencedBy } from './referencedBy'
import type { RefListResult } from '../../../shared/api-types'

describe('fetchReferencedBy', () => {
  it('passes its arguments through to api.referencedById and returns its result', async () => {
    const result: RefListResult = { target: '0x00012345', rows: [], total: 0, capped: false }
    const api = { referencedById: vi.fn(async () => result) }

    const out = await fetchReferencedBy('db1', '0x00012345', 3, api)

    expect(out).toBe(result)
    expect(api.referencedById).toHaveBeenCalledWith('db1', '0x00012345', 3)
  })
})
