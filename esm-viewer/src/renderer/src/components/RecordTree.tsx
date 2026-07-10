import React, { useEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { useStore } from '../store'
import type { RecordRow, GroupChild, GroupLabel } from '../../../shared/api-types'
import { formatRecordType } from '../recordTypeNames'
import { sortRows, type SortColumn, type SortState } from '../lib/recordSort'

const PAGE_SIZE = 100

/** Top-level GRUP types that get true hierarchical descent instead of a flat record list. */
const HIERARCHICAL = new Set(['WRLD', 'CELL'])

/** Flat per-type table layout: shared by the header and every virtualized row
 * so columns line up. */
const COLUMN_TEMPLATE = '95px 1fr 1fr'
const ROW_HEIGHT = 22
/** Viewport cap (in rows) before a group's table gets its own inner scrollbar —
 * below this, the viewport is sized exactly to content (no virtualization overhead visible). */
const MAX_VISIBLE_ROWS = 15
/** Auto-load-all fetch chunk size. `listTypeRecords` blocks Electron's main
 * process for the duration of each call, so this must stay small enough that
 * one call doesn't freeze the app — bigger than old PAGE_SIZE=100 is fine since
 * nothing renders per-page anymore. Tune against real large record types. */
const CHUNK_SIZE = 2000

interface Props {
  onNavigate: (dbId: string, formid: string) => void
}

interface GroupEntry {
  sig: string
  child_count: number
}

function groupLabelText(label: GroupLabel): string {
  switch (label.kind) {
    case 'record_type':
      return formatRecordType(label.sig)
    case 'form_id':
      return `World ${label.form_id}`
    case 'cell_children':
      return `Cell Children (${label.cell})`
    case 'interior_block':
      return `Block ${label.block}`
    case 'exterior_block':
      return `Block (${label.grid_x}, ${label.grid_y})`
    case 'raw':
      return `Group ${label.label}`
  }
}

/** Recursive node for the WRLD/CELL hierarchical subtree: a group descends
 * further via `listGroupChildren`, a record is a clickable leaf. */
function GroupChildNode({
  child,
  dbId,
  onNavigate,
}: {
  child: GroupChild
  dbId: string
  onNavigate: (dbId: string, formid: string) => void
}) {
  const [expanded, setExpanded] = useState(false)
  const [children, setChildren] = useState<GroupChild[] | null>(null)
  const [loading, setLoading] = useState(false)

  if (child.node === 'record') {
    return (
      <div
        onClick={() => onNavigate(dbId, child.form_id)}
        style={{ padding: '2px 6px', cursor: 'pointer' }}
      >
        <span style={{ fontFamily: 'monospace', color: '#7ec8e3' }}>{child.form_id}</span>{' '}
        <span style={{ color: '#aaa' }}>[{child.record_type}]</span>{' '}
        {child.editor_id && <span>{child.editor_id}</span>}
      </div>
    )
  }

  async function toggle() {
    if (expanded) {
      setExpanded(false)
      return
    }
    setExpanded(true)
    if (children) return
    setLoading(true)
    try {
      const result = await window.api.listGroupChildren(dbId, child.offset, 0, PAGE_SIZE)
      setChildren(result)
    } finally {
      setLoading(false)
    }
  }

  async function loadMore() {
    const current = children ?? []
    const next = await window.api.listGroupChildren(dbId, child.offset, current.length, PAGE_SIZE)
    setChildren([...current, ...next])
  }

  return (
    <div style={{ paddingLeft: 8 }}>
      <div onClick={() => void toggle()} style={{ padding: '2px 6px', cursor: 'pointer' }}>
        {expanded ? '▼' : '▶'} {groupLabelText(child.label)} ({child.child_count})
      </div>
      {expanded && (
        <div style={{ paddingLeft: 8 }}>
          {loading && <div style={{ padding: 4 }}>Loading…</div>}
          {(children ?? []).map((c, i) => (
            <GroupChildNode
              key={c.node === 'group' ? c.offset : `${c.form_id}-${i}`}
              child={c}
              dbId={dbId}
              onNavigate={onNavigate}
            />
          ))}
          {(children?.length ?? 0) < child.child_count && (
            <button onClick={() => void loadMore()} style={{ margin: 4, fontSize: 11 }}>
              Load more…
            </button>
          )}
        </div>
      )}
    </div>
  )
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
        const children = await window.api.listTypeChildren(activeDbId, sig, 0, PAGE_SIZE)
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
      let offset = 0
      let acc: RecordRow[] = []
      while (offset < total) {
        const chunk = await window.api.listTypeRecords(activeDbId, sig, offset, CHUNK_SIZE)
        if (chunk.length === 0) break // defensive: avoid an infinite loop on a short backend response
        acc = acc.concat(chunk)
        offset += chunk.length
        setRows((r) => ({ ...r, [sig]: acc }))
      }
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
    const next = await window.api.listTypeChildren(activeDbId, sig, current.length, PAGE_SIZE)
    setGroupChildren((c) => ({ ...c, [sig]: [...current, ...next] }))
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
                background: isFocused ? '#33395a' : '#1e1e2e',
                borderLeft: isFocused ? '2px solid #7ec8e3' : '2px solid transparent',
                borderBottom: '1px solid #333',
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

const HEADER_CELL_STYLE: React.CSSProperties = { padding: '2px 6px', textAlign: 'left', cursor: 'pointer' }
const BODY_CELL_STYLE: React.CSSProperties = { padding: '2px 6px', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }

function sortIndicator(column: SortColumn, sortState: SortState | undefined): string {
  if (sortState?.column !== column) return ''
  return sortState.direction === 'asc' ? ' ▲' : ' ▼'
}

/** Virtualized, sortable, click-to-navigate table for one record type's flat
 * row list. Rows are rendered as CSS-Grid `<div>`s rather than a native
 * `<table>`/`<tr>` because `@tanstack/react-virtual` positions items via
 * `transform: translateY()` on absolutely-positioned elements, which native
 * table row layout does not support. */
function RecordTypeTable({
  rows,
  sortState,
  onSortChange,
  focusedFormId,
  activeDbId,
  onNavigate,
}: {
  rows: RecordRow[]
  sortState: SortState | undefined
  onSortChange: (column: SortColumn) => void
  focusedFormId: string | null
  activeDbId: string | null
  onNavigate: (dbId: string, formid: string) => void
}) {
  const parentRef = useRef<HTMLDivElement>(null)
  const rowVirtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_HEIGHT,
    getItemKey: (i) => rows[i].form_id,
    overscan: 8,
  })

  useEffect(() => {
    if (focusedFormId == null) return
    const idx = rows.findIndex((r) => r.form_id === focusedFormId)
    if (idx >= 0) rowVirtualizer.scrollToIndex(idx, { align: 'auto' })
  }, [focusedFormId, rows, rowVirtualizer])

  const viewportHeight = Math.min(rows.length, MAX_VISIBLE_ROWS) * ROW_HEIGHT

  return (
    <div style={{ fontSize: 11 }}>
      <div
        style={{
          display: 'grid',
          gridTemplateColumns: COLUMN_TEMPLATE,
          background: '#16213e',
        }}
      >
        <div style={HEADER_CELL_STYLE} onClick={() => onSortChange('form_id')}>
          FormID{sortIndicator('form_id', sortState)}
        </div>
        <div style={HEADER_CELL_STYLE} onClick={() => onSortChange('editor_id')}>
          EditorID{sortIndicator('editor_id', sortState)}
        </div>
        <div style={HEADER_CELL_STYLE} onClick={() => onSortChange('name')}>
          Name{sortIndicator('name', sortState)}
        </div>
      </div>
      <div ref={parentRef} style={{ height: viewportHeight, overflow: 'auto', position: 'relative' }}>
        <div style={{ height: rowVirtualizer.getTotalSize(), position: 'relative' }}>
          {rowVirtualizer.getVirtualItems().map((vi) => {
            const row = rows[vi.index]
            const rowFocused = row.form_id === focusedFormId
            return (
              <div
                key={vi.key}
                onClick={() => activeDbId && onNavigate(activeDbId, row.form_id)}
                style={{
                  position: 'absolute',
                  top: 0,
                  left: 0,
                  right: 0,
                  transform: `translateY(${vi.start}px)`,
                  height: ROW_HEIGHT,
                  display: 'grid',
                  gridTemplateColumns: COLUMN_TEMPLATE,
                  cursor: 'pointer',
                  borderBottom: '1px solid #222',
                  background: rowFocused ? '#33395a' : undefined,
                }}
              >
                <div style={{ ...BODY_CELL_STYLE, fontFamily: 'monospace' }}>{row.form_id}</div>
                <div style={BODY_CELL_STYLE}>{row.editor_id ?? ''}</div>
                <div style={BODY_CELL_STYLE}>{row.name ?? ''}</div>
              </div>
            )
          })}
        </div>
      </div>
    </div>
  )
}
