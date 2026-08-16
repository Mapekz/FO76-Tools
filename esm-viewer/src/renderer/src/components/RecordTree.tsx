import React, { useEffect, useMemo, useState } from 'react'
import { useStore } from '../store'
import type { RecordRow, GroupChild } from '../../../shared/api-types'
import { formatRecordType } from '../recordTypeNames'
import { sortRows, type SortColumn, type SortState } from '../lib/recordSort'
import { loadAllTypeRecords, loadTypeChildrenPage } from '../lib/recordLoad'
import { colors } from '../theme'
import { GroupChildNode, PAGE_SIZE } from './GroupChildNode'
import { RecordTypeTable } from './RecordTypeTable'

/** Top-level GRUP types that get true hierarchical descent instead of a flat record list. */
const HIERARCHICAL = new Set(['WRLD', 'CELL'])

/** Auto-load-all fetch chunk size. `listTypeRecords` blocks Electron's main
 * process for the duration of each call, so this must stay small enough that
 * one call doesn't freeze the app. Tune against real large record types. */
const CHUNK_SIZE = 2000

interface Props {
  onNavigate: (dbId: string, formid: string) => void
}

interface GroupEntry {
  sig: string
  child_count: number
}

type FocusRow = { kind: 'group'; sig: string } | { kind: 'record'; row: RecordRow }

export function RecordTree({ onNavigate }: Props) {
  const { activeDbId } = useStore()
  const [groups, setGroups] = useState<GroupEntry[]>([])
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const [rows, setRows] = useState<Record<string, RecordRow[]>>({})
  const [groupChildren, setGroupChildren] = useState<Record<string, GroupChild[]>>({})
  const [loading, setLoading] = useState<Set<string>>(new Set())
  const [focusedIndex, setFocusedIndex] = useState(0)
  const [sortStateBySig, setSortStateBySig] = useState<Record<string, SortState>>({})

  useEffect(() => {
    if (!activeDbId) {
      setGroups([])
      return
    }
    window.api
      .listGroups(activeDbId)
      .then((gs) => {
        const parsed: GroupEntry[] = gs.map((g) => {
          const sig = g.label.kind === 'record_type' ? g.label.sig : '????'
          return { sig, child_count: g.child_count }
        })
        const filtered = parsed.filter((g) => g.child_count > 0)
        filtered.sort((a, b) => a.sig.localeCompare(b.sig))
        setGroups(filtered)
        setExpanded(new Set())
        setRows({})
        setGroupChildren({})
        setFocusedIndex(0)
      })
      .catch(console.error)
  }, [activeDbId])

  async function toggleGroup(sig: string) {
    if (expanded.has(sig)) {
      setExpanded((s) => {
        const n = new Set(s)
        n.delete(sig)
        return n
      })
      return
    }
    setExpanded((s) => new Set([...s, sig]))
    if (!activeDbId) return

    if (HIERARCHICAL.has(sig)) {
      if (groupChildren[sig]) return
      setLoading((s) => new Set([...s, sig]))
      try {
        const children = await loadTypeChildrenPage(window.api, activeDbId, sig, [], PAGE_SIZE)
        setGroupChildren((c) => ({ ...c, [sig]: children }))
      } finally {
        setLoading((s) => {
          const n = new Set(s)
          n.delete(sig)
          return n
        })
      }
      return
    }

    if (rows[sig]) return // already loaded (or loading)
    const total = groups.find((g) => g.sig === sig)?.child_count ?? 0
    void loadAllRecords(sig, total)
  }

  /** Auto-loads every record of `sig` in the background, fetching in bounded
   * chunks so no single IPC round-trip blocks Electron's main process for too
   * long. Fire-and-forget: expanding a group returns immediately and this
   * keeps running (and isn't cancelled) even if the group is collapsed again. */
  async function loadAllRecords(sig: string, total: number) {
    if (!activeDbId) return
    setRows((r) => ({ ...r, [sig]: [] })) // arm "already loading" guard; shows "0 / total" immediately
    setLoading((s) => new Set([...s, sig]))
    try {
      await loadAllTypeRecords(window.api, activeDbId, sig, total, CHUNK_SIZE, (acc) => {
        setRows((r) => ({ ...r, [sig]: acc }))
      })
    } catch (err) {
      console.error(err)
    } finally {
      setLoading((s) => {
        const n = new Set(s)
        n.delete(sig)
        return n
      })
    }
  }

  async function loadMore(sig: string) {
    if (!activeDbId) return
    const current = groupChildren[sig] ?? []
    const next = await loadTypeChildrenPage(window.api, activeDbId, sig, current, PAGE_SIZE)
    setGroupChildren((c) => ({ ...c, [sig]: next }))
  }

  function handleSortClick(sig: string, column: SortColumn) {
    setSortStateBySig((prev) => {
      const cur = prev[sig]
      const next: SortState =
        cur?.column === column
          ? { column, direction: cur.direction === 'asc' ? 'desc' : 'asc' } // same column: flip
          : { column, direction: 'asc' } // new column: ascending
      return { ...prev, [sig]: next }
    })
  }

  // Sorted view of each expanded flat group's rows. Deferred while a group is
  // still loading (raw arrival order is already FormID-ascending) to avoid
  // re-sorting the whole array on every auto-load chunk.
  const sortedRowsBySig = useMemo(() => {
    const out: Record<string, RecordRow[]> = {}
    for (const g of groups) {
      if (HIERARCHICAL.has(g.sig) || !expanded.has(g.sig)) continue
      const list = rows[g.sig] ?? []
      const sort = sortStateBySig[g.sig]
      out[g.sig] = !sort || loading.has(g.sig) ? list : sortRows(list, sort)
    }
    return out
  }, [groups, expanded, rows, loading, sortStateBySig])

  // Flat "focusable rows" model for keyboard navigation: top-level groups, plus
  // (for non-hierarchical expanded groups only) their loaded record rows, in
  // the same order the table visually renders (post-sort).
  const focusRows: FocusRow[] = []
  for (const g of groups) {
    focusRows.push({ kind: 'group', sig: g.sig })
    if (expanded.has(g.sig) && !HIERARCHICAL.has(g.sig)) {
      for (const row of sortedRowsBySig[g.sig] ?? []) {
        focusRows.push({ kind: 'record', row })
      }
    }
  }

  const focusedRow = focusRows[focusedIndex]
  const focusedFormId = focusedRow?.kind === 'record' ? focusedRow.row.form_id : null

  useEffect(() => {
    if (focusedIndex >= focusRows.length) {
      setFocusedIndex(Math.max(0, focusRows.length - 1))
    }
  }, [focusRows.length, focusedIndex])

  function handleKeyDown(e: React.KeyboardEvent<HTMLDivElement>) {
    if (focusRows.length === 0) return
    const fr = focusRows[focusedIndex]
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault()
        setFocusedIndex((i) => Math.min(i + 1, focusRows.length - 1))
        break
      case 'ArrowUp':
        e.preventDefault()
        setFocusedIndex((i) => Math.max(i - 1, 0))
        break
      case 'ArrowRight':
        if (fr?.kind === 'group' && !expanded.has(fr.sig)) {
          e.preventDefault()
          void toggleGroup(fr.sig)
        }
        break
      case 'ArrowLeft':
        if (fr?.kind === 'group' && expanded.has(fr.sig)) {
          e.preventDefault()
          void toggleGroup(fr.sig)
        }
        break
      case 'Enter':
      case ' ':
        if (fr) {
          e.preventDefault()
          if (fr.kind === 'record') {
            if (activeDbId) onNavigate(activeDbId, fr.row.form_id)
          } else {
            void toggleGroup(fr.sig)
          }
        }
        break
      default:
        break
    }
  }

  return (
    <div
      tabIndex={0}
      onKeyDown={handleKeyDown}
      style={{ overflowY: 'auto', flex: 1, fontSize: 12, outline: 'none' }}
    >
      {groups.map((g) => {
        const focusIdx = focusRows.findIndex((fr) => fr.kind === 'group' && fr.sig === g.sig)
        const isFocused = focusIdx === focusedIndex
        return (
          <div key={g.sig}>
            <div
              onClick={() => void toggleGroup(g.sig)}
              style={{
                padding: '3px 8px',
                cursor: 'pointer',
                background: isFocused ? colors.focusIndigo : colors.rowSlate,
                borderLeft: isFocused ? `2px solid ${colors.traceBlue}` : '2px solid transparent',
                borderBottom: `1px solid ${colors.rule}`,
              }}
            >
              {expanded.has(g.sig) ? '▼' : '▶'} {formatRecordType(g.sig)} ({g.child_count})
            </div>
            {expanded.has(g.sig) && (
              <div>
                {loading.has(g.sig) && (
                  <div style={{ padding: 4 }}>
                    {HIERARCHICAL.has(g.sig)
                      ? 'Loading…'
                      : `Loading… ${(rows[g.sig]?.length ?? 0).toLocaleString()} / ${g.child_count.toLocaleString()}`}
                  </div>
                )}
                {HIERARCHICAL.has(g.sig) ? (
                  activeDbId && (
                    <div style={{ paddingLeft: 8 }}>
                      {(groupChildren[g.sig] ?? []).map((child, i) => (
                        <GroupChildNode
                          key={child.node === 'group' ? child.offset : `${child.form_id}-${i}`}
                          child={child}
                          dbId={activeDbId}
                          onNavigate={onNavigate}
                        />
                      ))}
                      {(groupChildren[g.sig]?.length ?? 0) < g.child_count && (
                        <button
                          onClick={() => void loadMore(g.sig)}
                          style={{ margin: 4, fontSize: 11 }}
                        >
                          Load more…
                        </button>
                      )}
                    </div>
                  )
                ) : (
                  <RecordTypeTable
                    rows={sortedRowsBySig[g.sig] ?? []}
                    sortState={sortStateBySig[g.sig]}
                    onSortChange={(column) => handleSortClick(g.sig, column)}
                    focusedFormId={focusedFormId}
                    activeDbId={activeDbId}
                    onNavigate={onNavigate}
                  />
                )}
              </div>
            )}
          </div>
        )
      })}
    </div>
  )
}
