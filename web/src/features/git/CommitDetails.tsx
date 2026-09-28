// Commit details: changed-files tree (→ diff, commit mode) and message/metadata.
// Used by the log's details pane and by the 'commit' panel.

import { useMemo, useState } from 'react'
import {
  Bot,
  ChevronDown,
  ChevronRight,
  Copy,
  Crosshair,
  FileCode,
  FileDiff,
  FileText,
  Folder,
  GitBranchPlus,
  GitCommitHorizontal,
  GitPullRequestArrow,
  History,
  Pencil,
  RotateCcw,
  Tag,
  Trash2,
  Undo,
  Undo2,
  Cherry,
  Ellipsis,
} from 'lucide-react'
import type { PanelProps } from '@/shell/types'
import { EmptyState, ErrorBox, IconButton, Loading, TimeAgo, Toolbar, showMenu, type MenuEntry } from '@/ui'
import { useCommitDetails } from './api'
import {
  askExplain,
  checkoutRevision,
  cherryPick,
  copyText,
  createTag,
  markBisect,
  newBranch,
  openCommit,
  openDiff,
  openFile,
  openGitLog,
  openRebase,
  revertCommit,
  startBisect,
  undoCommit,
} from './actions'
import { buildFileTree, shortSha, splitMessage, statusClass, statusLabel, type TreeNode } from './logic'
import { useGitUi } from './store'
import type { ChangedFile, CommitDetails, RefLabel } from './types'

export function RefBadge({ r }: { r: RefLabel }) {
  const cls = r.kind === 'head' || r.kind === 'HEAD' ? 'head' : r.kind === 'branch' ? 'branch' : r.kind === 'tag' ? 'tag' : 'remote'
  return (
    <span className={`git-ref ${cls}`} title={`${r.kind === 'head' ? 'current branch' : r.kind}: ${r.name}`}>
      {r.kind === 'tag' && <Tag size={10} />}
      <span className="git-ref-name">{r.name}</span>
    </span>
  )
}

function FileTree({ pid, sha, files }: { pid: string; sha: string; files: ChangedFile[] }) {
  const tree = useMemo(() => buildFileTree(files), [files])
  const [closed, setClosed] = useState<Set<string>>(new Set())
  const [sel, setSel] = useState<string | null>(null)
  const open = (f: ChangedFile, preview: boolean) => openDiff(pid, f.path, 'commit', { sha, oldPath: f.oldPath, preview })
  const menu = (e: React.MouseEvent, f: ChangedFile) => {
    setSel(f.path)
    showMenu(e, [
      { label: 'Show Diff', icon: FileDiff, run: () => open(f, false) },
      { label: 'Open File', icon: FileCode, disabled: f.status === 'D', run: () => openFile(pid, f.path) },
      { label: 'Show History', icon: History, run: () => openGitLog(pid, { path: f.path }) },
      'separator',
      { label: 'Copy Path', icon: Copy, run: () => void copyText(f.path) },
    ])
  }
  const render = (nodes: TreeNode[], depth: number): React.ReactNode =>
    nodes.map((n) => {
      const pad = { paddingLeft: 8 + depth * 14 }
      if (n.kind === 'dir') {
        const isClosed = closed.has(n.path)
        return (
          <div key={`d:${n.path}`}>
            <div
              className="git-row"
              style={pad}
              onClick={() =>
                setClosed((c) => {
                  const x = new Set(c)
                  if (x.has(n.path)) x.delete(n.path)
                  else x.add(n.path)
                  return x
                })
              }
            >
              {isClosed ? <ChevronRight size={14} className="chev" /> : <ChevronDown size={14} className="chev" />}
              <Folder size={14} className="icon" />
              <span className="name">{n.name}</span>
              <span className="dir">{n.count}</span>
            </div>
            {!isClosed && render(n.children, depth + 1)}
          </div>
        )
      }
      const f = n.file
      return (
        <div
          key={`f:${f.path}`}
          className={`git-row${sel === f.path ? ' selected' : ''}`}
          style={pad}
          onClick={() => {
            setSel(f.path)
            open(f, true)
          }}
          onDoubleClick={() => open(f, false)}
          onContextMenu={(e) => menu(e, f)}
          title={`${statusLabel(f.status)}: ${f.oldPath ? `${f.oldPath} → ` : ''}${f.path}`}
        >
          <span className="chev" />
          <FileText size={14} className="icon" />
          <span className={`name ${statusClass(f.status)}`}>{n.name}</span>
          <span className="dir">{f.oldPath ? `← ${f.oldPath}` : ''}</span>
          {!f.binary && (
            <span className="stat">
              <span className="wb-success">+{f.additions}</span> <span className="wb-danger">−{f.deletions}</span>
            </span>
          )}
        </div>
      )
    })
  return (
    <div className="git-tree" tabIndex={0}>
      {render(tree, 0)}
    </div>
  )
}

function Meta({ pid, c, onSelectSha }: { pid: string; c: CommitDetails; onSelectSha?: (sha: string) => void }) {
  const { body } = splitMessage(c.message)
  const sameCommitter = c.committer === c.author && c.committerEmail === c.email
  return (
    <>
      <div className="subject">{c.subject}</div>
      {body && <div className="body">{body}</div>}
      {c.refs.length > 0 && (
        <div className="wb-row" style={{ flexWrap: 'wrap', gap: 4 }}>
          {c.refs.map((r) => (
            <RefBadge key={`${r.kind}:${r.name}`} r={r} />
          ))}
        </div>
      )}
      <div className="kv">
        <span>Commit</span>
        <span>
          <a className="git-link" title="Copy" onClick={() => void copyText(c.sha)}>
            {c.sha}
          </a>
        </span>
        <span>Author</span>
        <span>
          {c.author} &lt;{c.email}&gt; · <TimeAgo time={c.time} /> · {new Date(c.time).toLocaleString()}
        </span>
        {!sameCommitter && (
          <>
            <span>Committer</span>
            <span>
              {c.committer} &lt;{c.committerEmail}&gt; · <TimeAgo time={c.commitTime} />
            </span>
          </>
        )}
        {c.parents.length > 0 && (
          <>
            <span>{c.parents.length > 1 ? 'Parents' : 'Parent'}</span>
            <span className="wb-row" style={{ gap: 8 }}>
              {c.parents.map((p) => (
                <a key={p} className="git-link" onClick={() => (onSelectSha ? onSelectSha(p) : openCommit(pid, p))}>
                  {shortSha(p)}
                </a>
              ))}
            </span>
          </>
        )}
      </div>
    </>
  )
}

export function CommitDetailsView({
  pid,
  sha,
  layout = 'stacked',
  onSelectSha,
}: {
  pid: string
  sha: string | null
  layout?: 'stacked' | 'split'
  onSelectSha?: (sha: string) => void
}) {
  const q = useCommitDetails(pid, sha)
  if (!sha) return <EmptyState icon={GitCommitHorizontal} title="Select a commit" />
  if (q.isLoading) return <Loading />
  if (q.error) return <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
  const c = q.data
  if (!c) return null
  const adds = c.files.reduce((n, f) => n + f.additions, 0)
  const dels = c.files.reduce((n, f) => n + f.deletions, 0)
  return (
    <div className={`git-details ${layout}`}>
      <div className="files">
        <div className="git-log-header">
          <span>
            {c.files.length}
            {c.filesTruncated ? '+' : ''} file{c.files.length === 1 ? '' : 's'} changed
          </span>
          <span className="wb-success">+{adds}</span>
          <span className="wb-danger">−{dels}</span>
        </div>
        {c.files.length ? <FileTree pid={pid} sha={c.sha} files={c.files} /> : <EmptyState title="No file changes" />}
      </div>
      <div className="meta">
        <Meta pid={pid} c={c} onSelectSha={onSelectSha} />
      </div>
    </div>
  )
}

/** What the log knows around a commit (for the history-rewriting and bisect entries). */
export interface CommitMenuContext {
  headSha?: string | null
  bisecting?: boolean
  branch?: string | null
}

/** Context menu of a commit (log rows, commit panel toolbar). */
export function commitMenu(
  pid: string,
  c: { sha: string; subject: string; refs: RefLabel[] },
  onCheckoutRef?: (r: RefLabel) => void,
  ctx: CommitMenuContext = {},
): MenuEntry[] {
  const branchRefs = c.refs.filter((r) => r.kind === 'branch' || r.kind === 'remote')
  const isHead = !!ctx.headSha && ctx.headSha === c.sha
  const history: MenuEntry[] = [
    { label: 'Edit Commit Message…', icon: Pencil, run: () => openRebase(pid, { from: c.sha, focus: c.sha, preset: 'reword' }) },
    { label: 'Drop Commit…', icon: Trash2, run: () => openRebase(pid, { from: c.sha, focus: c.sha, preset: 'drop' }) },
    { label: 'Interactively Rebase from Here…', icon: GitPullRequestArrow, run: () => openRebase(pid, { from: c.sha, focus: c.sha }) },
  ]
  if (isHead) history.unshift({ label: 'Undo Commit…', icon: Undo, run: () => void undoCommit(pid, c.sha, c.subject) })
  const bisect: MenuEntry[] = ctx.bisecting
    ? [
        { label: 'Bisect: Mark as Good', icon: Crosshair, run: () => void markBisect(pid, 'good', c.sha) },
        { label: 'Bisect: Mark as Bad', icon: Crosshair, run: () => void markBisect(pid, 'bad', c.sha) },
        { label: 'Bisect: Skip', icon: Crosshair, run: () => void markBisect(pid, 'skip', c.sha) },
      ]
    : [
        { label: 'Start Bisect: This Is Bad…', icon: Crosshair, run: () => startBisect(pid, { bad: isHead ? 'HEAD' : c.sha }) },
        { label: 'Start Bisect: This Is Good…', icon: Crosshair, run: () => startBisect(pid, { good: c.sha }) },
      ]
  return [
    { label: 'Copy Revision Number', icon: Copy, run: () => void copyText(c.sha) },
    { label: 'Open in Editor Area', icon: GitCommitHorizontal, run: () => openCommit(pid, c.sha) },
    'separator',
    ...branchRefs.slice(0, 4).map<MenuEntry>((r) => ({ label: `Checkout '${r.name}'`, run: () => onCheckoutRef?.(r) })),
    { label: 'Checkout Revision…', run: () => void checkoutRevision(pid, c.sha) },
    { label: 'New Branch…', icon: GitBranchPlus, run: () => newBranch(pid, c.sha, shortSha(c.sha)) },
    { label: 'New Tag…', icon: Tag, run: () => void createTag(pid, c.sha) },
    'separator',
    { label: 'Cherry-Pick', icon: Cherry, run: () => void cherryPick(pid, c.sha, c.subject) },
    { label: 'Revert Commit', icon: Undo2, run: () => void revertCommit(pid, c.sha, c.subject) },
    {
      label: 'Reset Current Branch to Here…',
      icon: RotateCcw,
      danger: true,
      run: () => useGitUi.getState().openDialog({ kind: 'reset', projectId: pid, sha: c.sha, subject: c.subject }),
    },
    'separator',
    ...history,
    'separator',
    ...bisect,
    'separator',
    { label: 'Ask Agent to Explain This Commit', icon: Bot, run: () => void askExplain(pid, c.sha, c.subject) },
  ]
}

/** The 'commit' panel: one commit, files on the left, message on the right. */
export function CommitPanel({ params }: PanelProps<{ projectId: string; sha: string }>) {
  const q = useCommitDetails(params?.projectId ?? '', params?.sha ?? null)
  if (!params?.projectId || !params.sha) return <EmptyState title="No commit" />
  const pid = params.projectId
  const c = q.data
  return (
    <div className="git-commit-panel">
      <Toolbar>
        <GitCommitHorizontal size={15} className="wb-muted" />
        <span className="git-mono">{shortSha(params.sha)}</span>
        <span className="wb-ellipsis wb-grow wb-small">{c?.subject}</span>
        {c && (
          <>
            <IconButton size="small" icon={Copy} label="Copy revision number" onClick={() => void copyText(c.sha)} />
            <IconButton size="small" icon={Cherry} label="Cherry-pick" onClick={() => void cherryPick(pid, c.sha, c.subject)} />
            <IconButton size="small" icon={Undo2} label="Revert commit" onClick={() => void revertCommit(pid, c.sha, c.subject)} />
            <IconButton size="small" icon={GitBranchPlus} label="New branch here" onClick={() => newBranch(pid, c.sha, shortSha(c.sha))} />
            <IconButton size="small" icon={Bot} label="Ask agent to explain" onClick={() => void askExplain(pid, c.sha, c.subject)} />
            <IconButton
              size="small"
              icon={Ellipsis}
              label="More…"
              onClick={(e) => {
                const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
                showMenu({ clientX: r.left, clientY: r.bottom }, commitMenu(pid, c))
              }}
            />
          </>
        )}
      </Toolbar>
      <div className="wb-grow" style={{ minHeight: 0 }}>
        <CommitDetailsView pid={pid} sha={params.sha} layout="split" />
      </div>
    </div>
  )
}
