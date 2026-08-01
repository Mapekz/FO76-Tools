import { createRequire } from 'module'
import type { EsmDatabase as RawEsmDatabase } from '@fo76/esm-napi'
import type {
  CoverageReport,
  DiffResult,
  FileInfo,
  FilterResult,
  GroupChild,
  GroupNode,
  RawRecordView,
  RecordResult,
  RecordRow,
  RefListResult,
} from '../shared/api-types'

/**
 * Compile-time facade over the N-API `EsmDatabase` class: same method
 * signatures as `@fo76/esm-napi`'s generated `index.d.ts`, but with each
 * JSON-returning method typed to the matching `ts-rs` DTO instead of `any`.
 * Record *bodies* (`RecordResult.fields`) stay `Record<string, unknown>` —
 * those are schema-driven at runtime and intentionally untyped.
 */
export type TypedEsmDatabase = Omit<
  RawEsmDatabase,
  | 'fileInfo'
  | 'listGroups'
  | 'listTypeRecords'
  | 'search'
  | 'filterTypeRecords'
  | 'listTypeFieldPaths'
  | 'listTypeChildren'
  | 'listGroupChildren'
  | 'recordByFormid'
  | 'recordByEdid'
  | 'recordById'
  | 'referencedById'
  | 'recordRaw'
  | 'coverageReport'
  | 'diff'
> & {
  fileInfo(): FileInfo
  listGroups(): GroupNode[]
  listTypeRecords(sig: string, offset: number, limit: number): RecordRow[]
  search(pattern: string, types: Array<string>, field: string, limit: number): RecordRow[]
  filterTypeRecords(
    sig: string,
    path: string | undefined | null,
    op: string,
    value: string | undefined | null,
    limit: number,
  ): FilterResult
  listTypeFieldPaths(sig: string): string[]
  listTypeChildren(sig: string, offset: number, limit: number): GroupChild[]
  listGroupChildren(groupOffset: number, offset: number, limit: number): GroupChild[]
  recordByFormid(formid: string, resolve: string): RecordResult
  recordByEdid(edid: string, resolve: string): Promise<RecordResult>
  recordById(id: string, resolve: string): Promise<RecordResult>
  referencedById(id: string, depth?: number | undefined | null): Promise<RefListResult>
  recordRaw(id: string): Promise<RawRecordView>
  coverageReport(recordType: string | undefined | null, sample: number): Promise<CoverageReport>
  diff(
    other: RawEsmDatabase,
    recordType: string | undefined | null,
    bodies: string,
    suppressNoise: boolean,
    excludeTypes: Array<string>,
  ): Promise<DiffResult>
}

const require = createRequire(import.meta.url)
const napi = require('@fo76/esm-napi') as {
  EsmDatabase: {
    openDatabase(path: string): Promise<TypedEsmDatabase>
  }
  parseFormId: (s: string) => string
}
export { napi }
/** Typed alias — prefer this (or `TypedEsmDatabase`) over the raw N-API `any` returns. */
export type EsmDatabase = TypedEsmDatabase
