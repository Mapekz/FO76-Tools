import React, { useEffect, useState } from 'react'
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
import { colors } from './theme'

type LeftView = 'tree' | 'search' | 'filter' | 'coverage' | 'diff'

const LEFT_VIEW_LABELS: Record<LeftView, string> = {
  tree: 'Tree',
  search: 'Search',
  filter: 'Filter',
  coverage: 'Coverage',
  diff: 'Diff',
}

export function App() {
  const { goBack, goForward } = useStore()
  const [leftView, setLeftView] = useState<LeftView>('tree')

  // Back/Forward shortcuts: Alt+Arrow, media keys and mouse X-buttons.
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      // Media-keyboard back/forward keys never type text, so they navigate even
      // while an input is focused.
      if (e.key === 'BrowserBack') {
        e.preventDefault()
        void goBack()
        return
      } else if (e.key === 'BrowserForward') {
        e.preventDefault()
        void goForward()
        return
      }

      const active = document.activeElement
      const isTextInput =
        active instanceof HTMLInputElement || active instanceof HTMLTextAreaElement
      if (isTextInput) return

      if (e.altKey && e.key === 'ArrowLeft') {
        e.preventDefault()
        void goBack()
      } else if (e.altKey && e.key === 'ArrowRight') {
        e.preventDefault()
        void goForward()
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
        void goBack()
      } else if (e.button === 4) {
        e.preventDefault()
        void goForward()
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
    <div
      style={{
        display: 'flex',
        height: '100vh',
        background: colors.workbenchBlack,
        color: colors.benchLight,
        fontFamily: 'sans-serif',
      }}
    >
      {/* Left panel */}
      <div
        style={{
          width: 320,
          borderRight: `1px solid ${colors.seam}`,
          display: 'flex',
          flexDirection: 'column',
        }}
      >
        <OpenFilesPanel />
        <div
          style={{
            display: 'flex',
            gap: 4,
            padding: '4px 8px',
            borderBottom: `1px solid ${colors.seam}`,
          }}
        >
          {(['tree', 'search', 'filter', 'coverage', 'diff'] as const).map((v) => (
            <button
              key={v}
              onClick={() => setLeftView(v)}
              style={{
                fontSize: 11,
                padding: '3px 8px',
                background: leftView === v ? colors.focusIndigo : colors.panelSteel,
                color: colors.benchLight,
                border: `1px solid ${colors.seam}`,
                borderRadius: 3,
                cursor: 'pointer',
              }}
            >
              {LEFT_VIEW_LABELS[v]}
            </button>
          ))}
        </div>
        {leftView === 'tree' && <RecordTree />}
        {leftView === 'search' && <SearchPanel />}
        {leftView === 'filter' && <FilterPanel />}
        {leftView === 'coverage' && <CoveragePanel />}
        {leftView === 'diff' && <DiffPanel />}
      </div>
      {/* Right panel */}
      <div style={{ flex: 1, display: 'flex', flexDirection: 'column' }}>
        <NavHistory />
        <RecordDetail />
        <ReferencedByPanel />
      </div>
    </div>
  )
}
