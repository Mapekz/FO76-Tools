import React from 'react'
import { useStore } from '../store'

export function OpenFilesPanel() {
  const { openDbs, activeDbId, setOpenDbs, setActiveDb, setActiveRecord, setReferencedBy } = useStore()

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
    handleOpenPath(await window.api.openFileDialog())
  }

  async function handleOpenFolder() {
    handleOpenPath(await window.api.openFolderDialog())
  }

  async function handleClose(id: string) {
    await window.api.closeDatabase(id)
    const all = await window.api.listOpen()
    setOpenDbs(all)
    if (activeDbId === id) {
      setActiveDb(all[0]?.id ?? null)
      setActiveRecord(null)
      setReferencedBy({ target: '', rows: [], total: 0, capped: false })
    }
  }

  return (
    <div style={{ padding: 8, borderBottom: '1px solid #444' }}>
      <button onClick={handleOpen}>Open ESM…</button>
      <button onClick={handleOpenFolder} style={{ marginLeft: 4 }}>Open Folder…</button>
      <ul style={{ listStyle: 'none', margin: '8px 0 0', padding: 0 }}>
        {openDbs.map((db) => (
          <li
            key={db.id}
            style={{
              display: 'flex',
              gap: 8,
              alignItems: 'center',
              background: db.id === activeDbId ? '#2a2a3a' : 'transparent',
              padding: '2px 4px',
              cursor: 'pointer',
            }}
            onClick={() => setActiveDb(db.id)}
          >
            <span style={{ flex: 1, fontSize: 12, overflow: 'hidden', textOverflow: 'ellipsis' }}>
              {db.path.split('/').pop()}
            </span>
            <button
              onClick={(e) => { e.stopPropagation(); void handleClose(db.id) }}
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
