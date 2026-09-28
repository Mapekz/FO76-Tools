import React, { useState } from 'react'
import { useStore } from '../store'
import type { RecordRow } from '../../../shared/api-types'
import { parseSigList } from '../lib/sigLists'
import { useAsyncAction } from '../lib/useAsyncAction'
import { RecordRef } from './RecordRef'
import { colors, panelStyle, inputStyle } from '../theme'

const LIMIT = 200

export function SearchPanel() {
  const { activeDbId } = useStore()
  const [pattern, setPattern] = useState('')
  const [field, setField] = useState<'edid' | 'name' | 'both'>('both')
  const [typesText, setTypesText] = useState('')
  const [results, setResults] = useState<RecordRow[]>([])
  const { loading, error, run } = useAsyncAction()

  if (!activeDbId) return null

  async function runSearch() {
    if (!activeDbId) return
    const dbId = activeDbId
    const types = parseSigList(typesText)
    await run(async () => {
      setResults(await window.api.run(dbId, { op: 'search', pattern, types, field, limit: LIMIT }))
    })
  }

  return (
    <div style={panelStyle}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        <input
          type="text"
          value={pattern}
          onChange={(e) => setPattern(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void runSearch()
          }}
          placeholder="*Rifle*  (wildcard, case-insensitive)"
          style={inputStyle}
        />
        <div style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
          <label style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
            Field:
            <select
              value={field}
              onChange={(e) => setField(e.target.value as 'edid' | 'name' | 'both')}
            >
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
          style={inputStyle}
        />
        <button
          onClick={() => void runSearch()}
          disabled={loading}
          style={{ alignSelf: 'flex-start' }}
        >
          {loading ? 'Searching…' : 'Search'}
        </button>
      </div>

      {error && <div style={{ color: colors.faultRed, marginTop: 6 }}>{error}</div>}

      <div style={{ marginTop: 8, fontWeight: 'bold' }}>
        {results.length} result{results.length === 1 ? '' : 's'}
        {results.length === LIMIT ? ' (capped)' : ''}
      </div>
      <div style={{ overflowY: 'auto', flex: 1, marginTop: 4 }}>
        {results.map((row, i) => (
          // Composite key: index guards against duplicate form_ids across pages.
          <RecordRef
            // oxlint-disable-next-line react/no-array-index-key
            key={`${row.form_id}-${i}`}
            dbId={activeDbId}
            formId={row.form_id}
            recordType={row.record_type}
            editorId={row.editor_id}
            name={row.name}
          />
        ))}
      </div>
    </div>
  )
}
