import { describe, it, expect, vi } from 'bun:test'
import { listRecordTypeSigs, parseSigList } from './sigLists'
import type { GroupNode } from '../../../shared/api-types'

function makeGroup(sig: string, childCount: number): GroupNode {
  return { group_type: 0, label: { kind: 'record_type', sig }, child_count: childCount, offset: 0 }
}

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
      makeGroup('WEAP', 5),
      makeGroup('ARMO', 3),
      { group_type: 0, label: { kind: 'form_id', form_id: '0x01' }, child_count: 10, offset: 0 },
    ]
    const listGroups = vi.fn<(id: string) => Promise<GroupNode[]>>(async () => groups)

    const result = await listRecordTypeSigs({ listGroups }, 'db1')

    expect(result).toEqual(['ARMO', 'WEAP'])
    expect(listGroups).toHaveBeenCalledWith('db1')
  })

  it('drops record_type groups with zero children', async () => {
    const groups: GroupNode[] = [makeGroup('WEAP', 0), makeGroup('ARMO', 1)]
    const listGroups = vi.fn<(id: string) => Promise<GroupNode[]>>(async () => groups)

    const result = await listRecordTypeSigs({ listGroups }, 'db1')

    expect(result).toEqual(['ARMO'])
  })

  it('returns an empty array when there are no matching groups', async () => {
    const listGroups = vi.fn<(id: string) => Promise<GroupNode[]>>(async () => [])

    const result = await listRecordTypeSigs({ listGroups }, 'db1')

    expect(result).toEqual([])
  })
})
