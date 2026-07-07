import React, { useCallback, useEffect, useState } from 'react'
import { useStore } from '../store'
import type { RawRecordView } from '../../../shared/api-types'

interface Props {
  onNavigate: (dbId: string, formid: string) => void
}

/** Amber/warning accent for undecoded content — distinct from the `#e88` error red used elsewhere. */
const COVERAGE_COLOR = '#e8a838'

function isFormIdStub(v: unknown): v is { formid: string; editor_id?: string; record_type: string } {
  return typeof v === 'object' && v !== null && 'formid' in v && 'record_type' in v
}

/** Recursively checks a decoded `fields` tree for any schema decode-coverage gap
 * marker (`_unknown_record`, `_raw`, `_unmapped`, `_unresolved`) — see
 * esm/CLAUDE.md "Decode output key conventions". Used to auto-default the
 * raw/decoded toggle and to drive inline coverage badges. */
function hasCoverageMarkers(value: unknown): boolean {
  if (Array.isArray(value)) {
    return value.some((item) => hasCoverageMarkers(item))
  }
  if (typeof value === 'object' && value !== null) {
    const obj = value as Record<string, unknown>
    if (coverageBadges(obj).length > 0) return true
    return Object.values(obj).some((v) => hasCoverageMarkers(v))
  }
  return false
}

/** Which coverage-gap badges (if any) apply directly to this object node
 * (not its descendants) — e.g. `{ "_raw": true, ... }`. */
function coverageBadges(obj: Record<string, unknown>): string[] {
  const badges: string[] = []
  if (obj._unknown_record === true) badges.push('unknown record')
  if (obj._raw === true) badges.push('raw')
  if (isNonEmptyObject(obj._unmapped)) badges.push('unmapped')
  if (obj._unresolved === true) badges.push('unresolved')
  return badges
}

function isNonEmptyObject(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v) && Object.keys(v).length > 0
}

/** True only when the schema has no mapping for this record type at all
 * (top-level `_unknown_record`) — the byte dump is then the only useful view.
 * Unlike `hasCoverageMarkers`, this does NOT recurse: a record that's mostly
 * decoded but has a few nested `_raw`/`_unmapped` fields still counts as known. */
function isUnknownRecordType(fields: unknown): boolean {
  return (
    typeof fields === 'object' &&
    fields !== null &&
    (fields as Record<string, unknown>)._unknown_record === true
  )
}

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

function FieldValue({ value, onNavigate, dbId }: { value: unknown; onNavigate: (dbId: string, fid: string) => void; dbId: string }) {
  if (isFormIdStub(value)) {
    return (
      <span
        tabIndex={0}
        style={{ color: '#7ec8e3', cursor: 'pointer', textDecoration: 'underline' }}
        onClick={(e) => { if (e.ctrlKey || e.metaKey) onNavigate(dbId, value.formid) }}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault()
            onNavigate(dbId, value.formid)
          }
        }}
        title={`Ctrl+Click (or focus + Enter/Space) to navigate to ${value.formid}`}
      >
        {value.editor_id ?? value.formid} [{value.record_type}]
      </span>
    )
  }
  if (Array.isArray(value)) {
    return (
      <div style={{ paddingLeft: 16 }}>
        {value.map((item, i) => (
          <div key={i}>[{i}]: <FieldValue value={item} onNavigate={onNavigate} dbId={dbId} /></div>
        ))}
      </div>
    )
  }
  if (typeof value === 'object' && value !== null) {
    const obj = value as Record<string, unknown>
    const badges = coverageBadges(obj)
    return (
      <div style={{ paddingLeft: 16 }}>
        {badges.length > 0 && (
          <div style={{ color: COVERAGE_COLOR, fontWeight: 'bold' }}>
            {badges.map((b) => `[${b}]`).join(' ')}
          </div>
        )}
        {Object.entries(obj).map(([k, v]) => (
          <div key={k}>
            <span style={{ color: '#aaa' }}>{k}</span>:{' '}
            <FieldValue value={v} onNavigate={onNavigate} dbId={dbId} />
          </div>
        ))}
      </div>
    )
  }
  return <span style={{ color: '#c3e88d' }}>{String(value)}</span>
}

/** Simple first-pass hex-dump renderer for a raw-parsed record: one block per
 * subrecord, monospace, wrapped 16 bytes per line. No ASCII sidebar or byte
 * offset gutter yet — matches this app's other "simple first pass" views. */
function RawRecordSection({
  view,
  loading,
  error
}: {
  view: RawRecordView | null
  loading: boolean
  error: string | null
}) {
  if (loading) return <div style={{ color: '#aaa' }}>Loading raw dump…</div>
  if (error) return <div style={{ color: '#e88' }}>{error}</div>
  if (!view) return <div style={{ color: '#666' }}>No raw data loaded.</div>
  return (
    <div>
      <div style={{ marginBottom: 8, color: '#aaa', fontSize: 11 }}>
        {view.subrecords.length} subrecords &middot; {view.header.data_size} data bytes @ offset{' '}
        {view.header.offset}
      </div>
      {view.subrecords.map((sr, i) => (
        <div key={i} style={{ marginBottom: 10 }}>
          <div>
            <span style={{ color: '#82aaff', fontWeight: 'bold' }}>{sr.signature}</span>{' '}
            <span style={{ color: '#aaa' }}>({sr.size} bytes)</span>
          </div>
          <pre
            style={{
              margin: '4px 0 0',
              padding: 6,
              background: '#16213e',
              border: '1px solid #333',
              borderRadius: 3,
              fontFamily: 'monospace',
              fontSize: 11,
              whiteSpace: 'pre-wrap',
              wordBreak: 'break-all'
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
  const { activeRecord, activeDbId } = useStore()
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
    return <div style={{ padding: 16, color: '#666' }}>Select a record to view details.</div>
  }

  const { header, editor_id, fields } = activeRecord

  return (
    <div style={{ padding: 8, overflow: 'auto', flex: 1, fontSize: 12 }}>
      <div
        style={{
          marginBottom: 8,
          borderBottom: '1px solid #444',
          paddingBottom: 4,
          display: 'flex',
          alignItems: 'center',
          gap: 8
        }}
      >
        <strong>{header.signature}</strong>{' '}
        <span style={{ fontFamily: 'monospace' }}>{String(header.form_id)}</span>
        {editor_id && <span style={{ color: '#aaa' }}> [{editor_id}]</span>}
        <div style={{ marginLeft: 'auto', display: 'flex', gap: 4 }}>
          {(['decoded', 'raw'] as const).map((m) => (
            <button
              key={m}
              onClick={() => switchMode(m)}
              style={{
                fontSize: 11,
                padding: '2px 8px',
                background: mode === m ? '#33395a' : '#16213e',
                color: '#e0e0e0',
                border: '1px solid #444',
                borderRadius: 3,
                cursor: 'pointer'
              }}
            >
              {m === 'decoded' ? 'Decoded' : 'Raw'}
            </button>
          ))}
        </div>
      </div>
      {mode === 'decoded' && hasCoverageMarkers(fields) && (
        <div style={{ marginBottom: 8, color: COVERAGE_COLOR, fontSize: 11 }}>
          Some fields are undecoded (see [raw]/[unmapped] badges below) — switch to Raw for the
          full byte dump.
        </div>
      )}
      {mode === 'decoded' ? (
        Object.entries(fields as Record<string, unknown>).map(([key, value]) => (
          <div key={key} style={{ marginBottom: 4 }}>
            <span style={{ color: '#82aaff', fontWeight: 'bold' }}>{key}</span>:{' '}
            <FieldValue value={value} onNavigate={onNavigate} dbId={activeDbId} />
          </div>
        ))
      ) : (
        <RawRecordSection view={rawView} loading={rawLoading} error={rawError} />
      )}
    </div>
  )
}
