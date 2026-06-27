export const CH = {
  openFileDialog: 'open-file-dialog',
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
  editor_id?: string
  name?: string
  offset: number
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
  openDatabase(path: string): Promise<DbHandle>
  closeDatabase(id: DbId): Promise<void>
  listOpen(): Promise<DbHandle[]>
  fileInfo(id: DbId): Promise<FileInfo>
  listGroups(id: DbId): Promise<GroupNode[]>
  listTypeRecords(id: DbId, sig: string, offset: number, limit: number): Promise<RecordRow[]>
  recordByFormid(id: DbId, formid: string, resolve?: string): Promise<RecordResult>
  recordByEdid(id: DbId, edid: string): Promise<RecordResult>
  recordById(id: DbId, target: string, resolve?: string): Promise<RecordResult>
  referencedBy(id: DbId, formid: string): Promise<RecordRow[]>
  referencedById(id: DbId, target: string): Promise<RecordRow[]>
  parseFormId(s: string): Promise<string>
}
