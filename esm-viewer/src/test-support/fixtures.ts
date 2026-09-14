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
// This module must NOT import `src/main/db-registry` (directly or
// transitively): `ipc.test.ts` calls `mock.module('./db-registry', …)`, which
// Bun applies process-globally and cannot undo, so anything importing it from
// here would inherit the mocked copy depending on file evaluation order.
// `ipc.test.ts`'s `makeSpyDb` and `db-registry.test.ts`'s `fakeDb` stay local
// to those files for the same reason.

import { vi, type Mock } from 'bun:test'
import type {
  DbHandle,
  Fo76Api,
  GroupChild,
  GroupNode,
  RecordResult,
  RecordRow,
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

/** A one-method fake `Fo76Api` whose single method is a spy. The result is
 * assignable to the `Pick<Fo76Api, …>` slice each module under test asks for,
 * and the method is typed as a `Mock` so tests can queue return values and
 * assert calls on it directly. Pass `impl` for a fixed behaviour, or omit it
 * and drive the spy with `mockResolvedValueOnce`. */
export function mockApi<K extends keyof Fo76Api>(
  method: K,
  impl?: Fo76Api[K],
): { [P in K]: Mock<Fo76Api[P]> } {
  return { [method]: impl ? vi.fn(impl) : vi.fn() } as unknown as {
    [P in K]: Mock<Fo76Api[P]>
  }
}
