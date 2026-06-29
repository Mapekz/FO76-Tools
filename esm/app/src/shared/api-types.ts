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

export interface GroupNode {
  label: Record<string, unknown>
  group_type: number
  child_count: number
}

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
  recordByEdid(id: DbId, edid: string): Promise<RecordResult>
  recordById(id: DbId, target: string, resolve?: 'none' | 'stub' | 'full'): Promise<RecordResult>
  referencedBy(id: DbId, formid: string): Promise<RecordRow[]>
  referencedById(id: DbId, target: string, depth?: number): Promise<RefRow[]>
  parseFormId(s: string): Promise<string>
}
