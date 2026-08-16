import React from 'react'
import { useStore } from '../store'
import { colors } from '../theme'

export function OpenFilesPanel() {
  const {
    openDbs,
    activeDbId,
    recordColumns,
    setOpenDbs,
    setActiveDb,
    setActiveRecord,
    setRecordColumns,
    setReferencedBy,
  } = useStore()

  async function handleOpenPath(path: string | null) {
    if (!path) return
    try {
      const handle = await window.api.openDatabase(path)
      const all = await window.api.listOpen()
      setOpenDbs(all)
      setActiveDb(handle.id)
    } catch (e) {
      alert(String(e))
    }
  }

  async function handleOpen() {
    void handleOpenPath(await window.api.openFileDialog())
  }

  async function handleOpenFolder() {
    void handleOpenPath(await window.api.openFolderDialog())
  }

  async function handleClose(id: string) {
    await window.api.closeDatabase(id)
    const all = await window.api.listOpen()
    setOpenDbs(all)
    // Drop the closed file's column so RecordTable doesn't keep rendering a
    // now-invalid dbId; other open files stay put until the next navigation.
    setRecordColumns(recordColumns.filter((c) => c.dbId !== id))
    if (activeDbId === id) {
      setActiveDb(all[0]?.id ?? null)
      setActiveRecord(null)
      setReferencedBy({
        target: '',
        rows: [],
        total: 0,
        capped: false,
        requested_depth: 0,
        effective_depth: null,
        depth_capped: false,
        frontier_remaining: 0,
        per_depth_totals: [],
        shown_max_depth: 0,
      })
    }
  }

  return (
    <div style={{ padding: 8, borderBottom: `1px solid ${colors.seam}` }}>
      <button onClick={handleOpen}>Open ESM…</button>
      <button onClick={handleOpenFolder} style={{ marginLeft: 4 }}>
        Open Folder…
      </button>
      <ul style={{ listStyle: 'none', margin: '8px 0 0', padding: 0 }}>
        {openDbs.map((db) => (
          <li
            key={db.id}
            style={{
              display: 'flex',
              gap: 8,
              alignItems: 'center',
              background: db.id === activeDbId ? colors.hoverGraphite : 'transparent',
              padding: '2px 4px',
              cursor: 'pointer',
            }}
            onClick={() => setActiveDb(db.id)}
          >
            <span style={{ flex: 1, fontSize: 12, overflow: 'hidden', textOverflow: 'ellipsis' }}>
              {db.path.split('/').pop()}
            </span>
            <button
              onClick={(e) => {
                e.stopPropagation()
                void handleClose(db.id)
              }}
              style={{ fontSize: 10, padding: '1px 4px' }}
            >
              ✕
            </button>
          </li>
        ))}
      </ul>
    </div>
  )
}
