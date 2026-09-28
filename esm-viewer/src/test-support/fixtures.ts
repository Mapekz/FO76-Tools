// Shared test data builders. Every builder takes a partial override rather
// than positional arguments, so files with different needs share one builder
// and each test states only the fields it actually exercises.
//
// Defaults are deliberately neutral: where a test depends on a particular
// value (e.g. `recordSort` sorts WEAP rows, `recordLoad`/`recordColumns` load
// ARMO ones) that call site passes `record_type` explicitly instead of
// relying on the default here — otherwise changing a default would silently
// change what those suites exercise.
//
import { vi, type Mock } from 'bun:test'
import type {
  DbHandle,
  DbId,
  Fo76Api,
  GroupChild,
  GroupNode,
  RecordResult,
  RecordRow,
  RunnableOp,
} from '../shared/api-types'

/** The `node: 'record'` half of the `GroupChild` union. */
type RecordChild = Extract<GroupChild, { node: 'record' }>

/** A resolved FormID reference as it appears inside a decoded `fields` tree —
 * `alignedTree`'s `isFormIdStub` shape, not a generated DTO. */
export interface FormIdStub {
  formid: string
  editor_id: string | null
  record_type: string
}

export function makeRow(over: Partial<RecordRow> = {}): RecordRow {
  return { form_id: '0x01', record_type: 'ARMO', editor_id: null, name: null, offset: 0, ...over }
}

export function makeGroupChild(over: Partial<RecordChild> = {}): GroupChild {
  return {
    node: 'record',
    form_id: '0x01',
    record_type: 'ARMO',
    editor_id: null,
    offset: 0,
    ...over,
  }
}

export function makeGroupNode(sig: string, childCount: number): GroupNode {
  return { group_type: 0, label: { kind: 'record_type', sig }, child_count: childCount, offset: 0 }
}

export function makeDbHandle(id: string, path: string): DbHandle {
  return {
    id,
    path,
    info: {
      path,
      version: 1,
      record_count: 0,
      next_object_id: 0,
      author: null,
      description: null,
      masters: [],
      flags: 0,
      is_esm: true,
      is_localized: false,
    },
  }
}

export function makeRecordResult(formId: string, over: Partial<RecordResult> = {}): RecordResult {
  return {
    header: {
      signature: 'ARMO',
      form_id: formId,
      flags: 0,
      form_version: 44,
      data_size: 0,
      offset: 0,
    },
    editor_id: null,
    fields: {},
    ...over,
  }
}

export function makeFormIdStub(over: Partial<FormIdStub> = {}): FormIdStub {
  return { formid: '0x1', editor_id: 'Foo', record_type: 'ARMO', ...over }
}

/** A fake `Fo76Api.run` spy, assignable to the `Pick<Fo76Api, 'run'>` each
 * module under test asks for. Pass `impl` for a fixed behaviour, or omit it
 * and queue results with `mockResolvedValueOnce`; assert on the `(id, op)`
 * pairs it was called with. */
export function mockRun(impl?: (id: DbId, op: RunnableOp) => Promise<unknown>): {
  run: Mock<(id: DbId, op: RunnableOp) => Promise<unknown>>
} & Pick<Fo76Api, 'run'> {
  const run = impl ? vi.fn(impl) : vi.fn<(id: DbId, op: RunnableOp) => Promise<unknown>>()
  return { run } as unknown as {
    run: Mock<(id: DbId, op: RunnableOp) => Promise<unknown>>
  } & Pick<Fo76Api, 'run'>
}
