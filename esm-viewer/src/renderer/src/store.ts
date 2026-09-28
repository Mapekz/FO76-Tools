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
  setActiveDb: (id: string | null) => void
  setActiveRecord: (r: RecordResult | null) => void
  setRecordColumns: (cols: RecordColumn[]) => void
  /** Show a navigated-to record. The selection carries its file: `dbId`
   * becomes the active file, so the tree, raw view and Referenced By all
   * read the file the record came from. */
  showRecord: (dbId: string, record: RecordResult, columns: RecordColumn[]) => void
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

export const useStore = create<AppStore>((set, get) => {
  // Out-of-order-response guard: rapid navigation can start several loads
  // before earlier ones resolve. Each load captures its own sequence number
  // and bails before touching state if a later load has since started.
  let loadSeq = 0

  // Fetch and show a record without touching history, with one xEdit-style
  // column per open file that contains the resolved FormID.
  async function loadRecord(dbId: string, target: string): Promise<void> {
    const seq = ++loadSeq
    try {
      const { openDbs, referencedByDepth } = get()
      const { active, columns } = await buildRecordColumns(target, dbId, openDbs, window.api)
      if (seq !== loadSeq) return
      get().showRecord(dbId, active, columns)

      const refs = await fetchReferencedBy(dbId, target, referencedByDepth, window.api)
      if (seq !== loadSeq) return
      get().setReferencedBy(refs)
    } catch (e) {
      console.error('load record error:', e)
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
    setActiveDb: (id) => set({ activeDbId: id }),
    setActiveRecord: (r) => set({ activeRecord: r }),
    setRecordColumns: (cols) => set({ recordColumns: cols }),
    showRecord: (dbId, record, columns) =>
      set({ activeDbId: dbId, activeRecord: record, recordColumns: columns }),
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
