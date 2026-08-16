import { describe, it, expect } from 'bun:test'
import { formatRecordType } from './recordTypeNames'
import { RECORD_TYPE_NAMES } from '../../shared/generated/recordTypeNames.generated'

describe('formatRecordType', () => {
  it('renders "SIG - Name" for a signature with a known human name', () => {
    const [sig, name] = Object.entries(RECORD_TYPE_NAMES)[0]
    expect(formatRecordType(sig)).toBe(`${sig} - ${name}`)
  })

  it('falls back to the bare signature when no human name is known', () => {
    expect(formatRecordType('ZZZZ_NOT_A_REAL_SIG')).toBe('ZZZZ_NOT_A_REAL_SIG')
  })
})
