import { describe, it, expect } from 'bun:test'
import { compareRows, sortRows, type SortState } from './recordSort'
import type { RecordRow } from '../../../shared/api-types'
import { makeRow } from '../../../test-support/fixtures'

// These suites sort WEAP rows; `record_type` is pinned here rather than left
// to the shared builder's default, so changing that default can't silently
// change what the sort is exercised against.
function weapRow(formId: string, editorId: string | null, name: string | null): RecordRow {
  return makeRow({ form_id: formId, record_type: 'WEAP', editor_id: editorId, name })
}

describe('sortRows', () => {
  it('sorts form_id ascending as fixed-width hex strings', () => {
    const rows = [weapRow('0x00000010', null, null), weapRow('0x00000002', null, null)]
    const sort: SortState = { column: 'form_id', direction: 'asc' }
    expect(sortRows(rows, sort).map((r) => r.form_id)).toEqual(['0x00000002', '0x00000010'])
  })

  it('sorts form_id descending', () => {
    const rows = [weapRow('0x00000002', null, null), weapRow('0x00000010', null, null)]
    const sort: SortState = { column: 'form_id', direction: 'desc' }
    expect(sortRows(rows, sort).map((r) => r.form_id)).toEqual(['0x00000010', '0x00000002'])
  })

  it('sorts editor_id, pushing null to the end regardless of direction', () => {
    const rows = [
      weapRow('0x1', 'Zebra', null),
      weapRow('0x2', null, null),
      weapRow('0x3', 'Apple', null),
    ]
    const asc: SortState = { column: 'editor_id', direction: 'asc' }
    expect(sortRows(rows, asc).map((r) => r.editor_id)).toEqual(['Apple', 'Zebra', null])

    const desc: SortState = { column: 'editor_id', direction: 'desc' }
    expect(sortRows(rows, desc).map((r) => r.editor_id)).toEqual(['Zebra', 'Apple', null])
  })

  it('sorts name, pushing empty string to the end regardless of direction', () => {
    const rows = [
      weapRow('0x1', null, 'Banana'),
      weapRow('0x2', null, ''),
      weapRow('0x3', null, 'Apple'),
    ]
    const asc: SortState = { column: 'name', direction: 'asc' }
    expect(sortRows(rows, asc).map((r) => r.name)).toEqual(['Apple', 'Banana', ''])

    const desc: SortState = { column: 'name', direction: 'desc' }
    expect(sortRows(rows, desc).map((r) => r.name)).toEqual(['Banana', 'Apple', ''])
  })

  it('does not mutate the input array', () => {
    const rows = [weapRow('0x00000010', null, null), weapRow('0x00000002', null, null)]
    const sort: SortState = { column: 'form_id', direction: 'asc' }
    sortRows(rows, sort)
    expect(rows.map((r) => r.form_id)).toEqual(['0x00000010', '0x00000002'])
  })

  it('treats equal values as equal', () => {
    const a = weapRow('0x1', 'Same', 'Same')
    const b = weapRow('0x2', 'Same', 'Same')
    expect(compareRows(a, b, { column: 'editor_id', direction: 'asc' })).toBe(0)
  })
})
