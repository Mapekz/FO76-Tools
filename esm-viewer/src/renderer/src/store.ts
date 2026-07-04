import { create } from 'zustand'
import type { DbHandle, RecordRow, RecordResult } from '../../shared/api-types'

export interface NavEntry {
  dbId: string
  formid: string
}

export interface AppStore {
  openDbs: DbHandle[]
  activeDbId: string | null
  activeRecord: RecordResult | null
  referencedBy: RecordRow[]
  nav: { entries: NavEntry[]; index: number }

  setOpenDbs: (dbs: DbHandle[]) => void
  setActiveDb: (id: string | null) => void
  setActiveRecord: (r: RecordResult | null) => void
  setReferencedBy: (rows: RecordRow[]) => void
  navPush: (entry: NavEntry) => void
  navBack: () => NavEntry | null
  navForward: () => NavEntry | null
  navCurrent: () => NavEntry | null
}

export const useStore = create<AppStore>((set, get) => ({
  openDbs: [],
  activeDbId: null,
  activeRecord: null,
  referencedBy: [],
  nav: { entries: [], index: -1 },

  setOpenDbs: (dbs) => set({ openDbs: dbs }),
  setActiveDb: (id) => set({ activeDbId: id }),
  setActiveRecord: (r) => set({ activeRecord: r }),
  setReferencedBy: (rows) => set({ referencedBy: rows }),

  navPush: (entry) =>
    set((s) => {
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
}))
