import React, { useEffect, useState } from 'react'
import { useStore } from '../store'
import type { FilterOp, FilterResult, RecordRow } from '../../../shared/api-types'
import { formatRecordType } from '../recordTypeNames'
import { listRecordTypeSigs } from '../lib/sigLists'
import { useAsyncAction } from '../lib/useAsyncAction'
import { RecordRef } from './RecordRef'
import { colors, panelStyle, inputStyle } from '../theme'

const LIMIT = 200

const OPERATORS: { value: FilterOp; label: string }[] = [
  { value: 'exists', label: 'Exists' },
  { value: 'eq', label: 'Equals' },
  { value: 'contains', label: 'Contains' },
  { value: 'gt', label: '>' },
  { value: 'lt', label: '<' },
  { value: 'gte', label: '>=' },
  { value: 'lte', label: '<=' },
]

export function FilterPanel() {
  const { activeDbId, openDbs } = useStore()
  const [sigs, setSigs] = useState<string[]>([])
  const [sig, setSig] = useState('')
  const [fieldPaths, setFieldPaths] = useState<string[]>([])
  const [path, setPath] = useState('')
  const [op, setOp] = useState<FilterOp>('exists')
  const [value, setValue] = useState('')
  // The result remembers the file it was filtered in, so a row still opens
  // there after the active file changes; closing that file drops it.
  const [filtered, setFiltered] = useState<(FilterResult & { dbId: string }) | null>(null)
  const { loading, error, run } = useAsyncAction()
  const result = filtered && openDbs.some((db) => db.id === filtered.dbId) ? filtered : null

  useEffect(() => {
    if (!activeDbId) {
      setSigs([])
      return
    }
    listRecordTypeSigs(window.api, activeDbId)
      .then((list) => {
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
      .run(activeDbId, { op: 'list_type_field_paths', sig })
      .then(setFieldPaths)
      .catch(console.error)
  }, [activeDbId, sig])

  if (!activeDbId) return null

  async function runFilter() {
    if (!activeDbId || !sig) return
    const dbId = activeDbId
    await run(async () => {
      const res = await window.api.run(dbId, {
        op: 'filter_type_records',
        sig,
        path: path.trim() || null,
        filter_op: op,
        value: op === 'exists' ? null : value,
        limit: LIMIT,
      })
      setFiltered({ ...res, dbId })
    })
  }

  const rows: RecordRow[] = result?.rows ?? []

  return (
    <div style={panelStyle}>
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
          style={inputStyle}
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
              ...inputStyle,
              flex: 1,
              background: op === 'exists' ? colors.hairline : colors.panelSteel,
            }}
          />
        </div>

        <button
          onClick={() => void runFilter()}
          disabled={loading || !sig}
          style={{ alignSelf: 'flex-start' }}
        >
          {loading ? 'Filtering…' : 'Filter'}
        </button>
      </div>

      {error && <div style={{ color: colors.faultRed, marginTop: 6 }}>{error}</div>}

      {result && (
        <div style={{ marginTop: 8 }}>
          <div style={{ fontWeight: 'bold' }}>
            {rows.length} of {result.matched} matches
          </div>
          {result.scan_capped && (
            <div style={{ color: colors.dimReadout, fontSize: 11 }}>
              (scanned first {result.scanned} of {result.total} {sig} records)
            </div>
          )}
        </div>
      )}
      <div style={{ overflowY: 'auto', flex: 1, marginTop: 4 }}>
        {rows.map((row, i) => (
          // Composite key: index guards against duplicate form_ids across pages.
          <RecordRef
            // oxlint-disable-next-line react/no-array-index-key
            key={`${row.form_id}-${i}`}
            dbId={result?.dbId ?? activeDbId}
            formId={row.form_id}
            editorId={row.editor_id}
            name={row.name}
          />
        ))}
      </div>
    </div>
  )
}
