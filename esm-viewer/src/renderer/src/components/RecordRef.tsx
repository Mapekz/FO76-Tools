import React from 'react'
import { useStore } from '../store'
import { colors } from '../theme'

/** A clickable record row — FormID, then type, EditorID and name when
 * known — that navigates to the record in `dbId`. `header` renders above the
 * row's text and `children` after it, both inside the click target. */
export function RecordRef({
  dbId,
  formId,
  recordType,
  editorId,
  name,
  header,
  children,
  style,
}: {
  dbId: string
  formId: string
  recordType?: string | null
  editorId?: string | null
  name?: string | null
  header?: React.ReactNode
  children?: React.ReactNode
  style?: React.CSSProperties
}) {
  const navigate = useStore((s) => s.navigate)
  return (
    <div
      style={{ cursor: 'pointer', padding: '2px 0', ...style }}
      onClick={() => void navigate(dbId, formId)}
    >
      {header}
      <span style={{ fontFamily: 'monospace', color: colors.traceBlue }}>{formId}</span>{' '}
      {recordType && <span style={{ color: colors.dimReadout }}>[{recordType}]</span>}{' '}
      {editorId && <span style={{ color: colors.dimReadout }}>[{editorId}]</span>}{' '}
      {name && <span>{name}</span>}
      {children}
    </div>
  )
}
