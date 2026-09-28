// Phone tab "Docs": browse the Confluence tree or search, and read pages (read-only).
// Tapping a page opens it full screen with a back button; links stay inside the tab.

import { useState } from 'react'
import { ArrowLeft, ExternalLink, FileText, Search } from 'lucide-react'
import { Button, EmptyState, ErrorBox, IconButton, Input, Loading, Select, Spinner, TimeAgo, Toolbar } from '@/ui'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ConfluenceIcon } from '@/ui/brand'
import { confluenceApi, needsSetup, qk, refreshAtlassianStatus, useAtlassianStatusQuery, usePage, useSpaces, type TreeNode } from '../api'
import { usePrefs } from '../state'
import { sanitize } from './PageView'
import { SetupHint } from './ToolWindow'

function Children({ projectId, node, depth, onOpen }: { projectId: string | null; node: TreeNode; depth: number; onOpen: (id: string) => void }) {
  const [open, setOpen] = useState(false)
  const q = useQuery({
    queryKey: qk.children(projectId, node.id, node.type, false),
    queryFn: () => confluenceApi.children(projectId, node.id, node.type, false),
    enabled: open,
    staleTime: 5 * 60_000,
  })
  const can = node.hasChildren !== false && (node.type === 'page' || node.type === 'folder')
  return (
    <>
      <div className="atl-row" style={{ paddingLeft: 8 + depth * 16 }}>
        <span className="chev" onClick={() => can && setOpen(!open)} style={{ width: 28, height: 40, alignItems: 'center' }}>
          {can ? (open ? '▾' : '▸') : ''}
        </span>
        <span className="name" onClick={() => (node.type === 'page' ? onOpen(node.id) : setOpen(!open))}>
          {node.title}
        </span>
        {open && q.isFetching && <Spinner size={12} />}
      </div>
      {open && q.data?.children.map((c) => <Children key={c.id} projectId={projectId} node={c} depth={depth + 1} onOpen={onOpen} />)}
    </>
  )
}

function Reader({ projectId, pageId, onBack, onOpen }: { projectId: string | null; pageId: string; onBack: () => void; onOpen: (id: string) => void }) {
  const q = usePage(projectId, pageId)
  const onClick = (e: React.MouseEvent) => {
    const a = (e.target as HTMLElement).closest('a')
    if (!a) return
    e.preventDefault()
    if (a.dataset.wbPage) onOpen(a.dataset.wbPage)
    else if (a.getAttribute('href') && !a.getAttribute('href')!.startsWith('#')) window.open(a.getAttribute('href')!, '_blank', 'noopener,noreferrer')
  }
  return (
    <div className="atl-mobile">
      <Toolbar>
        <IconButton icon={ArrowLeft} label="Back" onClick={onBack} />
        <span className="title wb-ellipsis">{q.data?.title ?? 'Page'}</span>
        <span style={{ flex: 1 }} />
        {q.data && <IconButton icon={ExternalLink} label="Open in Confluence" onClick={() => window.open(q.data!.webUrl, '_blank', 'noopener,noreferrer')} />}
      </Toolbar>
      <div className="wb-scroll">
        {q.isLoading && <Loading />}
        {q.error && <ErrorBox error={q.error} onRetry={() => q.refetch()} />}
        {q.data && (
          <div className="cf-doc">
            <h1 className="cf-title">{q.data.title}</h1>
            <div className="cf-meta">
              v{q.data.version.number} · <TimeAgo time={q.data.version.createdAt} />
              {q.data.version.authorName ? ` · ${q.data.version.authorName}` : ''}
            </div>
            <div className="wb-prose wb-cf" onClick={onClick} dangerouslySetInnerHTML={{ __html: sanitize(q.data.html) }} />
          </div>
        )}
      </div>
    </div>
  )
}

export function MobileDocs({ projectId }: { projectId: string | null }) {
  const qc = useQueryClient()
  const status = useAtlassianStatusQuery(projectId)
  const spaces = useSpaces(projectId, !!status.data?.confluence)
  const prefs = usePrefs()
  const [stack, setStack] = useState<string[]>([])
  const [text, setText] = useState('')
  const [q, setQ] = useState('')
  const spaceId = prefs.spaceByProject[projectId ?? ''] ?? spaces.data?.[0]?.id ?? ''
  const space = spaces.data?.find((s) => s.id === spaceId) ?? spaces.data?.[0]
  const roots = useQuery({
    queryKey: qk.roots(projectId, space?.id ?? '', 'current'),
    queryFn: () => confluenceApi.roots(projectId, space!.id, 'current'),
    enabled: !!space,
    staleTime: 5 * 60_000,
  })
  const search = useQuery({
    // Phone search covers all spaces.
    queryKey: qk.search(projectId, q, '', false),
    queryFn: () => confluenceApi.search(projectId, q, '', false),
    enabled: q.length >= 2,
  })

  if (stack.length)
    return (
      <Reader
        key={stack[stack.length - 1]}
        projectId={projectId}
        pageId={stack[stack.length - 1]}
        onBack={() => setStack(stack.slice(0, -1))}
        onOpen={(id) => setStack([...stack, id])}
      />
    )
  const recheck = () => void refreshAtlassianStatus(qc, projectId)
  if (status.isLoading) return <Loading label="Connecting to Atlassian…" />
  if (status.error) return <SetupHint error={status.error} onRetry={recheck} />
  if (status.data && needsSetup(status.data)) return <SetupHint message={status.data.error} onRetry={recheck} />
  if (!status.data?.confluence) return <EmptyState icon={ConfluenceIcon} title="No Confluence">{status.data?.error}</EmptyState>
  const open = (id: string) => setStack([id])

  return (
    <div className="atl-mobile">
      <div className="atl-tw-bar" style={{ paddingTop: 8 }}>
        <div className="atl-search">
          <Search size={14} className="icon" />
          <Input
            value={text}
            placeholder="Search pages"
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && setQ(text.trim())}
            style={{ height: 36, fontSize: 16 }}
          />
        </div>
        {q && (
          <Button onClick={() => { setQ(''); setText('') }}>
            Clear
          </Button>
        )}
      </div>
      {!q && (
        <div className="atl-tw-bar">
          <Select value={space?.id ?? ''} onChange={(e) => prefs.setSpace(projectId, e.target.value)} style={{ height: 36, fontSize: 15 }}>
            {(spaces.data ?? []).map((s) => (
              <option key={s.id} value={s.id}>
                {s.name}
              </option>
            ))}
          </Select>
        </div>
      )}
      <div className="wb-scroll">
        {q ? (
          search.isLoading ? (
            <Loading label="Searching…" />
          ) : search.error ? (
            <ErrorBox error={search.error} />
          ) : (
            (search.data?.results ?? []).map((h) => (
              <div key={h.id} className="atl-hit" onClick={() => open(h.id)}>
                <div className="t">
                  <FileText size={14} className="wb-muted" /> {h.title}
                </div>
                <div className="x">{h.excerpt}</div>
              </div>
            ))
          )
        ) : roots.isLoading ? (
          <Loading />
        ) : (
          (roots.data?.children ?? []).map((n) => <Children key={n.id} projectId={projectId} node={n} depth={0} onOpen={open} />)
        )}
      </div>
    </div>
  )
}
