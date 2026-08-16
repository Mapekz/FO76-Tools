import React, { useEffect, useState } from 'react'
import { useStore } from '../store'
import type { CoverageReport, Markers } from '../../../shared/api-types'
import { formatRecordType } from '../recordTypeNames'
import { listRecordTypeSigs } from '../lib/sigLists'

const DEFAULT_SAMPLE = 200
const ALL_TYPES = ''

/** Sums every `Markers` field except `records` — that field is the sample
 * count, not a gap counter. Deriving the sum this way (rather than naming
 * each gap field) means a newly added marker field joins the total for
 * free. Mirrors Rust `Markers::total()` in `esm/src/ipc.rs`, which excludes
 * `records` from its own sum the same way. */
function totalGaps(m: Markers): number {
  return (Object.entries(m) as [keyof Markers, number][])
    .filter(([key]) => key !== 'records')
    .reduce((sum, [, value]) => sum + value, 0)
}

export function CoveragePanel() {
  const { activeDbId } = useStore()
  const [sigs, setSigs] = useState<string[]>([])
  const [sig, setSig] = useState<string>(ALL_TYPES)
  const [sample, setSample] = useState(DEFAULT_SAMPLE)
  const [scanAll, setScanAll] = useState(false)
  const [report, setReport] = useState<CoverageReport | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!activeDbId) {
      setSigs([])
      return
    }
    listRecordTypeSigs(window.api, activeDbId)
      .then((list) => setSigs(list))
      .catch(console.error)
  }, [activeDbId])

  if (!activeDbId) return null

  async function runCoverage() {
    if (!activeDbId) return
    setLoading(true)
    setError(null)
    try {
      const effectiveSample = scanAll ? 0 : sample
      const res = await window.api.coverageReport(activeDbId, sig || undefined, effectiveSample)
      setReport(res)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }

  const rows = report
    ? Object.entries(report.by_type).toSorted(([sigA, a], [sigB, b]) => {
        const diff = totalGaps(b) - totalGaps(a)
        return diff !== 0 ? diff : sigA.localeCompare(sigB)
      })
    : []

  return (
    <div
      style={{
        padding: 8,
        fontSize: 12,
        display: 'flex',
        flexDirection: 'column',
        flex: 1,
        minHeight: 0,
      }}
    >
      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        <label style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
          Type:
          <select value={sig} onChange={(e) => setSig(e.target.value)} style={{ flex: 1 }}>
            <option value={ALL_TYPES}>All types (slower)</option>
            {sigs.map((s) => (
              <option key={s} value={s}>
                {formatRecordType(s)}
              </option>
            ))}
          </select>
        </label>

        <div style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
          <label style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
            Sample size:
            <input
              type="number"
              min={1}
              value={sample}
              disabled={scanAll}
              onChange={(e) => setSample(Math.max(1, Number(e.target.value) || 1))}
              style={{
                width: 80,
                background: scanAll ? '#222' : '#16213e',
                color: '#e0e0e0',
                border: '1px solid #444',
                borderRadius: 3,
                padding: '4px 6px',
                fontFamily: 'monospace',
              }}
            />
          </label>
          <label style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
            <input
              type="checkbox"
              checked={scanAll}
              onChange={(e) => setScanAll(e.target.checked)}
            />
            Scan all records of this type (0 = unlimited, can be slow)
          </label>
        </div>

        <button
          onClick={() => void runCoverage()}
          disabled={loading}
          style={{ alignSelf: 'flex-start' }}
        >
          {loading ? 'Scanning…' : 'Run'}
        </button>
      </div>

      {error && <div style={{ color: '#e88', marginTop: 6 }}>{error}</div>}

      {report && (
        <div style={{ overflow: 'auto', flex: 1, marginTop: 8 }}>
          <table
            style={{
              borderCollapse: 'collapse',
              width: '100%',
              fontFamily: 'monospace',
              fontSize: 11,
            }}
          >
            <thead>
              <tr style={{ borderBottom: '1px solid #444', textAlign: 'right' }}>
                <th style={{ textAlign: 'left', padding: '2px 6px' }}>Type</th>
                <th style={{ padding: '2px 6px' }}>Records</th>
                <th style={{ padding: '2px 6px' }}>Unknown</th>
                <th style={{ padding: '2px 6px' }}>Raw fallback</th>
                <th style={{ padding: '2px 6px' }}>Unmapped</th>
                <th style={{ padding: '2px 6px' }}>Unresolved</th>
                <th style={{ padding: '2px 6px' }}>Total gaps</th>
              </tr>
            </thead>
            <tbody>
              {rows.map(([typeSig, m]) => (
                <tr key={typeSig} style={{ borderBottom: '1px solid #2a2a3a', textAlign: 'right' }}>
                  <td style={{ textAlign: 'left', padding: '2px 6px', color: '#82aaff' }}>
                    {typeSig}
                  </td>
                  <td style={{ padding: '2px 6px' }}>{m.records}</td>
                  <td style={{ padding: '2px 6px' }}>{m.unknown_record}</td>
                  <td style={{ padding: '2px 6px' }}>{m.raw_fallback}</td>
                  <td style={{ padding: '2px 6px' }}>{m.unmapped}</td>
                  <td style={{ padding: '2px 6px' }}>{m.unresolved}</td>
                  <td
                    style={{
                      padding: '2px 6px',
                      fontWeight: 'bold',
                      color: totalGaps(m) > 0 ? '#e8a838' : '#c3e88d',
                    }}
                  >
                    {totalGaps(m)}
                  </td>
                </tr>
              ))}
            </tbody>
            <tfoot>
              <tr style={{ borderTop: '2px solid #444', textAlign: 'right', fontWeight: 'bold' }}>
                <td style={{ textAlign: 'left', padding: '4px 6px' }}>TOTAL</td>
                <td style={{ padding: '4px 6px' }}>{report.totals.records}</td>
                <td style={{ padding: '4px 6px' }}>{report.totals.unknown_record}</td>
                <td style={{ padding: '4px 6px' }}>{report.totals.raw_fallback}</td>
                <td style={{ padding: '4px 6px' }}>{report.totals.unmapped}</td>
                <td style={{ padding: '4px 6px' }}>{report.totals.unresolved}</td>
                <td style={{ padding: '4px 6px' }}>{totalGaps(report.totals)}</td>
              </tr>
            </tfoot>
          </table>
        </div>
      )}
    </div>
  )
}
