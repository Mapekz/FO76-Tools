import React from 'react'
import { useStore } from '../store'

interface Props {
  onBack: () => void
  onForward: () => void
}

export function NavHistory({ onBack, onForward }: Props) {
  const { nav } = useStore()

  return (
    <div style={{ display: 'flex', gap: 4, padding: '4px 8px', borderBottom: '1px solid #444' }}>
      <button onClick={onBack} disabled={nav.index <= 0}>← Back</button>
      <button onClick={onForward} disabled={nav.index >= nav.entries.length - 1}>Forward →</button>
    </div>
  )
}
