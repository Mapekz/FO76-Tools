import React, { useCallback } from 'react'
import { OpenFilesPanel } from './components/OpenFilesPanel'
import { RecordTree } from './components/RecordTree'
import { RecordDetail } from './components/RecordDetail'
import { ReferencedByPanel } from './components/ReferencedByPanel'
import { NavHistory } from './components/NavHistory'
import { useStore } from './store'

export function App() {
  const { setActiveRecord, setReferencedBy, navPush } = useStore()

  const navigate = useCallback(async (dbId: string, target: string) => {
    navPush({ dbId, formid: target })
    try {
      const rec = await window.api.recordById(dbId, target, 'stub')
      setActiveRecord(rec)
      const refs = await window.api.referencedById(dbId, target)
      setReferencedBy(refs)
    } catch (e) {
      console.error('navigate error:', e)
    }
  }, [navPush, setActiveRecord, setReferencedBy])

  return (
    <div style={{ display: 'flex', height: '100vh', background: '#1a1a2e', color: '#e0e0e0', fontFamily: 'sans-serif' }}>
      {/* Left panel */}
      <div style={{ width: 320, borderRight: '1px solid #444', display: 'flex', flexDirection: 'column' }}>
        <OpenFilesPanel />
        <RecordTree />
      </div>
      {/* Right panel */}
      <div style={{ flex: 1, display: 'flex', flexDirection: 'column' }}>
        <NavHistory onNavigate={navigate} />
        <RecordDetail onNavigate={navigate} />
        <ReferencedByPanel onNavigate={navigate} />
      </div>
    </div>
  )
}
