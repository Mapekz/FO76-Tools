import React from 'react'
import { useStore } from '../store'
import { fetchReferencedBy } from '../lib/referencedBy'
import type { RefPathNode, RefRow } from '../../../shared/api-types'

interface Props {
  onNavigate: (dbId: string, formid: string) => void
}

function pathLabel(node: RefPathNode): string {
  return node.editor_id ?? node.form_id
}

/** Render the hop chain for a depth>1 row: target ← hop1 ← hop2 ← … ← this row. */
function HopChain({ row }: { row: RefRow }) {
  const path = row.path ?? []
  if (!row.depth || row.depth <= 1 || path.length === 0) return null
  const chain = [...path.map(pathLabel), row.editor_id ?? row.form_id]
  return (
    <div style={{ fontSize: 10, color: '#888', paddingLeft: 2 }}>
      {chain.join(' ← ')}
      {` (depth ${row.depth})`}
    </div>
  )
}

export function ReferencedByPanel({ onNavigate }: Props) {
  const {
    referencedBy,
    referencedByDepth,
    referencedByTotal,
    referencedByCapped,
    activeDbId,
    activeRecord,
    setReferencedBy,
    setReferencedByDepth,
  } = useStore()

  if (!activeDbId) return null

  async function handleDepthChange(e: React.ChangeEvent<HTMLSelectElement>) {
    const newDepth = Number(e.target.value)
    setReferencedByDepth(newDepth)
    if (!activeDbId || !activeRecord) return
    try {
      const result = await fetchReferencedBy(
        activeDbId,
        activeRecord.header.form_id,
        newDepth,
        window.api
      )
      setReferencedBy(result)
    } catch (err) {
      console.error('referencedById depth change error:', err)
    }
  }

  return (
    <div style={{ borderTop: '1px solid #444', padding: 8, fontSize: 12 }}>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <strong>
          Referenced By ({referencedBy.length} of {referencedByTotal}
          {referencedByCapped ? ', capped' : ''})
        </strong>
        <label style={{ display: 'flex', alignItems: 'center', gap: 4, fontWeight: 'normal' }}>
          Depth:
          <select value={referencedByDepth} onChange={(e) => void handleDepthChange(e)}>
            {[1, 2, 3, 4, 5, 6].map((d) => (
              <option key={d} value={d}>
                {d}
              </option>
            ))}
          </select>
        </label>
      </div>
      <div style={{ maxHeight: 150, overflowY: 'auto', marginTop: 4 }}>
        {referencedBy.map((row, i) => (
          <div
            key={`${row.form_id}-${i}`}
            style={{ cursor: 'pointer', padding: '1px 0' }}
            onClick={() => onNavigate(activeDbId, row.form_id)}
          >
            <HopChain row={row} />
            <span style={{ fontFamily: 'monospace', color: '#7ec8e3' }}>{row.form_id}</span>{' '}
            {row.editor_id && <span style={{ color: '#aaa' }}>[{row.editor_id}]</span>}{' '}
            {row.name && <span>{row.name}</span>}
          </div>
        ))}
      </div>
    </div>
  )
}
