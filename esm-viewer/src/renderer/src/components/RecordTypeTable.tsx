import React, { useEffect, useRef } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import type { RecordRow } from '../../../shared/api-types'
import { type SortColumn, type SortState } from '../lib/recordSort'
import { colors } from '../theme'

/** Flat per-type table layout: shared by the header and every virtualized row
 * so columns line up. */
const COLUMN_TEMPLATE = '95px 1fr 1fr'
const ROW_HEIGHT = 22
/** Viewport cap (in rows) before a group's table gets its own inner scrollbar —
 * below this, the viewport is sized exactly to content (no virtualization overhead visible). */
const MAX_VISIBLE_ROWS = 15

const HEADER_CELL_STYLE: React.CSSProperties = {
  padding: '2px 6px',
  textAlign: 'left',
  cursor: 'pointer',
}
const BODY_CELL_STYLE: React.CSSProperties = {
  padding: '2px 6px',
  overflow: 'hidden',
  textOverflow: 'ellipsis',
  whiteSpace: 'nowrap',
}

function sortIndicator(column: SortColumn, sortState: SortState | undefined): string {
  if (sortState?.column !== column) return ''
  return sortState.direction === 'asc' ? ' ▲' : ' ▼'
}

/** Virtualized, sortable, click-to-navigate table for one record type's flat
 * row list. Rows are rendered as CSS-Grid `<div>`s rather than a native
 * `<table>`/`<tr>` because `@tanstack/react-virtual` positions items via
 * `transform: translateY()` on absolutely-positioned elements, which native
 * table row layout does not support. */
export function RecordTypeTable({
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
          background: colors.panelSteel,
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
      <div
        ref={parentRef}
        style={{ height: viewportHeight, overflow: 'auto', position: 'relative' }}
      >
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
                  borderBottom: `1px solid ${colors.hairline}`,
                  background: rowFocused ? colors.focusIndigo : undefined,
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
