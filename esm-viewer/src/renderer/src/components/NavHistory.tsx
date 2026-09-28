import React from 'react'
import { navReach, useStore } from '../store'
import { colors } from '../theme'

export function NavHistory() {
  const { nav, navPending, goBack, goForward } = useStore()
  const reach = navReach({ nav, navPending })

  return (
    <div
      style={{
        display: 'flex',
        gap: 4,
        padding: '4px 8px',
        borderBottom: `1px solid ${colors.seam}`,
      }}
    >
      <button onClick={() => void goBack()} disabled={!reach.back}>
        ← Back
      </button>
      <button onClick={() => void goForward()} disabled={!reach.forward}>
        Forward →
      </button>
    </div>
  )
}
