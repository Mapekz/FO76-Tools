import { describe, it, expect } from 'vitest'
import { compareRows, sortRows, type SortState } from './recordSort'
import type { RecordRow } from '../../../shared/api-types'

function makeRow(formId: string, editorId: string | null, name: string | null): RecordRow {
  return { form_id: formId, record_type: 'WEAP', editor_id: editorId, name, offset: 0 }
}

describe('sortRows', () => {
  it('sorts form_id ascending as fixed-width hex strings', () => {
    const rows = [makeRow('0x00000010', null, null), makeRow('0x00000002', null, null)]
    const sort: SortState = { column: 'form_id', direction: 'asc' }
    expect(sortRows(rows, sort).map((r) => r.form_id)).toEqual(['0x00000002', '0x00000010'])
  })

  it('sorts form_id descending', () => {
    const rows = [makeRow('0x00000002', null, null), makeRow('0x00000010', null, null)]
    const sort: SortState = { column: 'form_id', direction: 'desc' }
    expect(sortRows(rows, sort).map((r) => r.form_id)).toEqual(['0x00000010', '0x00000002'])
  })

  it('sorts editor_id, pushing null to the end regardless of direction', () => {
    const rows = [
      makeRow('0x1', 'Zebra', null),
      makeRow('0x2', null, null),
      makeRow('0x3', 'Apple', null),
    ]
    const asc: SortState = { column: 'editor_id', direction: 'asc' }
    expect(sortRows(rows, asc).map((r) => r.editor_id)).toEqual(['Apple', 'Zebra', null])

    const desc: SortState = { column: 'editor_id', direction: 'desc' }
    expect(sortRows(rows, desc).map((r) => r.editor_id)).toEqual(['Zebra', 'Apple', null])
  })

  it('sorts name, pushing empty string to the end regardless of direction', () => {
    const rows = [
      makeRow('0x1', null, 'Banana'),
      makeRow('0x2', null, ''),
      makeRow('0x3', null, 'Apple'),
    ]
    const asc: SortState = { column: 'name', direction: 'asc' }
    expect(sortRows(rows, asc).map((r) => r.name)).toEqual(['Apple', 'Banana', ''])

    const desc: SortState = { column: 'name', direction: 'desc' }
    expect(sortRows(rows, desc).map((r) => r.name)).toEqual(['Banana', 'Apple', ''])
  })

  it('does not mutate the input array', () => {
    const rows = [makeRow('0x00000010', null, null), makeRow('0x00000002', null, null)]
    const sort: SortState = { column: 'form_id', direction: 'asc' }
    sortRows(rows, sort)
    expect(rows.map((r) => r.form_id)).toEqual(['0x00000010', '0x00000002'])
  })

  it('treats equal values as equal', () => {
    const a = makeRow('0x1', 'Same', 'Same')
    const b = makeRow('0x2', 'Same', 'Same')
    expect(compareRows(a, b, { column: 'editor_id', direction: 'asc' })).toBe(0)
  })
})
