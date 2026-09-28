import { create } from 'zustand'
import {
  sel,
  type DbHandle,
  type RawRecordView,
  type RecordResult,
  type RefRow,
  type RefListResult,
} from '../../shared/api-types'
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

export interface RawState {
  view: RawRecordView | null
  loading: boolean
  error: string | null
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
  /** Why the shown record's Referenced By lookup failed; the record stays shown. */
  referencedByError: string | null
  /** The shown record's raw dump, fetched on demand by `loadRaw`. */
  raw: RawState
  nav: { entries: NavEntry[]; index: number }

  setOpenDbs: (dbs: DbHandle[]) => void
  /** Show a navigated-to record. The selection carries its file: `dbId`
   * becomes the active file, so the tree, raw view and Referenced By all
   * read the file the record came from. Referenced By and the raw dump empty
   * until the record's own arrive. */
  showRecord: (dbId: string, record: RecordResult, columns: RecordColumn[]) => void
  /** Make `dbId` the active file (a file-list click, or a file just
   * opened). Supersedes any pending load. The shown record follows: its copy
   * in `dbId` loads, entering history once visible, when that file has it;
   * otherwise the record view clears. */
  selectFile: (dbId: string) => Promise<void>
  /** After `closeDatabase(id)`: drop the file, its column and its history
   * entries, and drop its part of any pending load. When the history cursor
   * lands on another entry, or the shown record came from `id`, that entry
   * loads. `remaining` is the files still open. */
  fileClosed: (id: string, remaining: DbHandle[]) => Promise<void>
  setReferencedBy: (result: RefListResult) => void
  /** Re-fetch the shown record's Referenced By at depth `d`. */
  changeReferencedByDepth: (d: number) => Promise<void>
  /** Fetch the shown record's raw dump into `raw`. A dump arriving after the
   * shown record changed is dropped. */
  loadRaw: () => Promise<void>
  navPush: (entry: NavEntry) => void
  navBack: () => NavEntry | null
  navForward: () => NavEntry | null
  navCurrent: () => NavEntry | null

  /** A new navigation choice (tree row, FormID link, search, diff or refs
   * row): load `formid` from `dbId`, entering history once it shows, so a
   * failed or superseded navigation leaves history alone. A `dbId` that is
   * not open is ignored. */
  navigate: (dbId: string, formid: string) => Promise<void>
  /** Step history back or forward and load that entry from its own file. */
  goBack: () => Promise<void>
  goForward: () => Promise<void>
}

const NO_REFS = {
  referencedBy: [],
  referencedByTotal: 0,
  referencedByCapped: false,
  referencedByError: null,
}
const NO_RAW: RawState = { view: null, loading: false, error: null }
const NO_RECORD = { activeRecord: null, recordColumns: [], ...NO_REFS, raw: NO_RAW }

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

export const useStore = create<AppStore>((set, get) => {
  // Out-of-order-response guard: rapid navigation can start several loads
  // before earlier ones resolve. Each load captures its own sequence number
  // and bails before touching state if a later load has since started.
  let loadSeq = 0
  // The same guard for Referenced By: a record load and a depth change can
  // both have a lookup in flight; only the newest lands.
  let refsSeq = 0

  const isOpen = (dbId: string) => get().openDbs.some((db) => db.id === dbId)
  // Whether `record` (one load's result object) from `dbId` is still shown.
  const isShown = (dbId: string, record: RecordResult) =>
    get().activeDbId === dbId && get().activeRecord === record

  async function loadRefs(dbId: string, record: RecordResult): Promise<void> {
    const seq = ++refsSeq
    const depth = get().referencedByDepth
    try {
      const refs = await fetchReferencedBy(dbId, record.header.form_id, depth, window.api)
      if (seq === refsSeq && isShown(dbId, record)) get().setReferencedBy(refs)
    } catch (e) {
      console.error('referenced by error:', e)
      if (seq === refsSeq && isShown(dbId, record)) {
        set({ ...NO_REFS, referencedByError: errorMessage(e) })
      }
    }
  }

  // Fetch and show a record, with one xEdit-style column per open file that
  // contains the resolved FormID, then fetch its Referenced By. `onShown`
  // runs as the record becomes visible. The outcome reflects the record
  // alone: a failed refs lookup still leaves it 'shown'. A file closed while
  // the load is pending drops out of the response; when it is `dbId` itself
  // the load is superseded.
  async function loadRecord(
    dbId: string,
    target: string,
    onShown?: () => void,
  ): Promise<'shown' | 'failed' | 'superseded'> {
    const seq = ++loadSeq
    let loaded
    try {
      loaded = await buildRecordColumns(target, dbId, get().openDbs, window.api)
    } catch (e) {
      console.error('load record error:', e)
      return seq === loadSeq && isOpen(dbId) ? 'failed' : 'superseded'
    }
    if (seq !== loadSeq || !isOpen(dbId)) return 'superseded'
    const columns = loaded.columns.filter((c) => isOpen(c.dbId))
    get().showRecord(dbId, loaded.active, columns)
    onShown?.()
    await loadRefs(dbId, loaded.active)
    return 'shown'
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
    referencedByError: null,
    raw: NO_RAW,
    nav: { entries: [], index: -1 },

    setOpenDbs: (dbs) => set({ openDbs: dbs }),
    showRecord: (dbId, record, columns) =>
      set({
        activeDbId: dbId,
        activeRecord: record,
        recordColumns: columns,
        ...NO_REFS,
        raw: NO_RAW,
      }),

    selectFile: async (dbId) => {
      const shown = get().activeRecord
      if (!shown) {
        loadSeq++ // supersede a load still pending for another file
        set({ activeDbId: dbId })
        return
      }
      const formid = shown.header.form_id
      const outcome = await loadRecord(dbId, formid, () => get().navPush({ dbId, formid }))
      if (outcome === 'failed') set({ activeDbId: dbId, ...NO_RECORD })
    },

    fileClosed: async (id, remaining) => {
      const { activeDbId, recordColumns, nav } = get()
      const kept = nav.entries.filter((e) => e.dbId !== id)
      const removedThroughIndex = nav.entries
        .slice(0, nav.index + 1)
        .filter((e) => e.dbId === id).length
      const index = kept.length ? Math.max(nav.index - removedThroughIndex, 0) : -1
      // The history cursor always names what is shown: when closing moves the
      // cursor off its entry, or takes the shown record's file, the entry now
      // under the cursor loads rather than leaving a blank view that Back and
      // Forward step around.
      const cursorEntry = kept[index]
      const reload =
        cursorEntry && (cursorEntry !== nav.entries[nav.index] || activeDbId === id)
          ? cursorEntry
          : null
      set({
        openDbs: remaining,
        recordColumns: recordColumns.filter((c) => c.dbId !== id),
        nav: { entries: kept, index },
        ...(activeDbId === id
          ? { activeDbId: reload?.dbId ?? remaining[0]?.id ?? null, ...NO_RECORD }
          : {}),
      })
      if (reload) {
        const outcome = await loadRecord(reload.dbId, reload.formid)
        if (outcome === 'failed') set({ activeDbId: reload.dbId, ...NO_RECORD })
      }
    },
    setReferencedBy: (result) =>
      set({
        referencedBy: result.rows,
        referencedByTotal: result.total,
        referencedByCapped: result.capped,
        referencedByError: null,
      }),
    changeReferencedByDepth: async (d) => {
      set({ referencedByDepth: d })
      const { activeDbId, activeRecord } = get()
      if (activeDbId && activeRecord) await loadRefs(activeDbId, activeRecord)
    },

    loadRaw: async () => {
      const { activeDbId: dbId, activeRecord: record } = get()
      if (!dbId || !record) return
      set({ raw: { view: null, loading: true, error: null } })
      try {
        const view = await window.api.run(dbId, {
          op: 'record_raw',
          sel: sel(record.header.form_id),
        })
        if (isShown(dbId, record)) set({ raw: { view, loading: false, error: null } })
      } catch (e) {
        if (isShown(dbId, record))
          set({ raw: { view: null, loading: false, error: errorMessage(e) } })
      }
    },

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
      if (!isOpen(dbId)) return
      await loadRecord(dbId, formid, () => get().navPush({ dbId, formid }))
    },

    // Back and Forward also cancel a navigation still pending: it has not
    // entered history, so stepping is relative to what is shown.
    goBack: async () => {
      loadSeq++
      const entry = get().navBack()
      if (entry) await loadRecord(entry.dbId, entry.formid)
    },

    goForward: async () => {
      loadSeq++
      const entry = get().navForward()
      if (entry) await loadRecord(entry.dbId, entry.formid)
    },
  }
})
