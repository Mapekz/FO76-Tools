/** Source of truth for the FO76 ESM Viewer palette — every hex/rgba literal
 * in `App.tsx` and `components/` should be a reference into this file, not an
 * inline literal. `../../DESIGN.md`'s YAML frontmatter documents the same 17
 * tokens for human readers; a drift test compares that doc against this file,
 * so keep names/values in lockstep when either changes. */

import type { CSSProperties } from 'react'

export const colors = {
  traceBlue: '#7ec8e3',
  signatureBlue: '#82aaff',
  completeGreen: '#c3e88d',
  gapAmber: '#e8a838',
  faultRed: '#e88',
  workbenchBlack: '#1a1a2e',
  panelSteel: '#16213e',
  rowSlate: '#1e1e2e',
  hoverGraphite: '#2a2a3a',
  focusIndigo: '#33395a',
  benchLight: '#e0e0e0',
  dimReadout: '#aaa',
  faintReadout: '#888',
  ghostText: '#666',
  seam: '#444',
  rule: '#333',
  hairline: '#222',
} as const

/** 10%-opacity Fault Red wash — the Missing-Tint Rule (DESIGN.md): a value
 * absent from one file in a comparison gets this translucent wash, never a
 * solid color swap, so "missing" reads differently from "present but
 * different" at a glance. */
export const missingTint = 'rgba(238,136,136,0.10)'

/** 10%-opacity Gap Amber wash — same translucent treatment as `missingTint`,
 * used to tint a whole row when its values conflict across columns. */
export const gapTint = 'rgba(232,168,56,0.10)'

/** Shared shell for every left-panel view (Search/Filter/Coverage/Diff): a
 * dense, flex-column body that fills the panel and lets its own scrollable
 * content region shrink below its natural size. */
export const panelStyle: CSSProperties = {
  padding: 8,
  fontSize: 12,
  display: 'flex',
  flexDirection: 'column',
  flex: 1,
  minHeight: 0,
}

/** Shared look for every text `<input>` across those same panels — Panel
 * Steel background, Bench Light text, Seam border, monospace. Sites with a
 * conditional (e.g. disabled) background spread this and override just that
 * property. */
export const inputStyle: CSSProperties = {
  background: colors.panelSteel,
  color: colors.benchLight,
  border: `1px solid ${colors.seam}`,
  borderRadius: 3,
  padding: '4px 6px',
  fontFamily: 'monospace',
}
