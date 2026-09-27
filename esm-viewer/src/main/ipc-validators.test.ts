import { describe, it, expect } from 'bun:test'
import {
  validateResolve,
  validateSig,
  validateSigArray,
  validateSearchField,
  validateBodies,
  validateFilterOp,
  validateOptionalText,
  validateUint,
  validateRefDepth,
  validateTarget,
} from './ipc-validators'

// Four validators share one shape: an allow-list of literal values passed
// through unchanged, plus one throw naming the whole allow-list. Each row
// spells out its expected message in full rather than templating it — that
// text is user-facing, and a shared template would stop catching a regression
// in any single validator's wording.
type EnumCase = [
  name: string,
  validate: (v: unknown) => string,
  valid: string[],
  invalid: string,
  message: string,
]

const enumCases: EnumCase[] = [
  [
    'validateResolve',
    validateResolve,
    ['none', 'stub', 'full'],
    'bogus',
    'invalid resolve value: expected none|stub|full, got bogus',
  ],
  [
    'validateSearchField',
    validateSearchField,
    ['edid', 'name', 'both'],
    'description',
    'invalid search field: expected edid|name|both, got description',
  ],
  [
    'validateBodies',
    validateBodies,
    ['none', 'stub', 'full'],
    'everything',
    'invalid bodies value: expected none|stub|full, got everything',
  ],
  [
    'validateFilterOp',
    validateFilterOp,
    ['exists', 'eq', 'contains', 'gt', 'lt', 'gte', 'lte'],
    'neq',
    'invalid filter op: expected exists|eq|contains|gt|lt|gte|lte, got neq',
  ],
]

describe.each(enumCases)('%s', (_name, validate, valid, invalid, message) => {
  it.each(valid)('passes %s through unchanged', (v) => {
    expect(validate(v)).toBe(v)
  })

  it('throws naming the full allow-list on an invalid value', () => {
    expect(() => validate(invalid)).toThrow(message)
  })
})

describe('validateResolve (beyond the shared enum shape)', () => {
  it('throws when the value is missing entirely', () => {
    expect(() => validateResolve(undefined)).toThrow(/invalid resolve value/)
  })
})

describe('validateSig', () => {
  it.each(['WEAP', 'NPC_', 'A', 'AB12', '0000'])(
    'passes valid signature %s through unchanged',
    (v) => {
      expect(validateSig(v)).toBe(v)
    },
  )

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
      'invalid pattern: must be a string of length <= 512',
    )
  })

  it('throws on a string exceeding a custom max', () => {
    expect(() => validateOptionalText('recordType', 'ABCDE', 4)).toThrow(
      'invalid recordType: must be a string of length <= 4',
    )
  })

  it('throws on a non-string, non-nullish value', () => {
    expect(() => validateOptionalText('pattern', 42)).toThrow(
      'invalid pattern: must be a string of length <= 512',
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
      'invalid limit: expected integer 0–100000, got 100001',
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
    expect(() => validateUint('offset', '5')).toThrow(
      'invalid offset: expected integer 0–100000, got 5',
    )
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
    expect(() => validateTarget('x'.repeat(513))).toThrow(
      'invalid target: must be a non-empty string',
    )
  })

  it('throws on a non-string value', () => {
    expect(() => validateTarget(123)).toThrow('invalid target: must be a non-empty string')
  })
})

describe('validateRefDepth', () => {
  it('defaults an absent depth to one hop', () => {
    expect(validateRefDepth(undefined)).toBe(1)
    expect(validateRefDepth(null)).toBe(1)
  })

  it('clamps integers into 1..=6', () => {
    expect(validateRefDepth(0)).toBe(1)
    expect(validateRefDepth(3)).toBe(3)
    expect(validateRefDepth(99)).toBe(6)
  })

  it('rejects non-integers instead of letting them reach the addon as an unbounded walk', () => {
    for (const bad of ['3', 2.5, Number.NaN, {}]) {
      expect(() => validateRefDepth(bad)).toThrow(/invalid depth/)
    }
  })
})
