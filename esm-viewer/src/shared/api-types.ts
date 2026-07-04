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
} as const

export type DbId = string

export interface DbHandle {
  id: DbId
  path: string
  info: FileInfo
}

export interface FileInfo {
  path: string
  version: number
  record_count: number
  next_object_id: number
  author?: string
  description?: string
  masters: string[]
  flags: number
  is_esm: boolean
  is_localized: boolean
}

/** The interpreted label of a GRUP, decoded per its `group_type`. Discriminated by `kind`. */
export type GroupLabel =
  | { kind: 'record_type'; sig: string }
  | { kind: 'form_id'; form_id: string }
  | { kind: 'interior_block'; block: number }
  | { kind: 'exterior_block'; grid_y: number; grid_x: number }
  | { kind: 'cell_children'; cell: string }
  | { kind: 'raw'; label: number }

export interface GroupNode {
  label: GroupLabel
  group_type: number
  child_count: number
  /** Byte offset of this GRUP's 24-byte header in the file — used to descend further. */
  offset: number
}

/** A single direct child of a GRUP: either a nested group or a leaf record stub. */
export type GroupChild =
  | ({ node: 'group' } & GroupNode)
  | { node: 'record'; form_id: string; editor_id?: string; record_type: string; offset: number }

export interface RecordRow {
  form_id: string
  record_type?: string
  editor_id?: string
  name?: string
  offset: number
}

/** One intermediate hop on the path from the lookup target to a result record. */
export interface RefPathNode {
  form_id: string
  record_type?: string
  editor_id?: string
}

/** A referencer row returned by a recursive refs walk (referencedById). */
export interface RefRow extends RecordRow {
  record_type?: string
  /** Hop distance from the lookup target (1 = direct reference). */
  depth?: number
  /** Intermediate nodes between target and this row; absent/empty for depth-1 results. */
  path?: RefPathNode[]
}

/** Full envelope returned by a recursive refs walk — mirrors Rust `RefList`. */
export interface RefListResult {
  target: string
  rows: RefRow[]
  total: number
  capped: boolean
}

export interface FormIdStub {
  formid: string
  editor_id?: string
  record_type: string
}

export interface RecordResult {
  header: {
    signature: string
    form_id: string
    flags: number
    form_version: number
    data_size: number
    offset: number
  }
  editor_id?: string
  fields: Record<string, unknown>
}

/** Result envelope for `filterTypeRecords` — mirrors Rust `FilterResult`. */
export interface FilterResult {
  rows: RecordRow[]
  /** Total matches found within the scanned set (may exceed rows.length if `limit` truncated). */
  matched: number
  /** How many records of this type were actually decoded and tested. */
  scanned: number
  /** Total records of this type that exist in the file. */
  total: number
  /** True if rows.length < matched (the match list itself was truncated by `limit`). */
  capped: boolean
  /** True if scanned < total (the decode pass itself stopped at the internal scan cap). */
  scan_capped: boolean
}

export type FilterOp = 'exists' | 'eq' | 'contains' | 'gt' | 'lt' | 'gte' | 'lte'

export interface RawSubrecordView {
  signature: string
  size: number
  hex: string
}

export interface RawRecordView {
  header: RecordResult['header']
  subrecords: RawSubrecordView[]
}

export interface Markers {
  unknown_record: number
  raw_fallback: number
  unmapped: number
  unresolved: number
  records: number
}

export interface CoverageReport {
  by_type: Record<string, Markers>
  totals: Markers
}

export interface Fo76Api {
  openFileDialog(): Promise<string | null>
  openFolderDialog(): Promise<string | null>
  openDatabase(path: string): Promise<DbHandle>
  closeDatabase(id: DbId): Promise<void>
  listOpen(): Promise<DbHandle[]>
  fileInfo(id: DbId): Promise<FileInfo>
  listGroups(id: DbId): Promise<GroupNode[]>
  listTypeRecords(id: DbId, sig: string, offset: number, limit: number): Promise<RecordRow[]>
  recordByFormid(id: DbId, formid: string, resolve?: 'none' | 'stub' | 'full'): Promise<RecordResult>
  recordByEdid(id: DbId, edid: string, resolve?: 'none' | 'stub' | 'full'): Promise<RecordResult>
  recordById(id: DbId, target: string, resolve?: 'none' | 'stub' | 'full'): Promise<RecordResult>
  referencedBy(id: DbId, formid: string): Promise<RecordRow[]>
  referencedById(id: DbId, target: string, depth?: number): Promise<RefListResult>
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
}
