import { describe, it, expect } from 'vitest'
import {
  validateResolve,
  validateSig,
  validateSigArray,
  validateSearchField,
  validateBodies,
  validateFilterOp,
  validateOptionalText,
  validateUint,
  validateTarget,
} from './ipc-validators'

describe('validateResolve', () => {
  it.each(['none', 'stub', 'full'] as const)('passes %s through unchanged', (v) => {
    expect(validateResolve(v)).toBe(v)
  })

  it('throws with a sensible message on an invalid value', () => {
    expect(() => validateResolve('bogus')).toThrow(
      'invalid resolve value: expected none|stub|full, got bogus'
    )
    expect(() => validateResolve(undefined)).toThrow(/invalid resolve value/)
  })
})

describe('validateSig', () => {
  it.each(['WEAP', 'NPC_', 'A', 'AB12', '0000'])('passes valid signature %s through unchanged', (v) => {
    expect(validateSig(v)).toBe(v)
  })

  it('throws on a lowercase signature', () => {
    expect(() => validateSig('weap')).toThrow(/invalid record signature/)
  })

  it('throws on a signature longer than 4 characters', () => {
    expect(() => validateSig('WEAPS')).toThrow(/invalid record signature/)
  })

  it('throws on a non-string value', () => {
    expect(() => validateSig(123)).toThrow('invalid record signature: 123')
  })
})

describe('validateSigArray', () => {
  it('validates every element and passes the array through', () => {
    expect(validateSigArray(['WEAP', 'ARMO'])).toEqual(['WEAP', 'ARMO'])
  })

  it('passes an empty array through', () => {
    expect(validateSigArray([])).toEqual([])
  })

  it('throws when given a non-array', () => {
    expect(() => validateSigArray('WEAP')).toThrow('invalid record signature list: WEAP')
  })

  it('throws when any element is an invalid signature', () => {
    expect(() => validateSigArray(['WEAP', 'nope'])).toThrow(/invalid record signature/)
  })
})

describe('validateSearchField', () => {
  it.each(['edid', 'name', 'both'] as const)('passes %s through unchanged', (v) => {
    expect(validateSearchField(v)).toBe(v)
  })

  it('throws on an invalid field', () => {
    expect(() => validateSearchField('description')).toThrow(
      'invalid search field: expected edid|name|both, got description'
    )
  })
})

describe('validateBodies', () => {
  it.each(['none', 'stub', 'full'] as const)('passes %s through unchanged', (v) => {
    expect(validateBodies(v)).toBe(v)
  })

  it('throws on an invalid value', () => {
    expect(() => validateBodies('everything')).toThrow(/invalid bodies value/)
  })
})

describe('validateFilterOp', () => {
  it.each(['exists', 'eq', 'contains', 'gt', 'lt', 'gte', 'lte'] as const)(
    'passes %s through unchanged',
    (v) => {
      expect(validateFilterOp(v)).toBe(v)
    }
  )

  it('throws on an invalid op', () => {
    expect(() => validateFilterOp('neq')).toThrow(
      'invalid filter op: expected exists|eq|contains|gt|lt|gte|lte, got neq'
    )
  })
})

describe('validateOptionalText', () => {
  it('passes a string within the max length through unchanged', () => {
    expect(validateOptionalText('pattern', 'hello', 512)).toBe('hello')
  })

  it('returns undefined for undefined', () => {
    expect(validateOptionalText('pattern', undefined)).toBeUndefined()
  })

  it('returns undefined for null', () => {
    expect(validateOptionalText('pattern', null)).toBeUndefined()
  })

  it('uses a default max of 512', () => {
    expect(validateOptionalText('pattern', 'x'.repeat(512))).toBe('x'.repeat(512))
    expect(() => validateOptionalText('pattern', 'x'.repeat(513))).toThrow(
      'invalid pattern: must be a string of length <= 512'
    )
  })

  it('throws on a string exceeding a custom max', () => {
    expect(() => validateOptionalText('recordType', 'ABCDE', 4)).toThrow(
      'invalid recordType: must be a string of length <= 4'
    )
  })

  it('throws on a non-string, non-nullish value', () => {
    expect(() => validateOptionalText('pattern', 42)).toThrow(
      'invalid pattern: must be a string of length <= 512'
    )
  })
})

describe('validateUint', () => {
  it('passes a valid non-negative integer through unchanged', () => {
    expect(validateUint('offset', 0)).toBe(0)
    expect(validateUint('offset', 42)).toBe(42)
  })

  it('uses a default max of 100_000', () => {
    expect(validateUint('limit', 100_000)).toBe(100_000)
    expect(() => validateUint('limit', 100_001)).toThrow(
      'invalid limit: expected integer 0–100000, got 100001'
    )
  })

  it('respects a custom max', () => {
    expect(validateUint('groupOffset', 123, Number.MAX_SAFE_INTEGER)).toBe(123)
  })

  it('throws on a negative number', () => {
    expect(() => validateUint('offset', -1)).toThrow(/invalid offset/)
  })

  it('throws on a non-integer number', () => {
    expect(() => validateUint('offset', 1.5)).toThrow(/invalid offset/)
  })

  it('throws on a non-number value', () => {
    expect(() => validateUint('offset', '5')).toThrow('invalid offset: expected integer 0–100000, got 5')
  })
})

describe('validateTarget', () => {
  it('passes a non-empty string through unchanged', () => {
    expect(validateTarget('0x00012345')).toBe('0x00012345')
  })

  it('throws on an empty string', () => {
    expect(() => validateTarget('')).toThrow('invalid target: must be a non-empty string')
  })

  it('throws on a string longer than 512 characters', () => {
    expect(() => validateTarget('x'.repeat(513))).toThrow('invalid target: must be a non-empty string')
  })

  it('throws on a non-string value', () => {
    expect(() => validateTarget(123)).toThrow('invalid target: must be a non-empty string')
  })
})
