import React, { useCallback, useEffect, useState } from 'react'
import { OpenFilesPanel } from './components/OpenFilesPanel'
import { RecordTree } from './components/RecordTree'
import { RecordDetail } from './components/RecordDetail'
import { ReferencedByPanel } from './components/ReferencedByPanel'
import { NavHistory } from './components/NavHistory'
import { SearchPanel } from './components/SearchPanel'
import { FilterPanel } from './components/FilterPanel'
import { CoveragePanel } from './components/CoveragePanel'
import { useStore } from './store'

type LeftView = 'tree' | 'search' | 'filter' | 'coverage'

export function App() {
  const { setActiveRecord, setReferencedBy, navPush, navBack, navForward, referencedByDepth } =
    useStore()
  const [leftView, setLeftView] = useState<LeftView>('tree')

  // Fetch + display a record without touching nav history. Used for Back/Forward
  // (which already moved the history index themselves) and as the shared core
  // of `navigate` below.
  const loadRecord = useCallback(
    async (dbId: string, target: string) => {
      try {
        const rec = await window.api.recordById(dbId, target, 'stub')
        setActiveRecord(rec)
        const refs = await window.api.referencedById(dbId, target, referencedByDepth)
        setReferencedBy(refs)
      } catch (e) {
        console.error('load record error:', e)
      }
    },
    [setActiveRecord, setReferencedBy, referencedByDepth]
  )

  // A NEW navigation choice (tree click, ctrl-click FormID link, referenced-by row):
  // push history, then load.
  const navigate = useCallback(
    async (dbId: string, target: string) => {
      navPush({ dbId, formid: target })
      await loadRecord(dbId, target)
    },
    [navPush, loadRecord]
  )

  // Single shared implementation of Back/Forward, used by both the NavHistory
  // buttons and the Alt+Arrow keyboard shortcuts below.
  const goBack = useCallback(() => {
    const entry = navBack()
    if (entry) void loadRecord(entry.dbId, entry.formid)
  }, [navBack, loadRecord])

  const goForward = useCallback(() => {
    const entry = navForward()
    if (entry) void loadRecord(entry.dbId, entry.formid)
  }, [navForward, loadRecord])

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      const active = document.activeElement
      const isTextInput =
        active instanceof HTMLInputElement || active instanceof HTMLTextAreaElement
      if (isTextInput) return

      if (e.altKey && e.key === 'ArrowLeft') {
        e.preventDefault()
        goBack()
      } else if (e.altKey && e.key === 'ArrowRight') {
        e.preventDefault()
        goForward()
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [goBack, goForward])

  return (
    <div style={{ display: 'flex', height: '100vh', background: '#1a1a2e', color: '#e0e0e0', fontFamily: 'sans-serif' }}>
      {/* Left panel */}
      <div style={{ width: 320, borderRight: '1px solid #444', display: 'flex', flexDirection: 'column' }}>
        <OpenFilesPanel />
        <div style={{ display: 'flex', gap: 4, padding: '4px 8px', borderBottom: '1px solid #444' }}>
          {(['tree', 'search', 'filter', 'coverage'] as const).map((v) => (
            <button
              key={v}
              onClick={() => setLeftView(v)}
              style={{
                fontSize: 11,
                padding: '3px 8px',
                background: leftView === v ? '#33395a' : '#16213e',
                color: '#e0e0e0',
                border: '1px solid #444',
                borderRadius: 3,
                cursor: 'pointer'
              }}
            >
              {v === 'tree' ? 'Tree' : v === 'search' ? 'Search' : v === 'filter' ? 'Filter' : 'Coverage'}
            </button>
          ))}
        </div>
        {leftView === 'tree' && <RecordTree onNavigate={navigate} />}
        {leftView === 'search' && <SearchPanel onNavigate={navigate} />}
        {leftView === 'filter' && <FilterPanel onNavigate={navigate} />}
        {leftView === 'coverage' && <CoveragePanel />}
      </div>
      {/* Right panel */}
      <div style={{ flex: 1, display: 'flex', flexDirection: 'column' }}>
        <NavHistory onBack={goBack} onForward={goForward} />
        <RecordDetail onNavigate={navigate} />
        <ReferencedByPanel onNavigate={navigate} />
      </div>
    </div>
  )
}
