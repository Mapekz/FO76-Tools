import React, { useEffect, useState } from 'react'
import { useStore } from '../store'
import type { RecordRow, GroupChild, GroupLabel } from '../../../shared/api-types'

const PAGE_SIZE = 100

/** Top-level GRUP types that get true hierarchical descent instead of a flat record list. */
const HIERARCHICAL = new Set(['WRLD', 'CELL'])

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
      return label.sig
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
        setGroups(parsed.filter((g) => g.child_count > 0))
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

    if (rows[sig]) return // already loaded
    setLoading((s) => new Set([...s, sig]))
    try {
      const page = await window.api.listTypeRecords(activeDbId, sig, 0, PAGE_SIZE)
      setRows((r) => ({ ...r, [sig]: page }))
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
    if (HIERARCHICAL.has(sig)) {
      const current = groupChildren[sig] ?? []
      const next = await window.api.listTypeChildren(activeDbId, sig, current.length, PAGE_SIZE)
      setGroupChildren((c) => ({ ...c, [sig]: [...current, ...next] }))
      return
    }
    const current = rows[sig] ?? []
    const next = await window.api.listTypeRecords(activeDbId, sig, current.length, PAGE_SIZE)
    setRows((r) => ({ ...r, [sig]: [...current, ...next] }))
  }

  // Flat "focusable rows" model for keyboard navigation: top-level groups, plus
  // (for non-hierarchical expanded groups only) their loaded record rows.
  const focusRows: FocusRow[] = []
  for (const g of groups) {
    focusRows.push({ kind: 'group', sig: g.sig })
    if (expanded.has(g.sig) && !HIERARCHICAL.has(g.sig)) {
      for (const row of rows[g.sig] ?? []) {
        focusRows.push({ kind: 'record', row })
      }
    }
  }

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
              {expanded.has(g.sig) ? '▼' : '▶'} {g.sig} ({g.child_count})
            </div>
            {expanded.has(g.sig) && (
              <div>
                {loading.has(g.sig) && <div style={{ padding: 4 }}>Loading…</div>}
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
                  <>
                    <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 11 }}>
                      <thead>
                        <tr style={{ background: '#16213e' }}>
                          <th style={{ padding: '2px 6px', textAlign: 'left' }}>FormID</th>
                          <th style={{ padding: '2px 6px', textAlign: 'left' }}>EditorID</th>
                          <th style={{ padding: '2px 6px', textAlign: 'left' }}>Name</th>
                        </tr>
                      </thead>
                      <tbody>
                        {(rows[g.sig] ?? []).map((row) => {
                          const rowFocusIdx = focusRows.findIndex(
                            (fr) => fr.kind === 'record' && fr.row === row
                          )
                          const rowFocused = rowFocusIdx === focusedIndex
                          return (
                            <tr
                              key={row.form_id}
                              onClick={() => activeDbId && onNavigate(activeDbId, row.form_id)}
                              style={{
                                cursor: 'pointer',
                                borderBottom: '1px solid #222',
                                background: rowFocused ? '#33395a' : undefined,
                              }}
                            >
                              <td style={{ padding: '2px 6px', fontFamily: 'monospace' }}>
                                {row.form_id}
                              </td>
                              <td style={{ padding: '2px 6px' }}>{row.editor_id ?? ''}</td>
                              <td style={{ padding: '2px 6px' }}>{row.name ?? ''}</td>
                            </tr>
                          )
                        })}
                      </tbody>
                    </table>
                    {(rows[g.sig]?.length ?? 0) < g.child_count && (
                      <button onClick={() => void loadMore(g.sig)} style={{ margin: 4, fontSize: 11 }}>
                        Load more…
                      </button>
                    )}
                  </>
                )}
              </div>
            )}
          </div>
        )
      })}
    </div>
  )
}
