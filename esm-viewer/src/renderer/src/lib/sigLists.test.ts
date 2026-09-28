import { describe, it, expect } from 'bun:test'
import { listRecordTypeSigs, parseSigList } from './sigLists'
import type { GroupNode } from '../../../shared/api-types'
import { makeGroupNode, mockRun } from '../../../test-support/fixtures'

describe('parseSigList', () => {
  it('splits, trims, and uppercases comma-separated signatures', () => {
    expect(parseSigList('weap, armo')).toEqual(['WEAP', 'ARMO'])
  })

  it('drops empty entries from blank segments and trailing commas', () => {
    expect(parseSigList('weap,,armo,')).toEqual(['WEAP', 'ARMO'])
  })

  it('drops whitespace-only segments', () => {
    expect(parseSigList('weap,   ,armo')).toEqual(['WEAP', 'ARMO'])
  })

  it('returns an empty array for blank input', () => {
    expect(parseSigList('')).toEqual([])
    expect(parseSigList('   ')).toEqual([])
  })
})

describe('listRecordTypeSigs', () => {
  it('keeps only record_type groups with at least one child, sorted', async () => {
    const groups: GroupNode[] = [
      makeGroupNode('WEAP', 5),
      makeGroupNode('ARMO', 3),
      { group_type: 0, label: { kind: 'form_id', form_id: '0x01' }, child_count: 10, offset: 0 },
    ]
    const api = mockRun(async () => groups)

    const result = await listRecordTypeSigs(api, 'db1')

    expect(result).toEqual(['ARMO', 'WEAP'])
    expect(api.run).toHaveBeenCalledWith('db1', { op: 'list_groups' })
  })

  it('drops record_type groups with zero children', async () => {
    const groups: GroupNode[] = [makeGroupNode('WEAP', 0), makeGroupNode('ARMO', 1)]
    const api = mockRun(async () => groups)

    const result = await listRecordTypeSigs(api, 'db1')

    expect(result).toEqual(['ARMO'])
  })

  it('returns an empty array when there are no matching groups', async () => {
    const api = mockRun(async () => [])

    const result = await listRecordTypeSigs(api, 'db1')

    expect(result).toEqual([])
  })
})
