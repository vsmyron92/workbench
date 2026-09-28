// The Hierarchy tool window (right, CLion's): the call hierarchy (Ctrl+Alt+H; callers
// or callees) or the type hierarchy (Ctrl+H; supertypes or subtypes) of a symbol, as
// a tree whose levels load when expanded. A click on a caller opens the call site;
// on a callee or a type, its declaration. A branch that comes back to one of its own
// ancestors is marked recursive instead of expanding forever.

import { useEffect, useState } from 'react'
import { ChevronDown, ChevronRight, Network, Repeat, RefreshCw } from 'lucide-react'
import { EmptyState, IconButton, Spinner } from '@/ui'
import type { LspHierarchyItem, LspIncomingCall, LspOutgoingCall, LspRange } from './api'
import { lsp } from './client'
import { displayPath, shortenPath } from './convert'
import { openLocation } from './nav'
import { useHierarchy, type HierarchyDirection, type HierarchyView } from './store'
import { SymbolIcon } from './SymbolIcon'

interface Node {
  item: LspHierarchyItem
  /** Where a click goes: the call site for callers, else the declaration. */
  target: { uri: string; range: LspRange }
  /** Call sites (callers and callees). */
  calls?: number
}

const LABELS: Record<HierarchyDirection, string> = {
  incoming: 'Callers',
  outgoing: 'Callees',
  supertypes: 'Supertypes',
  subtypes: 'Subtypes',
}

const itemKey = (i: LspHierarchyItem) => `${i.uri}#${i.selectionRange.start.line}:${i.selectionRange.start.character}`

async function children(view: HierarchyView, item: LspHierarchyItem): Promise<Node[]> {
  const conn = await lsp.connect(view.projectId)
  if (!conn) throw new Error('Code intelligence is off for this project')
  await conn.whenOpen()
  switch (view.direction) {
    case 'incoming': {
      const r = await conn.request<LspIncomingCall[] | null>('callHierarchy/incomingCalls', { item })
      return (r.result ?? []).map((c) => ({ item: c.from, target: { uri: c.from.uri, range: c.fromRanges[0] ?? c.from.selectionRange }, calls: c.fromRanges.length }))
    }
    case 'outgoing': {
      const r = await conn.request<LspOutgoingCall[] | null>('callHierarchy/outgoingCalls', { item })
      return (r.result ?? []).map((c) => ({ item: c.to, target: { uri: c.to.uri, range: c.to.selectionRange }, calls: c.fromRanges.length }))
    }
    default: {
      const r = await conn.request<LspHierarchyItem[] | null>(`typeHierarchy/${view.direction}`, { item })
      return (r.result ?? []).map((i) => ({ item: i, target: { uri: i.uri, range: i.selectionRange } }))
    }
  }
}

export function HierarchyWindow({ projectId }: { projectId: string | null }) {
  const view = useHierarchy((s) => s.view)
  if (!view || view.projectId !== projectId) {
    return (
      <EmptyState icon={Network} title="No hierarchy yet">
        Put the caret on a function and press Ctrl+Alt+H for its callers, or on a type and press Ctrl+H for its subtypes.
      </EmptyState>
    )
  }
  const directions: HierarchyDirection[] = view.kind === 'call' ? ['incoming', 'outgoing'] : ['supertypes', 'subtypes']
  const root: Node = { item: view.root, target: { uri: view.root.uri, range: view.root.selectionRange } }
  return (
    <div className="wb-fill lsp-hierarchy">
      <div className="lsp-problems-bar">
        <span className="lsp-hierarchy-title">{view.kind === 'call' ? 'Call Hierarchy' : 'Type Hierarchy'}</span>
        <div className="lsp-tabs" role="tablist">
          {directions.map((d) => (
            <button key={d} role="tab" aria-selected={view.direction === d} className={view.direction === d ? 'active' : ''} onClick={() => useHierarchy.getState().setDirection(d)}>
              {LABELS[d]}
            </button>
          ))}
        </div>
        <span className="wb-grow" />
        <IconButton icon={RefreshCw} size="small" label="Refresh" onClick={() => useHierarchy.getState().setDirection(view.direction)} />
      </div>
      <div className="wb-scroll lsp-hierarchy-tree" role="tree">
        <Branch key={view.id} view={view} node={root} depth={0} ancestors={[]} open />
      </div>
    </div>
  )
}

function Branch({ view, node, depth, ancestors, open: initiallyOpen = false }: { view: HierarchyView; node: Node; depth: number; ancestors: string[]; open?: boolean }) {
  const key = itemKey(node.item)
  const recursive = ancestors.includes(key)
  const [open, setOpen] = useState(initiallyOpen && !recursive)
  const [state, setState] = useState<{ loading: boolean; nodes?: Node[]; error?: string }>({ loading: false })

  useEffect(() => {
    if (!open || state.nodes || state.loading) return
    setState({ loading: true })
    let live = true
    children(view, node.item).then(
      (nodes) => live && setState({ loading: false, nodes }),
      (e: unknown) => live && setState({ loading: false, error: e instanceof Error ? e.message : String(e) }),
    )
    return () => {
      live = false
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  const path = shortenPath(displayPath(node.target.uri))
  const line = node.target.range.start.line + 1
  const leaf = state.nodes !== undefined && state.nodes.length === 0
  return (
    <div role="treeitem" aria-expanded={recursive || leaf ? undefined : open}>
      <div
        className="wb-list-row lsp-hierarchy-row"
        style={{ paddingLeft: 6 + depth * 16 }}
        title={`${node.item.name}${node.item.detail ? ` — ${node.item.detail}` : ''}\n${path}:${line}`}
        onClick={() => openLocation(node.target.uri, node.target.range)}
      >
        <span
          className="lsp-hierarchy-toggle"
          onClick={(e) => {
            e.stopPropagation()
            if (!recursive) setOpen(!open)
          }}
        >
          {recursive || leaf ? null : open ? <ChevronDown size={13} className="wb-subtle" /> : <ChevronRight size={13} className="wb-subtle" />}
        </span>
        <SymbolIcon kind={node.item.kind} />
        <span className="lsp-hierarchy-name">{node.item.name}</span>
        {node.item.detail && <span className="lsp-diag-src wb-ellipsis">{node.item.detail}</span>}
        {recursive && (
          <span className="lsp-hierarchy-recursive" title="Recursive: this is one of its own callers">
            <Repeat size={12} />
          </span>
        )}
        <span className="wb-grow" />
        {node.calls !== undefined && node.calls > 1 && <span className="lsp-diag-src">{node.calls} calls</span>}
        <span className="lsp-diag-pos wb-ellipsis">
          {path.slice(path.lastIndexOf('/') + 1)}:{line}
        </span>
      </div>
      {open && state.loading && (
        <div className="lsp-hierarchy-note" style={{ paddingLeft: 28 + depth * 16 }}>
          <Spinner size={10} /> Loading…
        </div>
      )}
      {open && state.error && (
        <div className="lsp-hierarchy-note wb-danger" style={{ paddingLeft: 28 + depth * 16 }}>
          {state.error}
        </div>
      )}
      {open && depth === 0 && leaf && (
        <div className="lsp-hierarchy-note" style={{ paddingLeft: 28 }}>
          No {LABELS[view.direction].toLowerCase()} found.
        </div>
      )}
      {open &&
        state.nodes?.map((n, i) => <Branch key={`${itemKey(n.item)}:${i}`} view={view} node={n} depth={depth + 1} ancestors={[...ancestors, key]} />)}
    </div>
  )
}
