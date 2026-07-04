import React from 'react'
import { useStore } from '../store'

interface Props {
  onNavigate: (dbId: string, formid: string) => void
}

export function NavHistory({ onNavigate }: Props) {
  const { nav, navBack, navForward } = useStore()

  function handleBack() {
    const entry = navBack()
    if (entry) onNavigate(entry.dbId, entry.formid)
  }

  function handleForward() {
    const entry = navForward()
    if (entry) onNavigate(entry.dbId, entry.formid)
  }

  return (
    <div style={{ display: 'flex', gap: 4, padding: '4px 8px', borderBottom: '1px solid #444' }}>
      <button onClick={handleBack} disabled={nav.index <= 0}>← Back</button>
      <button onClick={handleForward} disabled={nav.index >= nav.entries.length - 1}>Forward →</button>
    </div>
  )
}
