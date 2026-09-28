// Phone view: a folder-at-a-time browser and a read-only viewer (highlighted
// code through the Markdown renderer, rendered Markdown, images and media).
// No Monaco on phones.

import { useEffect, useState } from 'react'
import { ArrowLeft, ChevronRight, ExternalLink, FolderGit2, Search } from 'lucide-react'
import { useQuery } from '@tanstack/react-query'
import { useEvent } from '@/api/events'
import { Button, EmptyState, ErrorBox, formatBytes, Input, Loading, Markdown } from '@/ui'
import { filesApi } from './api'
import { useFileChanged, useVcsIndex } from './hooks'
import { FileIcon } from './icons'
import { MarkdownView } from './MarkdownView'
import { basename, dirname, fenced, hljsLanguage, isMarkdown, mediaKind } from './paths'
import { isDirEntry } from './treeModel'
import { vcsKindOf } from './vcs'

/** Files larger than this are shown as plain text (highlighting is slow on phones). */
const HIGHLIGHT_MAX = 150_000

export function MobileFiles({ projectId }: { projectId: string | null }) {
  const [dir, setDir] = useState('')
  const [file, setFile] = useState<string | null>(null)
  useEffect(() => {
    setDir('')
    setFile(null)
  }, [projectId])
  if (!projectId) return <EmptyState icon={FolderGit2} title="No project selected" />
  if (file) return <MobileViewer projectId={projectId} path={file} onBack={() => setFile(null)} />
  return <MobileBrowser projectId={projectId} dir={dir} onDir={setDir} onFile={setFile} />
}

function MobileBrowser({ projectId, dir, onDir, onFile }: { projectId: string; dir: string; onDir: (d: string) => void; onFile: (p: string) => void }) {
  const [filter, setFilter] = useState('')
  const vcs = useVcsIndex(projectId)
  const q = useQuery({ queryKey: ['files', 'mobile-list', projectId, dir], queryFn: ({ signal }) => filesApi.list(projectId, dir, signal), retry: false })
  useEvent<{ paths?: string[]; overflow?: boolean }>('fs.changed', (ev) => {
    if (ev.projectId !== projectId) return
    const hit = ev.data.overflow || (ev.data.paths ?? []).some((p) => (dirname(p) === '/' ? '' : dirname(p)) === dir || p === dir)
    if (hit) void q.refetch()
  })
  const up = dirname(dir) === '/' ? '' : dirname(dir)
  const entries = (q.data?.entries ?? []).filter((e) => !filter || e.name.toLowerCase().includes(filter.toLowerCase()))
  return (
    <div className="wb-fill">
      <div className="wb-mfiles-bar">
        {dir ? (
          <button className="wb-mfiles-back" onClick={() => onDir(up)} aria-label="Up">
            <ArrowLeft size={18} />
          </button>
        ) : (
          <FolderGit2 size={18} className="wb-muted" />
        )}
        <span className="wb-grow wb-ellipsis mono wb-small">{dir || '/'}</span>
      </div>
      <div className="wb-mfiles-filter">
        <Search size={14} className="wb-subtle" />
        <Input placeholder="Filter" value={filter} onChange={(e) => setFilter(e.target.value)} />
      </div>
      <div className="wb-scroll">
        {q.error ? (
          <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
        ) : !q.data ? (
          <Loading />
        ) : !entries.length ? (
          <EmptyState title={filter ? 'No matches' : 'Empty folder'} />
        ) : (
          entries.map((e) => {
            const isDir = isDirEntry(e)
            const kind = e.ignored ? 'ignored' : isDir ? (vcs.dirs.get(e.path) ?? null) : vcsKindOf(vcs, e.path)
            return (
              <button key={e.path} className="wb-mfiles-row" onClick={() => (isDir ? onDir(e.path) : onFile(e.path))}>
                <FileIcon path={e.path} dir={isDir} sensitive={e.sensitive} size={18} />
                <span className={`wb-grow wb-ellipsis${kind ? ` wb-vcs-${kind}` : ''}`}>{e.name}</span>
                {isDir ? <ChevronRight size={16} className="wb-subtle" /> : <span className="wb-xs wb-subtle">{formatBytes(e.size)}</span>}
              </button>
            )
          })
        )}
        {q.data?.truncated && <div className="wb-files-note">Showing {q.data.entries.length} of {q.data.total}</div>}
      </div>
    </div>
  )
}

function MobileViewer({ projectId, path, onBack }: { projectId: string; path: string; onBack: () => void }) {
  const [allow, setAllow] = useState(false)
  const media = mediaKind(path)
  const q = useQuery({
    queryKey: ['files', 'mobile-read', projectId, path, allow],
    queryFn: () => filesApi.read(projectId, path, allow),
    enabled: !media,
    retry: false,
  })
  useFileChanged(projectId, path, () => void q.refetch())
  const raw = filesApi.rawUrl(projectId, path)
  let body
  if (media === 'image') body = <div className="wb-mfiles-media"><img src={raw} alt={basename(path)} /></div>
  else if (media === 'video') body = <div className="wb-mfiles-media"><video src={raw} controls preload="metadata" /></div>
  else if (media === 'audio') body = <div className="wb-mfiles-media"><audio src={raw} controls preload="metadata" /></div>
  else if (media === 'pdf')
    body = <EmptyState title="PDF" action={<Button icon={ExternalLink} onClick={() => window.open(raw, '_blank', 'noopener')}>Open</Button>} />
  else if (q.error) body = <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
  else if (!q.data) body = <Loading />
  else if (q.data.sensitive && q.data.content === null)
    body = (
      <EmptyState title="Sensitive file" action={<Button onClick={() => setAllow(true)}>Show anyway</Button>}>
        Its content is hidden by default.
      </EmptyState>
    )
  else if (q.data.content === null)
    body = (
      <EmptyState title={q.data.tooLarge ? 'Too large to show' : 'Binary file'} action={<Button icon={ExternalLink} onClick={() => window.open(raw, '_blank', 'noopener')}>Open raw</Button>}>
        {formatBytes(q.data.size)}
      </EmptyState>
    )
  else if (isMarkdown(path)) body = <MarkdownView projectId={projectId} path={path} text={q.data.content} className="wb-mfiles-md" />
  else if (q.data.content.length > HIGHLIGHT_MAX) body = <pre className="wb-mfiles-pre">{q.data.content}</pre>
  else body = <div className="wb-mfiles-code"><Markdown text={fenced(q.data.content, hljsLanguage(path))} /></div>
  return (
    <div className="wb-fill">
      <div className="wb-mfiles-bar">
        <button className="wb-mfiles-back" onClick={onBack} aria-label="Back">
          <ArrowLeft size={18} />
        </button>
        <FileIcon path={path} size={16} />
        <span className="wb-grow wb-ellipsis">{basename(path)}</span>
      </div>
      <div className="wb-scroll">{body}</div>
    </div>
  )
}
