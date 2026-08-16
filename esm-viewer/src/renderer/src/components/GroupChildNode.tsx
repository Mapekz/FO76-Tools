import React, { useState } from 'react'
import type { GroupChild, GroupLabel } from '../../../shared/api-types'
import { formatRecordType } from '../recordTypeNames'
import { loadGroupChildrenPage } from '../lib/recordLoad'
import { colors } from '../theme'

export const PAGE_SIZE = 100

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
export function GroupChildNode({
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
        <span style={{ fontFamily: 'monospace', color: colors.traceBlue }}>{child.form_id}</span>{' '}
        <span style={{ color: colors.dimReadout }}>[{child.record_type}]</span>{' '}
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
      const result = await loadGroupChildrenPage(window.api, dbId, child.offset, [], PAGE_SIZE)
      setChildren(result)
    } finally {
      setLoading(false)
    }
  }

  async function loadMore() {
    const current = children ?? []
    const next = await loadGroupChildrenPage(window.api, dbId, child.offset, current, PAGE_SIZE)
    setChildren(next)
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
