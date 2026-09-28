import { create } from 'zustand'
import type { DbHandle, RecordResult, RefRow, RefListResult } from '../../shared/api-types'
import { buildRecordColumns } from './lib/recordColumns'
import { fetchReferencedBy } from './lib/referencedBy'

export interface NavEntry {
  dbId: string
  formid: string
}

/** One xEdit-style value column in `RecordTable` — one per open file containing
 * the active FormID. `record` is null when the file doesn't have it (dropped
 * before reaching the store; see `buildRecordColumns`), kept nullable here
 * so `RecordTable`/`buildAlignedTree` don't have to assume otherwise. */
export interface RecordColumn {
  dbId: string
  fileName: string
  record: RecordResult | null
}

export interface AppStore {
  openDbs: DbHandle[]
  activeDbId: string | null
  activeRecord: RecordResult | null
  recordColumns: RecordColumn[]
  referencedBy: RefRow[]
  referencedByDepth: number
  referencedByTotal: number
  referencedByCapped: boolean
  nav: { entries: NavEntry[]; index: number }

  setOpenDbs: (dbs: DbHandle[]) => void
  /** Show a navigated-to record. The selection carries its file: `dbId`
   * becomes the active file, so the tree, raw view and Referenced By all
   * read the file the record came from. Referenced By empties until the
   * record's own refs arrive. */
  showRecord: (dbId: string, record: RecordResult, columns: RecordColumn[]) => void
  /** Make `dbId` the active file (a file-list click, or a file just
   * opened). The shown record follows: its copy in `dbId` loads when that
   * file has it; otherwise the record view clears. */
  selectFile: (dbId: string) => Promise<void>
  /** After `closeDatabase(id)`: drop the file, its column and its history
   * entries, abandon a load still pending for it, and clear the record view
   * when it showed that file. `remaining` is the files still open. */
  fileClosed: (id: string, remaining: DbHandle[]) => void
  setReferencedBy: (result: RefListResult) => void
  setReferencedByDepth: (d: number) => void
  navPush: (entry: NavEntry) => void
  navBack: () => NavEntry | null
  navForward: () => NavEntry | null
  navCurrent: () => NavEntry | null

  /** A new navigation choice (tree row, FormID link, search, diff or refs
   * row): push history, then load `formid` from `dbId`. */
  navigate: (dbId: string, formid: string) => Promise<void>
  /** Step history back or forward and load that entry from its own file. */
  goBack: () => Promise<void>
  goForward: () => Promise<void>
}

const NO_REFS = { referencedBy: [], referencedByTotal: 0, referencedByCapped: false }
const NO_RECORD = { activeRecord: null, recordColumns: [], ...NO_REFS }

export const useStore = create<AppStore>((set, get) => {
  // Out-of-order-response guard: rapid navigation can start several loads
  // before earlier ones resolve. Each load captures its own sequence number
  // and bails before touching state if a later load has since started.
  let loadSeq = 0
  // The file the newest load reads, so closing it can abandon that load.
  let pendingDbId: string | null = null

  // Fetch and show a record without touching history, with one xEdit-style
  // column per open file that contains the resolved FormID.
  async function loadRecord(
    dbId: string,
    target: string,
  ): Promise<'shown' | 'failed' | 'superseded'> {
    const seq = ++loadSeq
    pendingDbId = dbId
    try {
      const { active, columns } = await buildRecordColumns(target, dbId, get().openDbs, window.api)
      if (seq !== loadSeq) return 'superseded'
      get().showRecord(dbId, active, columns)

      const refs = await fetchReferencedBy(dbId, target, get().referencedByDepth, window.api)
      if (seq !== loadSeq) return 'superseded'
      get().setReferencedBy(refs)
      return 'shown'
    } catch (e) {
      console.error('load record error:', e)
      return seq === loadSeq ? 'failed' : 'superseded'
    } finally {
      if (seq === loadSeq) pendingDbId = null
    }
  }

  return {
    openDbs: [],
    activeDbId: null,
    activeRecord: null,
    recordColumns: [],
    referencedBy: [],
    referencedByDepth: 1,
    referencedByTotal: 0,
    referencedByCapped: false,
    nav: { entries: [], index: -1 },

    setOpenDbs: (dbs) => set({ openDbs: dbs }),
    showRecord: (dbId, record, columns) =>
      set({ activeDbId: dbId, activeRecord: record, recordColumns: columns, ...NO_REFS }),

    selectFile: async (dbId) => {
      const shown = get().activeRecord
      if (!shown) {
        set({ activeDbId: dbId })
        return
      }
      const formid = shown.header.form_id
      const outcome = await loadRecord(dbId, formid)
      if (outcome === 'shown') get().navPush({ dbId, formid })
      else if (outcome === 'failed') set({ activeDbId: dbId, ...NO_RECORD })
    },

    fileClosed: (id, remaining) => {
      if (pendingDbId === id) {
        loadSeq++
        pendingDbId = null
      }
      const { activeDbId, recordColumns, nav } = get()
      const kept = nav.entries.filter((e) => e.dbId !== id)
      const removedThroughIndex = nav.entries
        .slice(0, nav.index + 1)
        .filter((e) => e.dbId === id).length
      set({
        openDbs: remaining,
        recordColumns: recordColumns.filter((c) => c.dbId !== id),
        nav: {
          entries: kept,
          index: kept.length ? Math.max(nav.index - removedThroughIndex, 0) : -1,
        },
        ...(activeDbId === id ? { activeDbId: remaining[0]?.id ?? null, ...NO_RECORD } : {}),
      })
    },
    setReferencedBy: (result) =>
      set({
        referencedBy: result.rows,
        referencedByTotal: result.total,
        referencedByCapped: result.capped,
      }),
    setReferencedByDepth: (d) => set({ referencedByDepth: d }),

    navPush: (entry) =>
      set((s) => {
        // Re-selecting the current entry reloads it without growing history.
        const current = s.nav.entries[s.nav.index]
        if (current && current.dbId === entry.dbId && current.formid === entry.formid) return {}
        const before = s.nav.entries.slice(0, s.nav.index + 1)
        const entries = [...before, entry]
        return { nav: { entries, index: entries.length - 1 } }
      }),

    navBack: () => {
      const { nav } = get()
      if (nav.index <= 0) return null
      const newIndex = nav.index - 1
      set({ nav: { ...nav, index: newIndex } })
      return nav.entries[newIndex]
    },

    navForward: () => {
      const { nav } = get()
      if (nav.index >= nav.entries.length - 1) return null
      const newIndex = nav.index + 1
      set({ nav: { ...nav, index: newIndex } })
      return nav.entries[newIndex]
    },

    navCurrent: () => {
      const { nav } = get()
      return nav.entries[nav.index] ?? null
    },

    navigate: async (dbId, formid) => {
      get().navPush({ dbId, formid })
      await loadRecord(dbId, formid)
    },

    goBack: async () => {
      const entry = get().navBack()
      if (entry) await loadRecord(entry.dbId, entry.formid)
    },

    goForward: async () => {
      const entry = get().navForward()
      if (entry) await loadRecord(entry.dbId, entry.formid)
    },
  }
})
