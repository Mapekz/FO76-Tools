import React, { useMemo, useState } from 'react'
import { useStore, type RecordColumn } from '../store'
import { buildAlignedTree, buildLeafNode, isFormIdStub, MISSING, type AlignedNode } from '../lib/alignedTree'

interface Props {
  columns: RecordColumn[]
  activeDbId: string | null
  onNavigate: (dbId: string, formid: string) => void
}

const PROPERTY_COL_WIDTH = 280
const VALUE_COL_WIDTH = 200

/** Paths of the two hand-made rows synthesized ahead of the decoded `fields`
 * tree (see `buildSyntheticNodes` below) — used to special-case their default
 * collapse state and to give them stable React/collapse-toggle keys. */
const HEADER_PATH = '__header'
const EDID_PATH = '__edid'

/** `buildAlignedTree`'s array branches are the only place children get `[i]`
 * labels — used as a cheap way to tell "this is an array node" apart from an
 * object node without adding a `kind` field to `AlignedNode` itself. */
function isArrayNode(node: AlignedNode): boolean {
  return node.children.length > 0 && node.children.every((c) => /^\[\d+\]$/.test(c.label))
}

/** Initial collapse state (before any manual toggle or Expand/Collapse all):
 * everything expanded except the synthesized Record Header branch and array
 * nodes with more than 50 elements (keeps WRLD/QUST-scale records mountable). */
function defaultNodeExpanded(node: AlignedNode): boolean {
  if (node.path === HEADER_PATH) return false
  if (isArrayNode(node) && node.children.length > 50) return false
  return true
}

/** Two hand-made rows shown before the decoded `fields` tree, xEdit-style:
 * a collapsed "Record Header" branch (flags/form_version only — `offset`/
 * `data_size` legitimately differ per file and would be false-conflict noise)
 * and a top-level EDID leaf. */
function buildSyntheticNodes(columns: RecordColumn[]): AlignedNode[] {
  const flagsValues = columns.map((c) => c.record?.header.flags ?? MISSING)
  const versionValues = columns.map((c) => c.record?.header.form_version ?? MISSING)
  const edidValues = columns.map((c) => c.record?.editor_id ?? MISSING)
  const headerSummaryValues = columns.map((c) =>
    c.record ? { flags: c.record.header.flags, form_version: c.record.header.form_version } : MISSING
  )

  const flagsNode = buildLeafNode('flags', `${HEADER_PATH}.flags`, flagsValues)
  const versionNode = buildLeafNode('form_version', `${HEADER_PATH}.form_version`, versionValues)
  const headerBranch: AlignedNode = {
    label: 'Record Header',
    path: HEADER_PATH,
    isLeaf: false,
    values: headerSummaryValues,
    children: [flagsNode, versionNode],
    conflict: flagsNode.conflict || versionNode.conflict,
    badgesPerCol: columns.map(() => []),
  }
  const edidNode = buildLeafNode('EDID', EDID_PATH, edidValues)

  return [headerBranch, edidNode]
}

function ValueCell({
  node,
  value,
  badges,
  column,
  columnCount,
  onNavigate,
}: {
  node: AlignedNode
  value: unknown
  badges: string[]
  column: RecordColumn
  columnCount: number
  onNavigate: (dbId: string, formid: string) => void
}) {
  // A conflicting row is amber-tinted as a whole (see RowNode); a MISSING cell
  // inside it gets an extra red-family tint so "absent here" reads differently
  // from "present but different" at a glance.
  const missingTint = columnCount > 1 && node.conflict && value === MISSING
  const tdStyle: React.CSSProperties = {
    padding: '2px 6px',
    borderBottom: '1px solid #222',
    borderLeft: '1px solid #222',
    verticalAlign: 'top',
    background: missingTint ? 'rgba(238,136,136,0.10)' : undefined,
  }

  if (value === MISSING) {
    return (
      <td style={tdStyle}>
        <span style={{ color: '#666' }}>—</span>
      </td>
    )
  }

  if (!node.isLeaf) {
    // Branch row: muted per-column summary, plus this column's coverage badges.
    const summary = Array.isArray(value)
      ? `[${value.length}]`
      : typeof value === 'object' && value !== null
        ? `{${Object.keys(value).length} fields}`
        : ''
    return (
      <td style={tdStyle}>
        <span style={{ color: '#aaa' }}>{summary}</span>
        {badges.length > 0 && (
          <span style={{ color: '#e8a838', marginLeft: 6 }}>
            {badges.map((b) => `[${b}]`).join(' ')}
          </span>
        )}
      </td>
    )
  }

  if (isFormIdStub(value)) {
    return (
      <td style={tdStyle}>
        <span
          tabIndex={0}
          style={{ color: '#7ec8e3', cursor: 'pointer', textDecoration: 'underline' }}
          onClick={(e) => {
            if (e.ctrlKey || e.metaKey) onNavigate(column.dbId, value.formid)
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault()
              onNavigate(column.dbId, value.formid)
            }
          }}
          title={`Ctrl+Click (or focus + Enter/Space) to navigate to ${value.formid} in ${column.fileName}`}
        >
          {value.editor_id ?? value.formid} [{value.record_type}]
        </span>
      </td>
    )
  }

  // Plain scalar, or a mixed-kind forced leaf (object/array/scalar disagreed
  // across columns) — the latter falls back to a truncated JSON summary.
  const isPlainScalar = value === null || typeof value !== 'object'
  const text = isPlainScalar ? String(value) : JSON.stringify(value)
  const truncated = text.length > 200 ? `${text.slice(0, 197)}…` : text
  return (
    <td style={tdStyle}>
      <span
        style={{
          color: isPlainScalar ? '#c3e88d' : '#aaa',
          whiteSpace: 'nowrap',
          overflow: 'hidden',
          textOverflow: 'ellipsis',
          display: 'inline-block',
          maxWidth: '100%',
        }}
        title={text}
      >
        {truncated}
      </span>
    </td>
  )
}

function RowNode({
  node,
  depth,
  columns,
  onNavigate,
  isExpanded,
  toggleNode,
}: {
  node: AlignedNode
  depth: number
  columns: RecordColumn[]
  onNavigate: (dbId: string, formid: string) => void
  isExpanded: (node: AlignedNode) => boolean
  toggleNode: (path: string) => void
}) {
  const hasChildren = !node.isLeaf && node.children.length > 0
  const expanded = hasChildren && isExpanded(node)
  const rowTint = columns.length > 1 && node.conflict ? 'rgba(232,168,56,0.10)' : undefined

  return (
    <>
      <tr style={{ background: rowTint }}>
        <td
          style={{
            padding: '2px 6px',
            paddingLeft: 8 + depth * 14,
            borderBottom: '1px solid #222',
            cursor: hasChildren ? 'pointer' : undefined,
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
          }}
          onClick={hasChildren ? () => toggleNode(node.path) : undefined}
        >
          {hasChildren && (
            <span style={{ display: 'inline-block', width: 12 }}>{expanded ? '▼' : '▶'}</span>
          )}
          <span style={{ color: '#82aaff', fontWeight: 'bold' }}>{node.label}</span>
        </td>
        {columns.map((col, i) => (
          <ValueCell
            key={col.dbId}
            node={node}
            value={node.values[i]}
            badges={node.badgesPerCol[i] ?? []}
            column={col}
            columnCount={columns.length}
            onNavigate={onNavigate}
          />
        ))}
      </tr>
      {expanded &&
        node.children.map((child) => (
          <RowNode
            key={child.path}
            node={child}
            depth={depth + 1}
            columns={columns}
            onNavigate={onNavigate}
            isExpanded={isExpanded}
            toggleNode={toggleNode}
          />
        ))}
    </>
  )
}

export function RecordTable({ columns, activeDbId, onNavigate }: Props) {
  const setActiveDb = useStore((s) => s.setActiveDb)

  // Per-path manual overrides layered on top of a global default. `globalOverride`
  // is null until Expand/Collapse all is pressed (per-node `defaultNodeExpanded`
  // rule applies); Expand/Collapse all force every node one way and clear the
  // per-path overrides, since they'd otherwise now mean the opposite of what the
  // user just clicked.
  const [globalOverride, setGlobalOverride] = useState<boolean | null>(null)
  const [toggled, setToggled] = useState<Set<string>>(new Set())

  const tree = useMemo(() => {
    const fieldsByCol = columns.map((c) => c.record?.fields ?? null)
    return [...buildSyntheticNodes(columns), ...buildAlignedTree(fieldsByCol)]
    // `columns` (recordColumns from the store) is a new array/object graph on
    // every navigation, so this is exactly "recompute once per loaded record".
  }, [columns])

  function isExpanded(node: AlignedNode): boolean {
    const base = globalOverride === null ? defaultNodeExpanded(node) : globalOverride
    return toggled.has(node.path) ? !base : base
  }

  function toggleNode(path: string) {
    setToggled((prev) => {
      const next = new Set(prev)
      if (next.has(path)) next.delete(path)
      else next.add(path)
      return next
    })
  }

  function expandAll() {
    setGlobalOverride(true)
    setToggled(new Set())
  }

  function collapseAll() {
    setGlobalOverride(false)
    setToggled(new Set())
  }

  const editorIds = columns.map((c) => c.record?.editor_id ?? '')
  const editorIdsDiffer = new Set(editorIds).size > 1

  return (
    <div style={{ display: 'flex', flexDirection: 'column', flex: 1, minHeight: 0 }}>
      <div style={{ display: 'flex', gap: 4, marginBottom: 4 }}>
        <button onClick={expandAll} style={{ fontSize: 11, padding: '2px 8px' }}>
          Expand all
        </button>
        <button onClick={collapseAll} style={{ fontSize: 11, padding: '2px 8px' }}>
          Collapse all
        </button>
      </div>
      <div style={{ overflow: 'auto', flex: 1 }}>
        <table
          style={{
            tableLayout: 'fixed',
            width: PROPERTY_COL_WIDTH + columns.length * VALUE_COL_WIDTH,
            borderCollapse: 'collapse',
            fontSize: 12,
          }}
        >
          <colgroup>
            <col style={{ width: PROPERTY_COL_WIDTH }} />
            {columns.map((col) => (
              <col key={col.dbId} style={{ width: VALUE_COL_WIDTH }} />
            ))}
          </colgroup>
          <thead style={{ position: 'sticky', top: 0, background: '#16213e', zIndex: 1 }}>
            <tr>
              <th style={{ textAlign: 'left', padding: '4px 6px', borderBottom: '1px solid #444' }}>
                Property
              </th>
              {columns.map((col) => (
                <th
                  key={col.dbId}
                  onClick={() => setActiveDb(col.dbId)}
                  title={`Click to make ${col.fileName} the active file (drives Raw mode and Referenced By)`}
                  style={{
                    textAlign: 'left',
                    padding: '4px 6px',
                    cursor: 'pointer',
                    borderBottom:
                      col.dbId === activeDbId ? '2px solid #7ec8e3' : '1px solid #444',
                  }}
                >
                  <div style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                    {col.fileName}
                  </div>
                  {editorIdsDiffer && (
                    <div style={{ fontWeight: 'normal', color: '#aaa', fontSize: 10 }}>
                      {col.record?.editor_id ?? '—'}
                    </div>
                  )}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {tree.map((node) => (
              <RowNode
                key={node.path}
                node={node}
                depth={0}
                columns={columns}
                onNavigate={onNavigate}
                isExpanded={isExpanded}
                toggleNode={toggleNode}
              />
            ))}
          </tbody>
        </table>
      </div>
    </div>
  )
}
