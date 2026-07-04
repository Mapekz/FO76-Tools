import React, { useEffect, useState } from 'react'
import { useStore } from '../store'
import type { DiffResult, RecordStubDiff, RecordChangeDiff } from '../../../shared/api-types'

interface Props {
  onNavigate: (dbId: string, formid: string) => void
}

type SectionKey = 'added' | 'removed' | 'changed'

function basename(path: string): string {
  return path.split(/[\\/]/).pop() ?? path
}

function isLeafChange(node: unknown): node is { from: unknown; to: unknown } {
  if (typeof node !== 'object' || node === null || Array.isArray(node)) return false
  const keys = Object.keys(node as Record<string, unknown>)
  return keys.length === 2 && keys.includes('from') && keys.includes('to')
}

function formatVal(v: unknown): string {
  if (v === null || v === undefined) return 'null'
  if (typeof v === 'object') return JSON.stringify(v)
  return String(v)
}

/** Recursive renderer for a `field_changes` sparse tree: a `{from, to}` node is
 * a leaf (rendered as a colored diff); `_array_diff` nodes render as raw JSON
 * for this first pass (see note below); everything else recurses as a nested
 * object, mirroring `RecordDetail.tsx`'s `FieldValue` indentation style. */
function FieldChangeNode({ fieldKey, node }: { fieldKey: string; node: unknown }) {
  if (fieldKey === '_array_diff') {
    // Known simplification: keyed per-element array diffs are rendered as raw
    // JSON here rather than a bespoke array-diff UI. Not a bug — a follow-up
    // could build a proper added/removed/reordered element view.
    return (
      <div style={{ paddingLeft: 16 }}>
        <span style={{ color: '#aaa' }}>{fieldKey}</span>:{' '}
        <code style={{ fontSize: 11, wordBreak: 'break-all' }}>{JSON.stringify(node)}</code>
      </div>
    )
  }
  if (isLeafChange(node)) {
    return (
      <div style={{ paddingLeft: 16 }}>
        <span style={{ color: '#82aaff' }}>{fieldKey}</span>:{' '}
        <span style={{ color: '#e88' }}>{formatVal(node.from)}</span>{' '}
        <span style={{ color: '#aaa' }}>&rarr;</span>{' '}
        <span style={{ color: '#c3e88d' }}>{formatVal(node.to)}</span>
      </div>
    )
  }
  if (typeof node === 'object' && node !== null && !Array.isArray(node)) {
    const obj = node as Record<string, unknown>
    return (
      <div style={{ paddingLeft: 16 }}>
        <span style={{ color: '#82aaff' }}>{fieldKey}</span>
        {Object.entries(obj).map(([k, v]) => (
          <FieldChangeNode key={k} fieldKey={k} node={v} />
        ))}
      </div>
    )
  }
  return (
    <div style={{ paddingLeft: 16 }}>
      <span style={{ color: '#82aaff' }}>{fieldKey}</span>: {formatVal(node)}
    </div>
  )
}

function StubRow({ row, onClick }: { row: RecordStubDiff; onClick: () => void }) {
  return (
    <div style={{ cursor: 'pointer', padding: '2px 0' }} onClick={onClick}>
      <span style={{ fontFamily: 'monospace', color: '#7ec8e3' }}>{row.form_id}</span>{' '}
      <span style={{ color: '#aaa' }}>[{row.record_type}]</span>{' '}
      {row.editor_id && <span style={{ color: '#aaa' }}>[{row.editor_id}]</span>}{' '}
      {row.name && <span>{row.name}</span>}
    </div>
  )
}

function ChangedRow({ change, onClick }: { change: RecordChangeDiff; onClick: () => void }) {
  const { stub, field_changes, prev_editor_id } = change
  const changesObj =
    typeof field_changes === 'object' && field_changes !== null && !Array.isArray(field_changes)
      ? (field_changes as Record<string, unknown>)
      : {}
  return (
    <div style={{ marginBottom: 8, borderBottom: '1px solid #222', paddingBottom: 6 }}>
      <div style={{ cursor: 'pointer' }} onClick={onClick}>
        <span style={{ fontFamily: 'monospace', color: '#7ec8e3' }}>{stub.form_id}</span>{' '}
        <span style={{ color: '#aaa' }}>[{stub.record_type}]</span>{' '}
        {stub.editor_id && <span style={{ color: '#aaa' }}>[{stub.editor_id}]</span>}{' '}
        {stub.name && <span>{stub.name}</span>}
        {prev_editor_id && (
          <span style={{ color: '#e8a838', marginLeft: 6, fontSize: 11 }}>
            renamed from &quot;{prev_editor_id}&quot;
          </span>
        )}
      </div>
      <div>
        {Object.entries(changesObj).map(([k, v]) => (
          <FieldChangeNode key={k} fieldKey={k} node={v} />
        ))}
      </div>
    </div>
  )
}

function Section({
  title,
  count,
  expanded,
  onToggle,
  children,
}: {
  title: string
  count: number
  expanded: boolean
  onToggle: () => void
  children: React.ReactNode
}) {
  return (
    <div style={{ marginBottom: 8 }}>
      <div
        onClick={onToggle}
        style={{
          cursor: 'pointer',
          fontWeight: 'bold',
          padding: '4px 6px',
          background: '#16213e',
          borderBottom: '1px solid #333',
        }}
      >
        {expanded ? '▼' : '▶'} {title} ({count})
      </div>
      {expanded && <div style={{ paddingLeft: 8, paddingTop: 4 }}>{children}</div>}
    </div>
  )
}

export function DiffPanel({ onNavigate }: Props) {
  const { openDbs, activeDbId } = useStore()
  const [oldId, setOldId] = useState('')
  const [newId, setNewId] = useState('')
  const [recordType, setRecordType] = useState('')
  const [bodies, setBodies] = useState<'none' | 'stub' | 'full'>('full')
  const [suppressNoise, setSuppressNoise] = useState(true)
  const [excludeTypes, setExcludeTypes] = useState('')
  const [result, setResult] = useState<DiffResult | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [expanded, setExpanded] = useState<Record<SectionKey, boolean>>({
    added: true,
    removed: true,
    changed: true,
  })

  // Default "Old" to the first open DB, keeping the current selection if it's
  // still a valid open DB.
  useEffect(() => {
    if (openDbs.length < 2) return
    setOldId((prev) => (prev && openDbs.some((d) => d.id === prev) ? prev : openDbs[0].id))
  }, [openDbs])

  // Default "New" to the active DB (if it differs from "Old") or else the next
  // open DB after "Old" — and never let it collapse onto the same DB as "Old".
  useEffect(() => {
    if (openDbs.length < 2 || !oldId) return
    setNewId((prev) => {
      if (prev && prev !== oldId && openDbs.some((d) => d.id === prev)) return prev
      if (activeDbId && activeDbId !== oldId && openDbs.some((d) => d.id === activeDbId)) {
        return activeDbId
      }
      return openDbs.find((d) => d.id !== oldId)?.id ?? oldId
    })
  }, [openDbs, oldId, activeDbId])

  if (openDbs.length < 2) {
    return (
      <div style={{ padding: 16, color: '#666' }}>
        Open at least two ESM files to compare them.
      </div>
    )
  }

  function toggleSection(key: SectionKey) {
    setExpanded((e) => ({ ...e, [key]: !e[key] }))
  }

  async function runDiff() {
    if (!oldId || !newId) return
    setLoading(true)
    setError(null)
    try {
      const excludeList = excludeTypes
        .split(',')
        .map((s) => s.trim().toUpperCase())
        .filter((s) => s.length > 0)
      const res = await window.api.diff(
        oldId,
        newId,
        recordType.trim() || undefined,
        bodies,
        suppressNoise,
        excludeList
      )
      setResult(res)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }

  const suppressedEntries = result ? Object.entries(result.suppressed_counts) : []

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
          Old (base):
          <select value={oldId} onChange={(e) => setOldId(e.target.value)} style={{ flex: 1 }}>
            {openDbs.map((db) => (
              <option key={db.id} value={db.id}>
                {basename(db.path)}
              </option>
            ))}
          </select>
        </label>
        <label style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
          New (compare):
          <select value={newId} onChange={(e) => setNewId(e.target.value)} style={{ flex: 1 }}>
            {openDbs.map((db) => (
              <option key={db.id} value={db.id}>
                {basename(db.path)}
              </option>
            ))}
          </select>
        </label>

        {oldId && newId && oldId === newId && (
          <div style={{ color: '#e8a838', fontSize: 11 }}>
            Old and New are the same database — this compares it to itself (expect empty results).
          </div>
        )}

        <div style={{ display: 'flex', gap: 6, alignItems: 'center', flexWrap: 'wrap' }}>
          <input
            type="text"
            value={recordType}
            onChange={(e) => setRecordType(e.target.value.toUpperCase())}
            placeholder="Type (blank = all)"
            maxLength={4}
            style={{
              width: 130,
              background: '#16213e',
              color: '#e0e0e0',
              border: '1px solid #444',
              borderRadius: 3,
              padding: '4px 6px',
              fontFamily: 'monospace',
            }}
          />
          <label style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
            Bodies:
            <select value={bodies} onChange={(e) => setBodies(e.target.value as 'none' | 'stub' | 'full')}>
              <option value="none">None</option>
              <option value="stub">Stub</option>
              <option value="full">Full</option>
            </select>
          </label>
          <label style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
            <input
              type="checkbox"
              checked={suppressNoise}
              onChange={(e) => setSuppressNoise(e.target.checked)}
            />
            Suppress noise
          </label>
        </div>

        <input
          type="text"
          value={excludeTypes}
          onChange={(e) => setExcludeTypes(e.target.value)}
          placeholder="Exclude types, comma-separated (e.g. LAND,NAVM)"
          style={{
            background: '#16213e',
            color: '#e0e0e0',
            border: '1px solid #444',
            borderRadius: 3,
            padding: '4px 6px',
            fontFamily: 'monospace',
          }}
        />

        <button onClick={() => void runDiff()} disabled={loading || !oldId || !newId} style={{ alignSelf: 'flex-start' }}>
          {loading ? 'Diffing…' : 'Run Diff'}
        </button>
      </div>

      {error && <div style={{ color: '#e88', marginTop: 6 }}>{error}</div>}

      {result && (
        <div style={{ overflowY: 'auto', flex: 1, marginTop: 8 }}>
          {suppressedEntries.length > 0 && (
            <div style={{ color: '#aaa', fontSize: 11, marginBottom: 8 }}>
              Noise suppressed (hidden by default, not lost data):{' '}
              {suppressedEntries.map(([t, n]) => `${n} ${t}`).join(', ')}
            </div>
          )}
          <Section
            title="Added"
            count={result.added.length}
            expanded={expanded.added}
            onToggle={() => toggleSection('added')}
          >
            {result.added.map((row) => (
              <StubRow key={row.form_id} row={row} onClick={() => onNavigate(newId, row.form_id)} />
            ))}
          </Section>
          <Section
            title="Removed"
            count={result.removed.length}
            expanded={expanded.removed}
            onToggle={() => toggleSection('removed')}
          >
            {result.removed.map((row) => (
              <StubRow key={row.form_id} row={row} onClick={() => onNavigate(oldId, row.form_id)} />
            ))}
          </Section>
          <Section
            title="Changed"
            count={result.changed.length}
            expanded={expanded.changed}
            onToggle={() => toggleSection('changed')}
          >
            {result.changed.map((c) => (
              <ChangedRow
                key={c.stub.form_id}
                change={c}
                onClick={() => onNavigate(newId, c.stub.form_id)}
              />
            ))}
          </Section>
        </div>
      )}
    </div>
  )
}
