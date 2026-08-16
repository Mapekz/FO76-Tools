import React, { useCallback, useEffect, useState } from 'react'
import { useStore } from '../store'
import { RecordTable } from './RecordTable'
import { hasCoverageMarkers, isUnknownRecordType } from '../lib/alignedTree'
import type { RawRecordView } from '../../../shared/api-types'
import { colors } from '../theme'

interface Props {
  onNavigate: (dbId: string, formid: string) => void
}

/** Amber/warning accent for undecoded content — distinct from the Fault Red error color used
 * elsewhere. */
const COVERAGE_COLOR = colors.gapAmber

/** Insert a space every 2 hex chars (byte boundary) and a line break every
 * 16 bytes — a plain hex-dump first pass with no ASCII sidebar or offset gutter. */
function formatHexDump(hex: string): string {
  const bytes = hex.match(/.{1,2}/g) ?? []
  const lines: string[] = []
  for (let i = 0; i < bytes.length; i += 16) {
    lines.push(bytes.slice(i, i + 16).join(' '))
  }
  return lines.join('\n')
}

/** Simple first-pass hex-dump renderer for a raw-parsed record: one block per
 * subrecord, monospace, wrapped 16 bytes per line. No ASCII sidebar or byte
 * offset gutter yet — matches this app's other "simple first pass" views. */
function RawRecordSection({
  view,
  loading,
  error,
}: {
  view: RawRecordView | null
  loading: boolean
  error: string | null
}) {
  if (loading) return <div style={{ color: colors.dimReadout }}>Loading raw dump…</div>
  if (error) return <div style={{ color: colors.faultRed }}>{error}</div>
  if (!view) return <div style={{ color: colors.ghostText }}>No raw data loaded.</div>
  return (
    <div>
      <div style={{ marginBottom: 8, color: colors.dimReadout, fontSize: 11 }}>
        {view.subrecords.length} subrecords &middot; {view.header.data_size} data bytes @ offset{' '}
        {view.header.offset}
      </div>
      {view.subrecords.map((sr, i) => (
        // Subrecords have no unique id — a signature (e.g. repeated CTDA) can appear more than once.
        // oxlint-disable-next-line react/no-array-index-key
        <div key={i} style={{ marginBottom: 10 }}>
          <div>
            <span style={{ color: colors.signatureBlue, fontWeight: 'bold' }}>{sr.signature}</span>{' '}
            <span style={{ color: colors.dimReadout }}>({sr.size} bytes)</span>
          </div>
          <pre
            style={{
              margin: '4px 0 0',
              padding: 6,
              background: colors.panelSteel,
              border: `1px solid ${colors.rule}`,
              borderRadius: 3,
              fontFamily: 'monospace',
              fontSize: 11,
              whiteSpace: 'pre-wrap',
              wordBreak: 'break-all',
            }}
          >
            {formatHexDump(sr.hex)}
          </pre>
        </div>
      ))}
    </div>
  )
}

type ViewMode = 'decoded' | 'raw'

export function RecordDetail({ onNavigate }: Props) {
  const { activeRecord, activeDbId, recordColumns } = useStore()
  const [mode, setMode] = useState<ViewMode>('decoded')
  const [rawView, setRawView] = useState<RawRecordView | null>(null)
  const [rawLoading, setRawLoading] = useState(false)
  const [rawError, setRawError] = useState<string | null>(null)

  const loadRaw = useCallback(async (dbId: string, formId: string) => {
    setRawLoading(true)
    setRawError(null)
    try {
      const view = await window.api.recordRaw(dbId, formId)
      setRawView(view)
    } catch (e) {
      setRawError(e instanceof Error ? e.message : String(e))
    } finally {
      setRawLoading(false)
    }
  }, [])

  // Re-default the toggle per-record: raw only when the record's type has no
  // schema mapping at all (nothing to decode); decoded otherwise, even if a
  // few nested fields carry coverage-gap markers (see the inline badges and
  // the header note below). Recomputed whenever the active record changes so
  // it doesn't get stuck on a prior record's choice.
  useEffect(() => {
    setRawView(null)
    setRawError(null)
    if (!activeRecord || !activeDbId) {
      setMode('decoded')
      return
    }
    const defaultMode: ViewMode = isUnknownRecordType(activeRecord.fields) ? 'raw' : 'decoded'
    setMode(defaultMode)
    if (defaultMode === 'raw') {
      void loadRaw(activeDbId, activeRecord.header.form_id)
    }
    // activeRecord's identity changes on every load (new object from IPC), so this
    // effect fires exactly once per navigated-to record.
  }, [activeRecord, activeDbId, loadRaw])

  function switchMode(next: ViewMode) {
    setMode(next)
    if (next === 'raw' && !rawView && !rawLoading && activeDbId && activeRecord) {
      void loadRaw(activeDbId, activeRecord.header.form_id)
    }
  }

  if (!activeRecord || !activeDbId) {
    return (
      <div style={{ padding: 16, color: colors.ghostText }}>Select a record to view details.</div>
    )
  }

  const { header, editor_id, fields } = activeRecord

  return (
    // Decoded mode: flex column with overflow hidden, so RecordTable's inner
    // scroll region gets a constrained height and its sticky <thead> works.
    // Raw mode: plain scrolling container.
    <div
      style={{
        padding: 8,
        flex: 1,
        fontSize: 12,
        minHeight: 0,
        ...(mode === 'decoded'
          ? { display: 'flex', flexDirection: 'column' as const, overflow: 'hidden' }
          : { overflow: 'auto' }),
      }}
    >
      <div
        style={{
          marginBottom: 8,
          borderBottom: `1px solid ${colors.seam}`,
          paddingBottom: 4,
          display: 'flex',
          alignItems: 'center',
          gap: 8,
        }}
      >
        <strong>{header.signature}</strong>{' '}
        <span style={{ fontFamily: 'monospace' }}>{String(header.form_id)}</span>
        {editor_id && <span style={{ color: colors.dimReadout }}> [{editor_id}]</span>}
        <div style={{ marginLeft: 'auto', display: 'flex', gap: 4 }}>
          {(['decoded', 'raw'] as const).map((m) => (
            <button
              key={m}
              onClick={() => switchMode(m)}
              title={m === 'raw' ? 'Raw shows the active file' : undefined}
              style={{
                fontSize: 11,
                padding: '2px 8px',
                background: mode === m ? colors.focusIndigo : colors.panelSteel,
                color: colors.benchLight,
                border: `1px solid ${colors.seam}`,
                borderRadius: 3,
                cursor: 'pointer',
              }}
            >
              {m === 'decoded' ? 'Decoded' : 'Raw'}
            </button>
          ))}
        </div>
      </div>
      {mode === 'decoded' && hasCoverageMarkers(fields) && (
        <div style={{ marginBottom: 8, color: COVERAGE_COLOR, fontSize: 11 }}>
          Some fields are undecoded (see [raw]/[unmapped] badges below) — switch to Raw for the full
          byte dump.
        </div>
      )}
      {mode === 'decoded' ? (
        <RecordTable
          key={header.form_id + activeDbId}
          columns={recordColumns}
          activeDbId={activeDbId}
          onNavigate={onNavigate}
        />
      ) : (
        <RawRecordSection view={rawView} loading={rawLoading} error={rawError} />
      )}
    </div>
  )
}
