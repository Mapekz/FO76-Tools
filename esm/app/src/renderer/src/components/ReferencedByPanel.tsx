import React from 'react'
import { useStore } from '../store'

interface Props {
  onNavigate: (dbId: string, formid: string) => void
}

export function ReferencedByPanel({ onNavigate }: Props) {
  const { referencedBy, activeDbId } = useStore()

  if (!activeDbId) return null

  return (
    <div style={{ borderTop: '1px solid #444', padding: 8, fontSize: 12 }}>
      <strong>Referenced By ({referencedBy.length})</strong>
      <div style={{ maxHeight: 150, overflowY: 'auto', marginTop: 4 }}>
        {referencedBy.map((row) => (
          <div
            key={row.form_id}
            style={{ cursor: 'pointer', padding: '1px 0' }}
            onClick={() => onNavigate(activeDbId, row.form_id)}
          >
            <span style={{ fontFamily: 'monospace', color: '#7ec8e3' }}>{row.form_id}</span>{' '}
            {row.editor_id && <span style={{ color: '#aaa' }}>[{row.editor_id}]</span>}{' '}
            {row.name && <span>{row.name}</span>}
          </div>
        ))}
      </div>
    </div>
  )
}
