// TypeScript mirrors of the Rust N-API DTOs, generated from `esm`'s structs via
// `ts-rs` (plus two hand-written generators for schema/marker data) — see
// `esm/justfile`'s `gen-types` recipe and `esm/CLAUDE.md`. Do NOT hand-edit
// anything under `./generated/`; regenerate it there instead.
//
// This file re-exports the generated types under their existing external names
// (aliasing where the Rust struct name collides with another, or where the
// TS-facing name predates this generation and downstream code depends on it),
// and keeps `CH` (Electron IPC channel names) and `Fo76Api` (the preload
// bridge contract) hand-written — those aren't Rust types, they're the IPC
// contract layered on top of them.
export type { FileInfo } from './generated/FileInfo'
export type { GroupLabel } from './generated/GroupLabel'
export type { GroupNode } from './generated/GroupNode'
export type { GroupChild } from './generated/GroupChild'
export type { RecordRow } from './generated/RecordRow'
export type { RefPathNode } from './generated/RefPathNode'
export type { RefRow } from './generated/RefRow'
/** Full envelope returned by a recursive refs walk — generated Rust name is `RefList`. */
export type { RefList as RefListResult } from './generated/RefList'
export type { FormIdStub } from './generated/FormIdStub'
export type { RecordHeaderInfo } from './generated/RecordHeaderInfo'
export type { RecordResult } from './generated/RecordResult'
export type { FilterResult } from './generated/FilterResult'
export type { RawSubrecordView } from './generated/RawSubrecordView'
export type { RawRecordView } from './generated/RawRecordView'
export type { Markers } from './generated/Markers'
export type { CoverageReport } from './generated/CoverageReport'
/** Lightweight record identity for added/removed diff entries — generated Rust name is `RecordStub`
 * (renamed here to avoid colliding with the tree module's own record stub). */
export type { RecordStub as RecordStubDiff } from './generated/RecordStub'
/** One record present in both snapshots whose decoded fields differ — generated Rust name is `RecordDiff`. */
export type { RecordDiff as RecordChangeDiff } from './generated/RecordDiff'
/** Resolved display info for a FormID referenced in a diff — generated Rust name is `RefName`. */
export type { RefName as RefNameEntry } from './generated/RefName'
export type { DiffResult } from './generated/DiffResult'
export type { ResolveDepth } from './generated/ResolveDepth'

import type { FileInfo } from './generated/FileInfo'
import type { GroupChild } from './generated/GroupChild'
import type { GroupNode } from './generated/GroupNode'
import type { RecordRow } from './generated/RecordRow'
import type { RecordResult } from './generated/RecordResult'
import type { RefList } from './generated/RefList'
import type { RawRecordView } from './generated/RawRecordView'
import type { CoverageReport } from './generated/CoverageReport'
import type { DiffResult } from './generated/DiffResult'
import type { FilterResult } from './generated/FilterResult'
import type { ResolveDepth } from './generated/ResolveDepth'

export const CH = {
  openFileDialog: 'open-file-dialog',
  openFolderDialog: 'open-folder-dialog',
  openDatabase: 'open-database',
  closeDatabase: 'close-database',
  listOpen: 'list-open',
  fileInfo: 'file-info',
  listGroups: 'list-groups',
  listTypeRecords: 'list-type-records',
  recordByFormid: 'record-by-formid',
  recordByEdid: 'record-by-edid',
  recordById: 'record-by-id',
  referencedBy: 'referenced-by',
  referencedById: 'referenced-by-id',
  parseFormId: 'parse-form-id',
  listTypeChildren: 'list-type-children',
  listGroupChildren: 'list-group-children',
  search: 'search',
  filterTypeRecords: 'filter-type-records',
  listTypeFieldPaths: 'list-type-field-paths',
  recordRaw: 'record-raw',
  coverageReport: 'coverage-report',
  diff: 'diff',
} as const

export type DbId = string

export interface DbHandle {
  id: DbId
  path: string
  info: FileInfo
}

/** Not a generated type — this is a query-input enum, never part of a
 * decode-output DTO crossing the N-API boundary as JSON, so it stays
 * hand-written (mirrors Rust's `FilterOp`, used only to build a request). */
export type FilterOp = 'exists' | 'eq' | 'contains' | 'gt' | 'lt' | 'gte' | 'lte'

export interface Fo76Api {
  openFileDialog(): Promise<string | null>
  openFolderDialog(): Promise<string | null>
  openDatabase(path: string): Promise<DbHandle>
  closeDatabase(id: DbId): Promise<void>
  listOpen(): Promise<DbHandle[]>
  fileInfo(id: DbId): Promise<FileInfo>
  listGroups(id: DbId): Promise<GroupNode[]>
  listTypeRecords(id: DbId, sig: string, offset: number, limit: number): Promise<RecordRow[]>
  recordByFormid(id: DbId, formid: string, resolve?: ResolveDepth): Promise<RecordResult>
  recordByEdid(id: DbId, edid: string, resolve?: ResolveDepth): Promise<RecordResult>
  recordById(id: DbId, target: string, resolve?: ResolveDepth): Promise<RecordResult>
  referencedBy(id: DbId, formid: string): Promise<RecordRow[]>
  referencedById(id: DbId, target: string, depth?: number): Promise<RefList>
  parseFormId(s: string): Promise<string>
  listTypeChildren(id: DbId, sig: string, offset: number, limit: number): Promise<GroupChild[]>
  listGroupChildren(
    id: DbId,
    groupOffset: number,
    offset: number,
    limit: number
  ): Promise<GroupChild[]>
  search(
    id: DbId,
    pattern: string,
    types: string[],
    field: 'edid' | 'name' | 'both',
    limit: number
  ): Promise<RecordRow[]>
  filterTypeRecords(
    id: DbId,
    sig: string,
    path: string | undefined,
    op: FilterOp,
    value: string | undefined,
    limit: number
  ): Promise<FilterResult>
  listTypeFieldPaths(id: DbId, sig: string): Promise<string[]>
  recordRaw(id: DbId, target: string): Promise<RawRecordView>
  coverageReport(id: DbId, recordType: string | undefined, sample: number): Promise<CoverageReport>
  diff(
    oldId: DbId,
    newId: DbId,
    recordType: string | undefined,
    bodies: 'none' | 'stub' | 'full',
    suppressNoise: boolean,
    excludeTypes: string[]
  ): Promise<DiffResult>
}
