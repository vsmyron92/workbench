// Lazy Confluence page tree: children load when a node is expanded (one request
// returns the children and whether each has children of its own). Keyboard: arrows
// move/expand/collapse, Enter opens. Expanded nodes are remembered per browser.

import { useRef, useState, type KeyboardEvent, type MouseEvent } from 'react'
import { useQuery, useQueryClient, type QueryClient } from '@tanstack/react-query'
import {
  ArrowDown,
  ArrowUp,
  Bot,
  ChevronDown,
  ChevronRight,
  Copy,
  CopyPlus,
  Database,
  ExternalLink,
  FilePlus,
  FileText,
  Folder,
  FolderInput,
  FolderOpen,
  LayoutDashboard,
  Trash2,
} from 'lucide-react'
import { toast, toastError } from '@/shell/actions'
import { Badge, showMenu, Spinner } from '@/ui'
import { confluenceApi, qk, type TreeNode } from '../api'
import { useAtlassianUi, usePrefs } from '../state'

import { askAgentAboutPage, copyText, openConfluencePage, openExternal, pageUrl } from './actions'
import { invalidateTree, trashPage } from './PageOps'

const INDENT = 14

function TypeIcon({ node, open }: { node: TreeNode; open: boolean }) {
  if (node.type === 'folder') return open ? <FolderOpen size={14} /> : <Folder size={14} />
  if (node.type === 'whiteboard') return <LayoutDashboard size={14} />
  if (node.type === 'database') return <Database size={14} />
  return <FileText size={14} />
}

const expandable = (n: TreeNode) => (n.type === 'page' || n.type === 'folder') && n.hasChildren !== false

interface MenuOpts {
  qc?: QueryClient
  /** The sibling pages before and after it (to move it up or down). */
  prev?: TreeNode | null
  next?: TreeNode | null
}

async function reorder(qc: QueryClient, projectId: string | null, node: TreeNode, position: 'before' | 'after', target: TreeNode) {
  try {
    await confluenceApi.move(projectId, node.id, position, target.id)
    invalidateTree(qc, node.id)
  } catch (e) {
    toastError(e, `Could not move “${node.title}”`)
  }
}

export function nodeMenu(e: MouseEvent, node: TreeNode, projectId: string | null, opts: MenuOpts = {}) {
  const url = pageUrl(node.id)
  const page = node.type === 'page'
  const { qc, prev, next } = opts
  // Pages at the top of a space have no parent: Confluence refuses siblings of those.
  const nested = !!node.parentId
  showMenu(e, [
    { label: 'Open', icon: FileText, run: () => openConfluencePage(node.id, node.title), disabled: node.type !== 'page' },
    { label: 'Open in browser', icon: ExternalLink, run: () => openExternal(url), disabled: !url },
    { label: 'Copy link', icon: Copy, run: () => url && void copyText(url), disabled: !url },
    'separator',
    {
      label: 'New child page…',
      icon: FilePlus,
      run: () => useAtlassianUi.getState().openNewPage({ parentId: node.id, parentTitle: node.title, spaceId: node.spaceId ?? undefined }),
      disabled: node.type !== 'page' && node.type !== 'folder',
    },
    { label: 'Ask agent about this page…', icon: Bot, run: () => void askAgentAboutPage(projectId, node), disabled: node.type !== 'page' },
    'separator',
    {
      label: 'Move up',
      icon: ArrowUp,
      disabled: !page || !qc || !prev || prev.type !== 'page' || !nested,
      run: () => qc && prev && void reorder(qc, projectId, node, 'before', prev),
    },
    {
      label: 'Move down',
      icon: ArrowDown,
      disabled: !page || !qc || !next || next.type !== 'page' || !nested,
      run: () => qc && next && void reorder(qc, projectId, node, 'after', next),
    },
    { label: 'Move…', icon: FolderInput, disabled: !page, run: () => useAtlassianUi.getState().openPageOp({ kind: 'move', page: node, projectId }) },
    { label: 'Copy…', icon: CopyPlus, disabled: !page, run: () => useAtlassianUi.getState().openPageOp({ kind: 'copy', page: node, projectId }) },
    'separator',
    {
      label: 'Move to trash…',
      icon: Trash2,
      danger: true,
      disabled: !page || !qc || node.status !== 'current',
      run: () => (qc ? void trashPage(qc, projectId, node) : toast('info', 'Open the page to delete it')),
    },
  ])
}

interface RowProps {
  node: TreeNode
  prev: TreeNode | null
  next: TreeNode | null
  depth: number
  projectId: string | null
  archived: boolean
  selected: string | null
  onSelect: (key: string) => void
  parentKey: string | null
}

function Row({ node, prev, next, depth, projectId, archived, selected, onSelect, parentKey }: RowProps) {
  const qc = useQueryClient()
  const key = `${node.type}:${node.id}`
  const expanded = usePrefs((s) => s.expanded.includes(key))
  const toggle = usePrefs((s) => s.toggleExpanded)
  const can = expandable(node)
  const q = useQuery({
    queryKey: qk.children(projectId, node.id, node.type, archived),
    queryFn: () => confluenceApi.children(projectId, node.id, node.type, archived),
    enabled: expanded && can,
    staleTime: 5 * 60_000,
    retry: false,
  })
  const kids = q.data?.children
  const known = q.data !== undefined
  const showChevron = can && !(known && kids!.length === 0)
  const open = () => {
    if (node.type === 'page') openConfluencePage(node.id, node.title)
    else if (can) toggle(key)
  }
  return (
    <>
      <div
        className={['atl-row', selected === key && 'selected', node.status === 'archived' && 'archived'].filter(Boolean).join(' ')}
        style={{ paddingLeft: 4 + depth * INDENT }}
        data-key={key}
        data-parent={parentKey ?? ''}
        data-expandable={showChevron ? '1' : ''}
        data-expanded={expanded ? '1' : ''}
        title={node.title}
        onClick={() => {
          onSelect(key)
          open()
        }}
        onContextMenu={(e) => {
          onSelect(key)
          nodeMenu(e, node, projectId, { qc, prev, next })
        }}
      >
        <span
          className="chev"
          onClick={(e) => {
            e.stopPropagation()
            onSelect(key)
            if (showChevron) toggle(key)
          }}
        >
          {showChevron ? expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} /> : null}
        </span>
        <span className={`ico ${node.type}`}>
          <TypeIcon node={node} open={expanded} />
        </span>
        <span className="name">{node.title || 'Untitled'}</span>
        {node.status === 'archived' && <span className="hint">archived</span>}
        {expanded && q.isFetching && !known && <Spinner size={11} />}
      </div>
      {expanded && can && q.error && (
        <div className="atl-row-note wb-danger" style={{ paddingLeft: 24 + (depth + 1) * INDENT }}>
          {(q.error as Error).message}
        </div>
      )}
      {expanded &&
        kids?.map((c, i) => (
          <Row
            key={`${c.type}:${c.id}`}
            node={c}
            prev={kids[i - 1] ?? null}
            next={kids[i + 1] ?? null}
            depth={depth + 1}
            projectId={projectId}
            archived={archived}
            selected={selected}
            onSelect={onSelect}
            parentKey={key}
          />
        ))}
      {expanded && q.data?.truncated && (
        <div className="atl-row-note" style={{ paddingLeft: 24 + (depth + 1) * INDENT }}>
          <Badge tone="warning">capped</Badge> showing the first {kids?.length} items
        </div>
      )}
    </>
  )
}

export function PageTree({ roots, projectId, archived }: { roots: TreeNode[]; projectId: string | null; archived: boolean }) {
  const [selected, setSelected] = useState<string | null>(null)
  const ref = useRef<HTMLDivElement>(null)
  const toggle = usePrefs((s) => s.toggleExpanded)

  const onKey = (e: KeyboardEvent) => {
    const rows = [...(ref.current?.querySelectorAll<HTMLElement>('.atl-row') ?? [])]
    if (!rows.length) return
    const i = rows.findIndex((r) => r.dataset.key === selected)
    const cur = i >= 0 ? rows[i] : null
    const select = (el: HTMLElement | undefined) => {
      if (!el) return
      setSelected(el.dataset.key ?? null)
      el.scrollIntoView({ block: 'nearest' })
    }
    switch (e.key) {
      case 'ArrowDown':
        select(rows[Math.min(rows.length - 1, i + 1)])
        break
      case 'ArrowUp':
        select(rows[Math.max(0, i - 1)])
        break
      case 'ArrowRight':
        if (cur?.dataset.expandable && !cur.dataset.expanded) toggle(cur.dataset.key!, true)
        else if (cur?.dataset.expanded) select(rows[i + 1])
        break
      case 'ArrowLeft':
        if (cur?.dataset.expanded) toggle(cur.dataset.key!, false)
        else if (cur?.dataset.parent) select(rows.find((r) => r.dataset.key === cur.dataset.parent))
        break
      case 'Enter':
        cur?.click()
        break
      default:
        return
    }
    e.preventDefault()
  }

  return (
    <div ref={ref} className="atl-tree" tabIndex={0} onKeyDown={onKey} role="tree">
      {roots.map((n, i) => (
        <Row
          key={`${n.type}:${n.id}`}
          node={n}
          prev={roots[i - 1] ?? null}
          next={roots[i + 1] ?? null}
          depth={0}
          projectId={projectId}
          archived={archived}
          selected={selected}
          onSelect={setSelected}
          parentKey={null}
        />
      ))}
    </div>
  )
}
