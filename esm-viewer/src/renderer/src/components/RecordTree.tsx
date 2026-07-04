import React, { useState, useEffect } from 'react'
import { useStore } from '../store'
import type { RecordRow } from '../../../shared/api-types'

interface GroupEntry {
  sig: string
  child_count: number
}

export function RecordTree() {
  const { activeDbId, navPush, setActiveRecord, setReferencedBy } = useStore()
  const [groups, setGroups] = useState<GroupEntry[]>([])
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const [rows, setRows] = useState<Record<string, RecordRow[]>>({})
  const [loading, setLoading] = useState<Set<string>>(new Set())

  useEffect(() => {
    if (!activeDbId) { setGroups([]); return }
    window.api.listGroups(activeDbId).then((gs) => {
      const parsed: GroupEntry[] = gs.map((g) => {
        const label = g.label as Record<string, unknown>
        const recType = label['RecordType'] as Record<string, unknown> | undefined
        const sig = (recType?.['sig'] as string | undefined) ??
                    (typeof label['sig'] === 'string' ? label['sig'] : '????')
        return { sig, child_count: g.child_count }
      })
      setGroups(parsed.filter(g => g.child_count > 0))
      setExpanded(new Set())
      setRows({})
    }).catch(console.error)
  }, [activeDbId])

  async function toggleGroup(sig: string) {
    if (expanded.has(sig)) {
      setExpanded((s) => { const n = new Set(s); n.delete(sig); return n })
      return
    }
    setExpanded((s) => new Set([...s, sig]))
    if (rows[sig]) return  // already loaded
    if (!activeDbId) return
    setLoading((s) => new Set([...s, sig]))
    try {
      const page = await window.api.listTypeRecords(activeDbId, sig, 0, 100)
      setRows((r) => ({ ...r, [sig]: page }))
    } finally {
      setLoading((s) => { const n = new Set(s); n.delete(sig); return n })
    }
  }

  async function loadMore(sig: string) {
    if (!activeDbId) return
    const current = rows[sig] ?? []
    const next = await window.api.listTypeRecords(activeDbId, sig, current.length, 100)
    setRows((r) => ({ ...r, [sig]: [...current, ...next] }))
  }

  async function selectRecord(row: RecordRow) {
    if (!activeDbId) return
    navPush({ dbId: activeDbId, formid: row.form_id })
    try {
      const rec = await window.api.recordById(activeDbId, row.form_id, 'stub')
      setActiveRecord(rec)
      const refs = await window.api.referencedById(activeDbId, row.form_id)
      setReferencedBy(refs)
    } catch (e) {
      console.error(e)
    }
  }

  return (
    <div style={{ overflowY: 'auto', flex: 1, fontSize: 12 }}>
      {groups.map((g) => (
        <div key={g.sig}>
          <div
            onClick={() => void toggleGroup(g.sig)}
            style={{ padding: '3px 8px', cursor: 'pointer', background: '#1e1e2e', borderBottom: '1px solid #333' }}
          >
            {expanded.has(g.sig) ? '▼' : '▶'} {g.sig} ({g.child_count})
          </div>
          {expanded.has(g.sig) && (
            <div>
              {loading.has(g.sig) && <div style={{ padding: 4 }}>Loading…</div>}
              <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 11 }}>
                <thead>
                  <tr style={{ background: '#16213e' }}>
                    <th style={{ padding: '2px 6px', textAlign: 'left' }}>FormID</th>
                    <th style={{ padding: '2px 6px', textAlign: 'left' }}>EditorID</th>
                    <th style={{ padding: '2px 6px', textAlign: 'left' }}>Name</th>
                  </tr>
                </thead>
                <tbody>
                  {(rows[g.sig] ?? []).map((row) => (
                    <tr
                      key={row.form_id}
                      onClick={() => void selectRecord(row)}
                      style={{ cursor: 'pointer', borderBottom: '1px solid #222' }}
                    >
                      <td style={{ padding: '2px 6px', fontFamily: 'monospace' }}>{row.form_id}</td>
                      <td style={{ padding: '2px 6px' }}>{row.editor_id ?? ''}</td>
                      <td style={{ padding: '2px 6px' }}>{row.name ?? ''}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
              {(rows[g.sig]?.length ?? 0) < g.child_count && (
                <button onClick={() => void loadMore(g.sig)} style={{ margin: 4, fontSize: 11 }}>
                  Load more…
                </button>
              )}
            </div>
          )}
        </div>
      ))}
    </div>
  )
}
