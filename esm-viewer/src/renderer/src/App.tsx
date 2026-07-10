import React, { useCallback, useEffect, useRef, useState } from 'react'
import { OpenFilesPanel } from './components/OpenFilesPanel'
import { RecordTree } from './components/RecordTree'
import { RecordDetail } from './components/RecordDetail'
import { ReferencedByPanel } from './components/ReferencedByPanel'
import { NavHistory } from './components/NavHistory'
import { SearchPanel } from './components/SearchPanel'
import { FilterPanel } from './components/FilterPanel'
import { CoveragePanel } from './components/CoveragePanel'
import { DiffPanel } from './components/DiffPanel'
import { useStore } from './store'
import { buildRecordColumns } from './lib/recordColumns'
import { fetchReferencedBy } from './lib/referencedBy'

type LeftView = 'tree' | 'search' | 'filter' | 'coverage' | 'diff'

export function App() {
  const {
    setActiveRecord,
    setRecordColumns,
    setReferencedBy,
    navPush,
    navBack,
    navForward,
    referencedByDepth,
  } = useStore()
  const [leftView, setLeftView] = useState<LeftView>('tree')

  // Out-of-order-response guard: rapid navigation can fire several loadRecord
  // calls before earlier ones resolve. Each call captures its own sequence
  // number and bails before touching the store if a later call has since won.
  const loadSeq = useRef(0)

  // Fetch + display a record without touching nav history. Used for Back/Forward
  // (which already moved the history index themselves) and as the shared core
  // of `navigate` below. Also builds one xEdit-style column per open file that
  // contains the resolved FormID (auto-columns).
  const loadRecord = useCallback(
    async (dbId: string, target: string) => {
      const seq = ++loadSeq.current
      try {
        // Read openDbs fresh (not from the component's closure) so this callback's
        // identity doesn't have to change every time a file is opened/closed.
        const { openDbs } = useStore.getState()
        const { active, columns } = await buildRecordColumns(target, dbId, openDbs, window.api)
        if (seq !== loadSeq.current) return

        setActiveRecord(active)
        setRecordColumns(columns)

        const refs = await fetchReferencedBy(dbId, target, referencedByDepth, window.api)
        if (seq !== loadSeq.current) return
        setReferencedBy(refs)
      } catch (e) {
        console.error('load record error:', e)
      }
    },
    [setActiveRecord, setRecordColumns, setReferencedBy, referencedByDepth]
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

  // Single shared implementation of Back/Forward, used by the NavHistory buttons
  // and the Alt+Arrow / media-key / mouse X-button shortcuts below.
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
      // Media-keyboard back/forward keys never type text, so they navigate even
      // while an input is focused.
      if (e.key === 'BrowserBack') {
        e.preventDefault()
        goBack()
        return
      } else if (e.key === 'BrowserForward') {
        e.preventDefault()
        goForward()
        return
      }

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
    // Mouse X-buttons (back = 3, forward = 4). preventDefault on mousedown too,
    // so Chromium never treats them as page-history navigation.
    function onMouseDown(e: MouseEvent) {
      if (e.button === 3 || e.button === 4) e.preventDefault()
    }
    function onMouseUp(e: MouseEvent) {
      if (e.button === 3) {
        e.preventDefault()
        goBack()
      } else if (e.button === 4) {
        e.preventDefault()
        goForward()
      }
    }
    window.addEventListener('keydown', onKeyDown)
    window.addEventListener('mousedown', onMouseDown)
    window.addEventListener('mouseup', onMouseUp)
    return () => {
      window.removeEventListener('keydown', onKeyDown)
      window.removeEventListener('mousedown', onMouseDown)
      window.removeEventListener('mouseup', onMouseUp)
    }
  }, [goBack, goForward])

  return (
    <div style={{ display: 'flex', height: '100vh', background: '#1a1a2e', color: '#e0e0e0', fontFamily: 'sans-serif' }}>
      {/* Left panel */}
      <div style={{ width: 320, borderRight: '1px solid #444', display: 'flex', flexDirection: 'column' }}>
        <OpenFilesPanel />
        <div style={{ display: 'flex', gap: 4, padding: '4px 8px', borderBottom: '1px solid #444' }}>
          {(['tree', 'search', 'filter', 'coverage', 'diff'] as const).map((v) => (
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
              {v === 'tree'
                ? 'Tree'
                : v === 'search'
                  ? 'Search'
                  : v === 'filter'
                    ? 'Filter'
                    : v === 'coverage'
                      ? 'Coverage'
                      : 'Diff'}
            </button>
          ))}
        </div>
        {leftView === 'tree' && <RecordTree onNavigate={navigate} />}
        {leftView === 'search' && <SearchPanel onNavigate={navigate} />}
        {leftView === 'filter' && <FilterPanel onNavigate={navigate} />}
        {leftView === 'coverage' && <CoveragePanel />}
        {leftView === 'diff' && <DiffPanel onNavigate={navigate} />}
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
