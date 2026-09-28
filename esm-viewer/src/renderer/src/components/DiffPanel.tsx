import React, { useEffect, useState } from 'react'
import { useStore } from '../store'
import type { DiffResult, RecordStubDiff, RecordChangeDiff } from '../../../shared/api-types'
import { parseSigList } from '../lib/sigLists'
import { useAsyncAction } from '../lib/useAsyncAction'
import { RecordRef } from './RecordRef'
import { colors, panelStyle, inputStyle } from '../theme'

type SectionKey = 'added' | 'removed' | 'changed'

function basename(path: string): string {
  return path.split(/[\\/]/).pop() ?? path
}

/** Recursively counts `{from, to}` leaves in a `field_changes` sparse tree for
 * the one-line "N fields changed" summary; an `_array_diff` node (keyed
 * per-element array diff) counts as a single change rather than being
 * expanded — the detail pane's side-by-side columns are where you inspect
 * what changed. */
function countFieldChanges(node: unknown): number {
  if (typeof node !== 'object' || node === null || Array.isArray(node)) return 0
  const obj = node as Record<string, unknown>
  const keys = Object.keys(obj)
  if (keys.length === 2 && keys.includes('from') && keys.includes('to')) return 1
  let total = 0
  for (const [k, v] of Object.entries(obj)) {
    total += k === '_array_diff' ? 1 : countFieldChanges(v)
  }
  return total
}

function StubRow({ row, dbId }: { row: RecordStubDiff; dbId: string }) {
  return (
    <RecordRef
      dbId={dbId}
      formId={row.form_id}
      recordType={row.record_type}
      editorId={row.editor_id}
      name={row.name}
    />
  )
}

function ChangedRow({ change, dbId }: { change: RecordChangeDiff; dbId: string }) {
  const { stub, field_changes, prev_editor_id } = change
  const changeCount = countFieldChanges(field_changes)
  return (
    <div
      style={{ marginBottom: 4, borderBottom: `1px solid ${colors.hairline}`, paddingBottom: 4 }}
    >
      <RecordRef
        dbId={dbId}
        formId={stub.form_id}
        recordType={stub.record_type}
        editorId={stub.editor_id}
        name={stub.name}
        style={{ padding: 0 }}
      >
        {prev_editor_id && (
          <span style={{ color: colors.gapAmber, marginLeft: 6, fontSize: 11 }}>
            renamed from &quot;{prev_editor_id}&quot;
          </span>
        )}
      </RecordRef>
      <div style={{ color: colors.dimReadout, fontSize: 11 }}>
        {changeCount} {changeCount === 1 ? 'field' : 'fields'} changed
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
          background: colors.panelSteel,
          borderBottom: `1px solid ${colors.rule}`,
        }}
      >
        {expanded ? '▼' : '▶'} {title} ({count})
      </div>
      {expanded && <div style={{ paddingLeft: 8, paddingTop: 4 }}>{children}</div>}
    </div>
  )
}

export function DiffPanel() {
  const { openDbs, activeDbId } = useStore()
  const [oldId, setOldId] = useState('')
  const [newId, setNewId] = useState('')
  const [recordType, setRecordType] = useState('')
  const [suppressNoise, setSuppressNoise] = useState(true)
  const [excludeTypes, setExcludeTypes] = useState('')
  // The result remembers the files it compared, so its rows open there even
  // after the Old/New selectors change.
  const [result, setResult] = useState<(DiffResult & { oldId: string; newId: string }) | null>(null)
  const { loading, error, run } = useAsyncAction()
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
      <div style={{ padding: 16, color: colors.ghostText }}>
        Open at least two ESM files to compare them.
      </div>
    )
  }

  function toggleSection(key: SectionKey) {
    setExpanded((e) => ({ ...e, [key]: !e[key] }))
  }

  async function runDiff() {
    if (!oldId || !newId) return
    const [from, to] = [oldId, newId]
    await run(async () => {
      const excludeList = parseSigList(excludeTypes)
      const res = await window.api.diff(from, to, {
        record_type: recordType.trim() || null,
        options: {
          // Bodies are never rendered here (see ChangedRow's one-line summary +
          // the detail pane's side-by-side columns), so skip decoding them —
          // `field_changes` is computed regardless.
          bodies: 'none',
          suppress_noise: suppressNoise,
          exclude_types: excludeList,
        },
      })
      setResult({ ...res, oldId: from, newId: to })
    })
  }

  const suppressedEntries = result ? Object.entries(result.suppressed_counts ?? {}) : []

  return (
    <div style={panelStyle}>
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
          <div style={{ color: colors.gapAmber, fontSize: 11 }}>
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
            style={{ ...inputStyle, width: 130 }}
          />
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
          style={inputStyle}
        />

        <button
          onClick={() => void runDiff()}
          disabled={loading || !oldId || !newId}
          style={{ alignSelf: 'flex-start' }}
        >
          {loading ? 'Diffing…' : 'Run Diff'}
        </button>
      </div>

      {error && <div style={{ color: colors.faultRed, marginTop: 6 }}>{error}</div>}

      {result && (
        <div style={{ overflowY: 'auto', flex: 1, marginTop: 8 }}>
          {suppressedEntries.length > 0 && (
            <div style={{ color: colors.dimReadout, fontSize: 11, marginBottom: 8 }}>
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
              <StubRow key={row.form_id} row={row} dbId={result.newId} />
            ))}
          </Section>
          <Section
            title="Removed"
            count={result.removed.length}
            expanded={expanded.removed}
            onToggle={() => toggleSection('removed')}
          >
            {result.removed.map((row) => (
              <StubRow key={row.form_id} row={row} dbId={result.oldId} />
            ))}
          </Section>
          <Section
            title="Changed"
            count={result.changed.length}
            expanded={expanded.changed}
            onToggle={() => toggleSection('changed')}
          >
            {result.changed.map((c) => (
              <ChangedRow key={c.stub.form_id} change={c} dbId={result.newId} />
            ))}
          </Section>
        </div>
      )}
    </div>
  )
}
