import React from 'react'
import { useStore } from '../store'
import { colors } from '../theme'

export function OpenFilesPanel() {
  const { openDbs, activeDbId, setOpenDbs, selectFile, fileClosed } = useStore()

  async function handleOpenPath(path: string | null) {
    if (!path) return
    try {
      const handle = await window.api.openDatabase(path)
      setOpenDbs(await window.api.listOpen())
      await selectFile(handle.id)
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
    fileClosed(id, await window.api.listOpen())
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
            onClick={() => void selectFile(db.id)}
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
