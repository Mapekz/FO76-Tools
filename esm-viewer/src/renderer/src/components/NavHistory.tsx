import React from 'react'
import { useStore } from '../store'
import { colors } from '../theme'

export function NavHistory() {
  const { nav, goBack, goForward } = useStore()

  return (
    <div
      style={{
        display: 'flex',
        gap: 4,
        padding: '4px 8px',
        borderBottom: `1px solid ${colors.seam}`,
      }}
    >
      <button onClick={() => void goBack()} disabled={nav.index <= 0}>
        ← Back
      </button>
      <button onClick={() => void goForward()} disabled={nav.index >= nav.entries.length - 1}>
        Forward →
      </button>
    </div>
  )
}
