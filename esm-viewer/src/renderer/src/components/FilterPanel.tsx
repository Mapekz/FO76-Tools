import React, { useEffect, useState } from 'react'
import { useStore } from '../store'
import type { FilterOp, FilterResult, RecordRow } from '../../../shared/api-types'
import { formatRecordType } from '../recordTypeNames'

interface Props {
  onNavigate: (dbId: string, formid: string) => void
}

const LIMIT = 200

const OPERATORS: { value: FilterOp; label: string }[] = [
  { value: 'exists', label: 'Exists' },
  { value: 'eq', label: 'Equals' },
  { value: 'contains', label: 'Contains' },
  { value: 'gt', label: '>' },
  { value: 'lt', label: '<' },
  { value: 'gte', label: '>=' },
  { value: 'lte', label: '<=' }
]

export function FilterPanel({ onNavigate }: Props) {
  const { activeDbId } = useStore()
  const [sigs, setSigs] = useState<string[]>([])
  const [sig, setSig] = useState('')
  const [fieldPaths, setFieldPaths] = useState<string[]>([])
  const [path, setPath] = useState('')
  const [op, setOp] = useState<FilterOp>('exists')
  const [value, setValue] = useState('')
  const [result, setResult] = useState<FilterResult | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!activeDbId) {
      setSigs([])
      return
    }
    window.api
      .listGroups(activeDbId)
      .then((groups) => {
        const list = groups
          .filter((g) => g.label.kind === 'record_type' && g.child_count > 0)
          .map((g) => (g.label.kind === 'record_type' ? g.label.sig : ''))
          .filter((s) => s.length > 0)
          .sort()
        setSigs(list)
        setSig((prev) => prev || list[0] || '')
      })
      .catch(console.error)
  }, [activeDbId])

  useEffect(() => {
    if (!activeDbId || !sig) {
      setFieldPaths([])
      return
    }
    window.api
      .listTypeFieldPaths(activeDbId, sig)
      .then(setFieldPaths)
      .catch(console.error)
  }, [activeDbId, sig])

  if (!activeDbId) return null

  async function runFilter() {
    if (!activeDbId || !sig) return
    setLoading(true)
    setError(null)
    try {
      const res = await window.api.filterTypeRecords(
        activeDbId,
        sig,
        path.trim() || undefined,
        op,
        op === 'exists' ? undefined : value,
        LIMIT
      )
      setResult(res)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }

  const rows: RecordRow[] = result?.rows ?? []

  return (
    <div style={{ padding: 8, fontSize: 12, display: 'flex', flexDirection: 'column', flex: 1, minHeight: 0 }}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        <label style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
          Type:
          <select value={sig} onChange={(e) => setSig(e.target.value)} style={{ flex: 1 }}>
            {sigs.map((s) => (
              <option key={s} value={s}>
                {formatRecordType(s)}
              </option>
            ))}
          </select>
        </label>

        <input
          type="text"
          list="filter-field-paths"
          value={path}
          onChange={(e) => setPath(e.target.value)}
          placeholder="(leave blank to scan all fields)"
          style={{
            background: '#16213e',
            color: '#e0e0e0',
            border: '1px solid #444',
            borderRadius: 3,
            padding: '4px 6px',
            fontFamily: 'monospace'
          }}
        />
        <datalist id="filter-field-paths">
          {fieldPaths.map((p) => (
            <option key={p} value={p} />
          ))}
        </datalist>

        <div style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
          <label style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
            Op:
            <select value={op} onChange={(e) => setOp(e.target.value as FilterOp)}>
              {OPERATORS.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
          </label>
          <input
            type="text"
            value={value}
            disabled={op === 'exists'}
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void runFilter()
            }}
            placeholder="value"
            style={{
              flex: 1,
              background: op === 'exists' ? '#222' : '#16213e',
              color: '#e0e0e0',
              border: '1px solid #444',
              borderRadius: 3,
              padding: '4px 6px',
              fontFamily: 'monospace'
            }}
          />
        </div>

        <button onClick={() => void runFilter()} disabled={loading || !sig} style={{ alignSelf: 'flex-start' }}>
          {loading ? 'Filtering…' : 'Filter'}
        </button>
      </div>

      {error && <div style={{ color: '#e88', marginTop: 6 }}>{error}</div>}

      {result && (
        <div style={{ marginTop: 8 }}>
          <div style={{ fontWeight: 'bold' }}>
            {rows.length} of {result.matched} matches
          </div>
          {result.scan_capped && (
            <div style={{ color: '#aaa', fontSize: 11 }}>
              (scanned first {result.scanned} of {result.total} {sig} records)
            </div>
          )}
        </div>
      )}
      <div style={{ overflowY: 'auto', flex: 1, marginTop: 4 }}>
        {rows.map((row, i) => (
          <div
            key={`${row.form_id}-${i}`}
            style={{ cursor: 'pointer', padding: '2px 0' }}
            onClick={() => onNavigate(activeDbId, row.form_id)}
          >
            <span style={{ fontFamily: 'monospace', color: '#7ec8e3' }}>{row.form_id}</span>{' '}
            {row.editor_id && <span style={{ color: '#aaa' }}>[{row.editor_id}]</span>}{' '}
            {row.name && <span>{row.name}</span>}
          </div>
        ))}
      </div>
    </div>
  )
}
