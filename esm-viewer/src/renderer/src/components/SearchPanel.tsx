import React, { useState } from 'react'
import { useStore } from '../store'
import type { RecordRow } from '../../../shared/api-types'

interface Props {
  onNavigate: (dbId: string, formid: string) => void
}

const LIMIT = 200

export function SearchPanel({ onNavigate }: Props) {
  const { activeDbId } = useStore()
  const [pattern, setPattern] = useState('')
  const [field, setField] = useState<'edid' | 'name' | 'both'>('both')
  const [typesText, setTypesText] = useState('')
  const [results, setResults] = useState<RecordRow[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  if (!activeDbId) return null

  async function runSearch() {
    if (!activeDbId) return
    const types = typesText
      .split(',')
      .map((t) => t.trim().toUpperCase())
      .filter((t) => t.length > 0)
    setLoading(true)
    setError(null)
    try {
      const rows = await window.api.search(activeDbId, pattern, types, field, LIMIT)
      setResults(rows)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }

  return (
    <div style={{ padding: 8, fontSize: 12, display: 'flex', flexDirection: 'column', flex: 1, minHeight: 0 }}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        <input
          type="text"
          value={pattern}
          onChange={(e) => setPattern(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void runSearch()
          }}
          placeholder="*Rifle*  (wildcard, case-insensitive)"
          style={{
            background: '#16213e',
            color: '#e0e0e0',
            border: '1px solid #444',
            borderRadius: 3,
            padding: '4px 6px',
            fontFamily: 'monospace'
          }}
        />
        <div style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
          <label style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
            Field:
            <select value={field} onChange={(e) => setField(e.target.value as 'edid' | 'name' | 'both')}>
              <option value="edid">EditorID</option>
              <option value="name">Name</option>
              <option value="both">Both</option>
            </select>
          </label>
        </div>
        <input
          type="text"
          value={typesText}
          onChange={(e) => setTypesText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void runSearch()
          }}
          placeholder="Type signatures, comma-separated (e.g. WEAP,ARMO) — blank = all types"
          style={{
            background: '#16213e',
            color: '#e0e0e0',
            border: '1px solid #444',
            borderRadius: 3,
            padding: '4px 6px',
            fontFamily: 'monospace'
          }}
        />
        <button onClick={() => void runSearch()} disabled={loading} style={{ alignSelf: 'flex-start' }}>
          {loading ? 'Searching…' : 'Search'}
        </button>
      </div>

      {error && <div style={{ color: '#e88', marginTop: 6 }}>{error}</div>}

      <div style={{ marginTop: 8, fontWeight: 'bold' }}>
        {results.length} result{results.length === 1 ? '' : 's'}
        {results.length === LIMIT ? ' (capped)' : ''}
      </div>
      <div style={{ overflowY: 'auto', flex: 1, marginTop: 4 }}>
        {results.map((row, i) => (
          <div
            key={`${row.form_id}-${i}`}
            style={{ cursor: 'pointer', padding: '2px 0' }}
            onClick={() => onNavigate(activeDbId, row.form_id)}
          >
            <span style={{ fontFamily: 'monospace', color: '#7ec8e3' }}>{row.form_id}</span>{' '}
            {row.record_type && <span style={{ color: '#aaa' }}>({row.record_type})</span>}{' '}
            {row.editor_id && <span style={{ color: '#aaa' }}>[{row.editor_id}]</span>}{' '}
            {row.name && <span>{row.name}</span>}
          </div>
        ))}
      </div>
    </div>
  )
}
