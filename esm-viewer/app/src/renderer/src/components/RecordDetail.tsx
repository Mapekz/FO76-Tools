import React from 'react'
import { useStore } from '../store'

interface Props {
  onNavigate: (dbId: string, formid: string) => void
}

function isFormIdStub(v: unknown): v is { formid: string; editor_id?: string; record_type: string } {
  return typeof v === 'object' && v !== null && 'formid' in v && 'record_type' in v
}

function FieldValue({ value, onNavigate, dbId }: { value: unknown; onNavigate: (dbId: string, fid: string) => void; dbId: string }) {
  if (isFormIdStub(value)) {
    return (
      <span
        style={{ color: '#7ec8e3', cursor: 'pointer', textDecoration: 'underline' }}
        onClick={(e) => { if (e.ctrlKey || e.metaKey) onNavigate(dbId, value.formid) }}
        title={`Ctrl+Click to navigate to ${value.formid}`}
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
    return (
      <div style={{ paddingLeft: 16 }}>
        {Object.entries(value as Record<string, unknown>).map(([k, v]) => (
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

export function RecordDetail({ onNavigate }: Props) {
  const { activeRecord, activeDbId } = useStore()

  if (!activeRecord || !activeDbId) {
    return <div style={{ padding: 16, color: '#666' }}>Select a record to view details.</div>
  }

  const { header, editor_id, fields } = activeRecord

  return (
    <div style={{ padding: 8, overflow: 'auto', flex: 1, fontSize: 12 }}>
      <div style={{ marginBottom: 8, borderBottom: '1px solid #444', paddingBottom: 4 }}>
        <strong>{header.signature}</strong>{' '}
        <span style={{ fontFamily: 'monospace' }}>{String(header.form_id)}</span>
        {editor_id && <span style={{ color: '#aaa' }}> [{editor_id}]</span>}
      </div>
      {Object.entries(fields as Record<string, unknown>).map(([key, value]) => (
        <div key={key} style={{ marginBottom: 4 }}>
          <span style={{ color: '#82aaff', fontWeight: 'bold' }}>{key}</span>:{' '}
          <FieldValue value={value} onNavigate={onNavigate} dbId={activeDbId} />
        </div>
      ))}
    </div>
  )
}
