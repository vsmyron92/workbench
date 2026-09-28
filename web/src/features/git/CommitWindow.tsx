// The 'commit' tool window (CLion's Commit / Changes view). Tabs: Changes (staging
// area or changelists), Stash and Shelf. The Changes tab ends in the commit box.

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import {
  AlertTriangle,
  Archive,
  ArrowDownToLine,
  ArrowUpFromLine,
  Bot,
  Check,
  ChevronDown,
  ChevronRight,
  CloudDownload,
  Copy,
  FileCode,
  FileDiff,
  FileText,
  GitCompareArrows,
  GitMerge,
  History,
  ListPlus,
  ListTree,
  Minus,
  PackagePlus,
  Plus,
  RefreshCw,
  Sparkles,
  Undo2,
} from 'lucide-react'
import { useQueryClient } from '@tanstack/react-query'
import { toast, toastError } from '@/shell/actions'
import { Button, Checkbox, EmptyState, ErrorBox, IconButton, Loading, Splitter, Tabs, TextArea, showMenu, showMenuAt, type MenuEntry } from '@/ui'
import { gitApi, gk, isNotRepo, useGitStatus, useShelves } from './api'
import {
  askCommitMessage,
  askReview,
  compareWithBranch,
  copyText,
  editChangelist,
  fetchAll,
  newBranch,
  openCommit,
  openConflict,
  openDiff,
  openFile,
  openGitLog,
  openPush,
  rollback,
  sequencer,
  shelveChanges,
  stagePaths,
  unstagePaths,
  updateProject,
} from './actions'
import { BisectBanner } from './Bisect'
import { ChangelistView, useCommitSelection } from './Changelists'
import { refsOf } from './lineSelection'
import { groupStatus, sectionCode, shortSha, splitPath, stateLabel, statusClass, statusLabel, type SectionId, type StatusSections } from './logic'
import { ShelfView, StashView } from './ShelfView'
import { useDraft, useDrafts, useGitPrefs, useGitUi, useInclusion, type CommitTab } from './store'
import type { GitStatus, GitStatusFile } from './types'

const SECTION_TITLES: Record<SectionId, string> = {
  conflicts: 'Merge Conflicts',
  staged: 'Staged',
  unstaged: 'Unstaged',
  untracked: 'Unversioned Files',
}
const ORDER: SectionId[] = ['conflicts', 'staged', 'unstaged', 'untracked']
const RENDER_CAP = 1000

const key = (s: SectionId, path: string) => `${s}\u0000${path}`
const parseKey = (k: string) => {
  const [s, path] = k.split('\u0000')
  return { section: s as SectionId, path }
}

function readNumber(k: string, dflt: number) {
  try {
    const v = Number(localStorage.getItem(k))
    return Number.isFinite(v) && v > 0 ? v : dflt
  } catch {
    return dflt
  }
}

export function CommitToolWindow({ projectId }: { projectId: string | null }) {
  if (!projectId) return <EmptyState title="No project selected" />
  return <CommitWindow key={projectId} pid={projectId} />
}

function CommitWindow({ pid }: { pid: string }) {
  const status = useGitStatus(pid)
  const st = status.data
  const tab = useGitPrefs((s) => s.tab)
  const setTab = useGitPrefs((s) => s.setTab)
  const shelves = useShelves(st ? pid : null)

  if (status.error) {
    if (isNotRepo(status.error)) {
      return <EmptyState icon={AlertTriangle} title="Not a git repository">This project is not under git version control.</EmptyState>
    }
    return <ErrorBox error={status.error} onRetry={() => void status.refetch()} />
  }
  if (!st) return <Loading />

  const total = st.files.filter((f) => f.index !== '!').length
  const count = (n: number) => (n ? <span className="git-tab-count">{n > 999 ? '999+' : n}</span> : undefined)
  const changed = st.files.filter((f) => f.index !== '!' && !f.conflict).map((f) => f.path)
  return (
    <div className="git-cw">
      <div className="git-cw-tabs">
        <Tabs<CommitTab>
          value={tab}
          onChange={setTab}
          tabs={[
            { id: 'changes', label: 'Changes', badge: count(total) },
            { id: 'stash', label: 'Stash', badge: count(st.stashes) },
            { id: 'shelf', label: 'Shelf', badge: count(shelves.data?.length ?? 0) },
          ]}
        />
      </div>
      {tab === 'changes' && <ChangesTab pid={pid} st={st} />}
      {tab === 'stash' && <StashView pid={pid} />}
      {tab === 'shelf' && <ShelfView pid={pid} changed={changed} />}
    </div>
  )
}

function ChangesTab({ pid, st }: { pid: string; st: GitStatus }) {
  const groupBy = useGitPrefs((s) => s.groupBy)
  const sections = useMemo(() => groupStatus(st.files), [st])
  const [boxHeight, setBoxHeight] = useState(() => readNumber('wb.git.commitBoxHeight', 190))
  const startHeight = useRef(boxHeight)
  useEffect(() => {
    try {
      localStorage.setItem('wb.git.commitBoxHeight', String(Math.round(boxHeight)))
    } catch {
      /* private mode */
    }
  }, [boxHeight])
  return (
    <>
      {groupBy === 'changelists' ? <ChangelistPane pid={pid} st={st} conflicts={sections.conflicts.length} /> : <StagingPane pid={pid} st={st} sections={sections} />}
      <Splitter
        direction="h"
        onResizeStart={() => (startHeight.current = boxHeight)}
        onResize={(d) => setBoxHeight(Math.max(120, Math.min(window.innerHeight * 0.7, startHeight.current - d)))}
      />
      {groupBy === 'changelists' ? (
        <ChangelistCommitBox pid={pid} st={st} conflicts={sections.conflicts.length} height={boxHeight} />
      ) : (
        <CommitBox pid={pid} st={st} staged={sections.staged.length} conflicts={sections.conflicts.length} height={boxHeight} />
      )}
    </>
  )
}

/** Refresh, update/fetch/push, then the pane's own buttons, the group-by menu and the log. */
function Tools({ pid, children }: { pid: string; children?: ReactNode }) {
  const qc = useQueryClient()
  const groupBy = useGitPrefs((s) => s.groupBy)
  const setGroupBy = useGitPrefs((s) => s.setGroupBy)
  return (
    <div className="wb-toolbar">
      <IconButton icon={RefreshCw} size="small" label="Refresh" onClick={() => void qc.invalidateQueries({ queryKey: gk.all(pid) })} />
      <span className="git-tb-sep" />
      <IconButton icon={ArrowDownToLine} size="small" label="Update Project (Ctrl+T)" onClick={() => void updateProject(pid)} />
      <IconButton icon={CloudDownload} size="small" label="Fetch" onClick={() => void fetchAll(pid)} />
      <IconButton icon={ArrowUpFromLine} size="small" label="Push… (Ctrl+Shift+K)" onClick={() => openPush(pid)} />
      <span className="git-tb-sep" />
      {children}
      <span style={{ flex: 1 }} />
      <IconButton
        icon={ListTree}
        size="small"
        label="Group by"
        onClick={(e) =>
          showMenuAt(e.currentTarget as HTMLElement, [
            { label: 'Staging Area', icon: groupBy === 'staging' ? Check : undefined, run: () => setGroupBy('staging') },
            { label: 'Changelists', icon: groupBy === 'changelists' ? Check : undefined, run: () => setGroupBy('changelists') },
            'separator',
            { label: 'New Changelist…', icon: ListPlus, run: () => editChangelist(pid) },
          ])
        }
      />
      <IconButton icon={History} size="small" label="Show Git Log" onClick={() => openGitLog(pid)} />
    </div>
  )
}

// ---------------------------------------------------------------- staging area

function StagingPane({ pid, st, sections }: { pid: string; st: GitStatus; sections: StatusSections }) {
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [anchor, setAnchor] = useState<string | null>(null)
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({})
  const listRef = useRef<HTMLDivElement>(null)

  // Flat, visible row order for keyboard navigation and shift-selection.
  const flat = useMemo(() => {
    const v: { section: SectionId; file: GitStatusFile }[] = []
    for (const s of ORDER) if (!collapsed[s]) for (const f of sections[s].slice(0, RENDER_CAP)) v.push({ section: s, file: f })
    return v
  }, [sections, collapsed])

  // Drop selections of rows that disappeared.
  useEffect(() => {
    setSelected((prev) => {
      const live = new Set(flat.map((r) => key(r.section, r.file.path)))
      const next = new Set([...prev].filter((k) => live.has(k)))
      return next.size === prev.size ? prev : next
    })
  }, [flat])

  const selection = useCallback(
    (fallback?: string) => {
      const keys = fallback && !selected.has(fallback) ? [fallback] : [...selected]
      return keys.map(parseKey)
    },
    [selected],
  )

  const openRow = (section: SectionId, f: GitStatusFile, preview: boolean) => {
    if (section === 'conflicts') openConflict(pid, f.path)
    else openDiff(pid, f.path, section === 'staged' ? 'staged' : 'working', { preview, oldPath: f.origPath })
  }

  const onRowClick = (e: React.MouseEvent, section: SectionId, f: GitStatusFile) => {
    const k = key(section, f.path)
    if (e.ctrlKey || e.metaKey) {
      setSelected((prev) => {
        const n = new Set(prev)
        if (n.has(k)) n.delete(k)
        else n.add(k)
        return n
      })
      setAnchor(k)
      return
    }
    if (e.shiftKey && anchor) {
      const a = flat.findIndex((r) => key(r.section, r.file.path) === anchor)
      const b = flat.findIndex((r) => key(r.section, r.file.path) === k)
      if (a >= 0 && b >= 0) {
        const [lo, hi] = a < b ? [a, b] : [b, a]
        setSelected(new Set(flat.slice(lo, hi + 1).map((r) => key(r.section, r.file.path))))
        return
      }
    }
    setSelected(new Set([k]))
    setAnchor(k)
    openRow(section, f, true)
  }

  /** Stage / unstage / rollback the given rows, grouped by what makes sense per section. */
  const stageRows = (rows: { section: SectionId; path: string }[]) => {
    const paths = rows.filter((r) => r.section !== 'staged').map((r) => r.path)
    if (paths.length) void stagePaths(pid, paths)
  }
  const unstageRows = (rows: { section: SectionId; path: string }[]) => {
    const paths = rows.filter((r) => r.section === 'staged').map((r) => r.path)
    if (paths.length) void unstagePaths(pid, paths)
  }
  const rollbackRows = (rows: { section: SectionId; path: string }[]) => {
    const rs = rows.filter((r) => r.section !== 'conflicts')
    const onlyUnstaged = rs.every((r) => r.section === 'unstaged' || r.section === 'untracked')
    void rollback(pid, [...new Set(rs.map((r) => r.path))], onlyUnstaged ? 'worktree' : 'all')
  }
  const shelveRows = (rows: { section: SectionId; path: string }[]) => {
    const paths = [...new Set(rows.filter((r) => r.section !== 'conflicts').map((r) => r.path))]
    if (paths.length) shelveChanges(pid, paths, { name: paths.length === 1 ? splitPath(paths[0]).name : '' })
  }
  const allChanged = () => [...new Set(st.files.filter((f) => f.index !== '!' && !f.conflict).map((f) => f.path))]

  const rowMenu = (e: React.MouseEvent, section: SectionId, f: GitStatusFile) => {
    const k = key(section, f.path)
    if (!selected.has(k)) {
      setSelected(new Set([k]))
      setAnchor(k)
    }
    const rows = selection(k)
    const single = rows.length === 1
    const items: MenuEntry[] = []
    if (section === 'conflicts') {
      items.push(
        { label: 'Resolve…', icon: GitMerge, run: () => openConflict(pid, f.path) },
        { label: 'Accept Yours', run: () => void resolveSide(pid, rows.map((r) => r.path), 'ours') },
        { label: 'Accept Theirs', run: () => void resolveSide(pid, rows.map((r) => r.path), 'theirs') },
        { label: 'Mark Resolved', icon: Check, run: () => void stagePaths(pid, rows.map((r) => r.path)) },
        'separator',
      )
    } else {
      items.push({ label: 'Show Diff', icon: FileDiff, shortcut: 'Enter', run: () => rows.forEach((r) => openRow(r.section, fileOf(st, r.path) ?? f, false)) })
    }
    items.push(
      { label: 'Open File', icon: FileCode, shortcut: 'F4', disabled: f.worktree === 'D', run: () => rows.forEach((r) => openFile(pid, r.path)) },
      'separator',
    )
    if (rows.some((r) => r.section !== 'staged' && r.section !== 'conflicts'))
      items.push({ label: 'Stage', icon: Plus, shortcut: 'Space', run: () => stageRows(rows) })
    if (rows.some((r) => r.section === 'staged')) items.push({ label: 'Unstage', icon: Minus, shortcut: 'Space', run: () => unstageRows(rows) })
    if (section !== 'conflicts') {
      items.push(
        { label: 'Shelve Changes…', icon: PackagePlus, run: () => shelveRows(rows) },
        { label: 'Rollback…', icon: Undo2, shortcut: 'Del', danger: true, run: () => rollbackRows(rows) },
      )
    }
    items.push('separator')
    if (single) {
      items.push(
        { label: 'Show History', icon: History, run: () => openGitLog(pid, { path: f.path }) },
        { label: 'Compare with Branch…', icon: GitCompareArrows, disabled: section === 'untracked', run: () => compareWithBranch(pid, f.path) },
      )
    }
    items.push(
      { label: single ? 'Copy Path' : `Copy ${rows.length} Paths`, icon: Copy, run: () => void copyText(rows.map((r) => r.path).join('\n')) },
      { label: 'Ask Agent to Review', icon: Bot, disabled: !single, run: () => void askReview(pid, f.path, section === 'staged') },
    )
    showMenu(e, items)
  }

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!flat.length) return
    const cur = flat.findIndex((r) => selected.has(key(r.section, r.file.path)))
    const at = (i: number) => {
      const r = flat[Math.max(0, Math.min(flat.length - 1, i))]
      const k = key(r.section, r.file.path)
      setSelected(new Set([k]))
      setAnchor(k)
      listRef.current?.querySelector(`[data-key="${CSS.escape(k)}"]`)?.scrollIntoView({ block: 'nearest' })
    }
    const rows = selection()
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      at(cur + 1)
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      at(cur < 0 ? 0 : cur - 1)
    } else if (e.key === 'Enter' && rows.length) {
      e.preventDefault()
      rows.forEach((r) => {
        const f = fileOf(st, r.path)
        if (f) openRow(r.section, f, false)
      })
    } else if (e.key === ' ' && rows.length) {
      e.preventDefault()
      if (rows.every((r) => r.section === 'staged')) unstageRows(rows)
      else stageRows(rows)
    } else if (e.key === 'Delete' && rows.length) {
      e.preventDefault()
      rollbackRows(rows)
    } else if (e.key === 'F4' && rows.length) {
      e.preventDefault()
      rows.forEach((r) => openFile(pid, r.path))
    }
  }

  const total = st.files.filter((f) => f.index !== '!').length
  const sectionActions = (s: SectionId): ReactNode => {
    const paths = sections[s].map((f) => f.path)
    if (s === 'staged')
      return <IconButton size="small" icon={Minus} label="Unstage all" onClick={() => void gitApi.post(pid, 'unstage', { all: true }).catch((e) => toastError(e))} />
    if (s === 'unstaged' || s === 'untracked')
      return (
        <>
          <IconButton size="small" icon={Plus} label="Stage all" onClick={() => void stagePaths(pid, paths)} />
          <IconButton size="small" icon={Undo2} label="Rollback all…" onClick={() => void rollback(pid, paths, 'worktree')} />
        </>
      )
    return <IconButton size="small" icon={GitMerge} label="Resolve first conflict" onClick={() => paths[0] && openConflict(pid, paths[0])} />
  }

  return (
    <>
      <Tools pid={pid}>
        <IconButton icon={Archive} size="small" label="Stash Changes…" onClick={() => useGitUi.getState().openDialog({ kind: 'stash', projectId: pid })} />
        <IconButton
          icon={PackagePlus}
          size="small"
          label={selected.size ? 'Shelve selected changes…' : 'Shelve all changes…'}
          disabled={!total}
          onClick={() => (selected.size ? shelveRows(selection()) : shelveChanges(pid, allChanged(), { name: '' }))}
        />
        <IconButton icon={Undo2} size="small" label="Rollback selected…" disabled={!selected.size} onClick={() => rollbackRows(selection())} />
        <IconButton
          icon={FileDiff}
          size="small"
          label="Show diff of selected"
          disabled={!selected.size}
          onClick={() => selection().forEach((r) => fileOf(st, r.path) && openRow(r.section, fileOf(st, r.path)!, false))}
        />
      </Tools>
      <StateBanner pid={pid} st={st} conflicts={sections.conflicts.length} />
      <div className="git-changes" ref={listRef} tabIndex={0} onKeyDown={onKeyDown}>
        {total === 0 && (
          <div className="wb-empty" style={{ height: 'auto', padding: '18px 12px' }}>
            <Check size={22} className="icon" />
            <div className="wb-small">No local changes{st.branch ? ` on ${st.branch}` : ''}</div>
          </div>
        )}
        {ORDER.map((s) =>
          sections[s].length === 0 ? null : (
            <div key={s}>
              <div className="git-section" onClick={() => setCollapsed((c) => ({ ...c, [s]: !c[s] }))}>
                {collapsed[s] ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
                <span className={s === 'conflicts' ? 'git-c-conflict' : undefined}>{SECTION_TITLES[s]}</span>
                <span className="count">
                  {sections[s].length} file{sections[s].length === 1 ? '' : 's'}
                </span>
                <span className="actions" onClick={(e) => e.stopPropagation()}>
                  {sectionActions(s)}
                </span>
              </div>
              {!collapsed[s] &&
                sections[s].slice(0, RENDER_CAP).map((f) => (
                  <FileStatusRow
                    key={f.path}
                    pid={pid}
                    section={s}
                    f={f}
                    selected={selected.has(key(s, f.path))}
                    onClick={(e) => onRowClick(e, s, f)}
                    onDoubleClick={() => openRow(s, f, false)}
                    onContextMenu={(e) => rowMenu(e, s, f)}
                  />
                ))}
              {!collapsed[s] && sections[s].length > RENDER_CAP && (
                <div className="git-more">{sections[s].length - RENDER_CAP} more files not shown</div>
              )}
            </div>
          ),
        )}
        {st.truncated && <div className="git-more wb-warning">Only the first 20,000 changes are listed.</div>}
      </div>
    </>
  )
}

function fileOf(st: GitStatus | undefined, path: string) {
  return st?.files.find((f) => f.path === path)
}

async function resolveSide(pid: string, paths: string[], side: 'ours' | 'theirs') {
  try {
    for (const path of paths) await gitApi.post(pid, 'conflict/resolve', { path, side })
    toast('success', `Resolved ${paths.length} file${paths.length === 1 ? '' : 's'} with ${side === 'ours' ? 'yours' : 'theirs'}`)
  } catch (e) {
    toastError(e, 'Resolve failed')
  }
}

function FileStatusRow({
  pid,
  section,
  f,
  selected,
  onClick,
  onDoubleClick,
  onContextMenu,
}: {
  pid: string
  section: SectionId
  f: GitStatusFile
  selected: boolean
  onClick: (e: React.MouseEvent) => void
  onDoubleClick: () => void
  onContextMenu: (e: React.MouseEvent) => void
}) {
  const code = sectionCode(f, section)
  const { name, dir } = splitPath(f.path)
  const renamed = section === 'staged' && f.origPath ? f.origPath : null
  const dirText = renamed ? `${dir}${dir ? ' ' : ''}← ${splitPath(renamed).dir === dir ? splitPath(renamed).name : renamed}` : dir
  return (
    <div
      className={`git-row${selected ? ' selected' : ''}`}
      data-key={key(section, f.path)}
      onClick={onClick}
      onDoubleClick={onDoubleClick}
      onContextMenu={onContextMenu}
      title={`${statusLabel(code)}: ${renamed ? `${f.origPath} → ` : ''}${f.path}`}
    >
      <FileText size={14} className="icon" />
      <span className={`name ${statusClass(code)}`}>{name}</span>
      <span className="dir">{dirText}</span>
      <span className={`letter ${statusClass(code)}`}>{code === '?' ? 'U' : code}</span>
      <span className="actions" onClick={(e) => e.stopPropagation()} onDoubleClick={(e) => e.stopPropagation()}>
        {section === 'staged' ? (
          <IconButton size="small" icon={Minus} label="Unstage" onClick={() => void unstagePaths(pid, [f.path])} />
        ) : section === 'conflicts' ? (
          <IconButton size="small" icon={GitMerge} label="Resolve…" onClick={() => openConflict(pid, f.path)} />
        ) : (
          <IconButton size="small" icon={Plus} label="Stage" onClick={() => void stagePaths(pid, [f.path])} />
        )}
        {section !== 'conflicts' && (
          <IconButton
            size="small"
            icon={Undo2}
            label="Rollback…"
            onClick={() => void rollback(pid, [f.path], section === 'staged' ? 'all' : 'worktree')}
          />
        )}
      </span>
    </div>
  )
}

export function StateBanner({ pid, st, conflicts }: { pid: string; st: GitStatus; conflicts: number }) {
  if (st.state === 'clean') {
    if (!st.branch && st.head)
      return (
        <div className="git-banner info">
          <span className="text">
            HEAD is detached at <span className="git-mono">{shortSha(st.head)}</span>
          </span>
          <Button size="small" onClick={() => newBranch(pid)}>
            New Branch…
          </Button>
        </div>
      )
    return null
  }
  if (st.state === 'bisecting') return <BisectBanner pid={pid} />
  const d = st.stateDetail
  const label = stateLabel(st.state)
  const editStop = st.state === 'rebasing' && d.edit && !conflicts
  const what =
    st.state === 'merging'
      ? `${d.branch ?? shortSha(d.onto)} into ${st.branch ?? 'HEAD'}`
      : st.state === 'rebasing'
        ? `${d.branch ?? 'HEAD'}${d.step && d.total ? ` (${d.step}/${d.total})` : ''}`
        : shortSha(d.onto)
  const canSkip = st.state === 'rebasing' || st.state === 'cherry-picking' || st.state === 'reverting'
  return (
    <div className={`git-banner${editStop ? ' info' : ''}`}>
      <AlertTriangle size={14} className={editStop ? 'wb-muted' : 'wb-warning'} />
      <span className="text">
        <b>{label}</b> {what}
        {conflicts > 0 && <span className="git-c-conflict"> · {conflicts} conflict{conflicts === 1 ? '' : 's'}</span>}
        {editStop && (
          <>
            {' '}· stopped for editing at{' '}
            {d.stopped ? (
              <a className="git-link git-mono" onClick={() => openCommit(pid, d.stopped!)}>
                {shortSha(d.stopped)}
              </a>
            ) : (
              'a commit'
            )}
            : amend it or add commits, then Continue
          </>
        )}
      </span>
      <Button size="small" variant="primary" disabled={conflicts > 0} title={conflicts ? 'Resolve all conflicts first' : undefined} onClick={() => void sequencer(pid, 'continue', label)}>
        Continue
      </Button>
      {canSkip && (
        <Button size="small" onClick={() => void sequencer(pid, 'skip', label)}>
          Skip
        </Button>
      )}
      <Button size="small" onClick={() => void sequencer(pid, 'abort', label)}>
        Abort
      </Button>
    </div>
  )
}

// ---------------------------------------------------------------- changelists

function ChangelistPane({ pid, st, conflicts }: { pid: string; st: GitStatus; conflicts: number }) {
  const [sel, setSel] = useState<string[]>([])
  const onSelection = useCallback((p: string[]) => setSel((cur) => (cur.length === p.length && cur.every((x, i) => x === p[i]) ? cur : p)), [])
  const all = st.files.filter((f) => f.index !== '!' && !f.conflict).map((f) => f.path)
  return (
    <>
      <Tools pid={pid}>
        <IconButton icon={Archive} size="small" label="Stash Changes…" onClick={() => useGitUi.getState().openDialog({ kind: 'stash', projectId: pid })} />
        <IconButton
          icon={PackagePlus}
          size="small"
          label={sel.length ? 'Shelve selected changes…' : 'Shelve all changes…'}
          disabled={!all.length}
          onClick={() => shelveChanges(pid, sel.length ? sel : all, { name: sel.length === 1 ? splitPath(sel[0]).name : '' })}
        />
        <IconButton icon={Undo2} size="small" label="Rollback selected…" disabled={!sel.length} onClick={() => void rollback(pid, sel, 'all')} />
        <IconButton icon={ListPlus} size="small" label="New changelist…" onClick={() => editChangelist(pid, undefined, sel.length ? sel : undefined)} />
      </Tools>
      <StateBanner pid={pid} st={st} conflicts={conflicts} />
      <ChangelistView pid={pid} st={st} onSelection={onSelection} />
    </>
  )
}

// ---------------------------------------------------------------- commit box

function useAmend(pid: string) {
  const draft = useDraft(pid)
  const update = useDrafts((s) => s.update)
  return async (on: boolean) => {
    update(pid, { amend: on })
    if (on && !draft.message.trim()) {
      try {
        const { message } = await gitApi.lastMessage(pid)
        update(pid, { message, amendLoaded: message })
      } catch (e) {
        toastError(e)
      }
    } else if (!on && draft.amendLoaded !== undefined && draft.message === draft.amendLoaded) {
      update(pid, { message: '', amendLoaded: undefined })
    }
  }
}

function CommitBox({ pid, st, staged, conflicts, height }: { pid: string; st: GitStatus; staged: number; conflicts: number; height: number }) {
  const draft = useDraft(pid)
  const rebasing = st.state === 'rebasing' && !st.stateDetail.edit
  // git has a message prepared during a merge / cherry-pick / revert.
  const prepared = st.state === 'merging' || st.state === 'cherry-picking' || st.state === 'reverting'
  const canCommit = !rebasing && conflicts === 0 && (draft.amend || (staged > 0 && (prepared || draft.message.trim().length > 0)))
  const reason = rebasing
    ? 'Rebasing: use Continue'
    : conflicts
      ? 'Resolve conflicts first'
      : !staged && !draft.amend
        ? 'Stage changes to commit'
        : !draft.message.trim() && !draft.amend && !prepared
          ? 'Write a commit message'
          : `${staged} file${staged === 1 ? '' : 's'} staged`
  return (
    <CommitForm
      pid={pid}
      st={st}
      height={height}
      canCommit={canCommit}
      reason={reason}
      placeholder={prepared ? 'Commit message (leave empty to use git’s prepared message)' : 'Commit message'}
      body={() => ({ message: draft.message, amend: draft.amend, signoff: draft.signoff })}
    />
  )
}

function ChangelistCommitBox({ pid, st, conflicts, height }: { pid: string; st: GitStatus; conflicts: number; height: number }) {
  const draft = useDraft(pid)
  const { included, partial, partialSel } = useCommitSelection(pid, st)
  const rebasing = st.state === 'rebasing' && !st.stateDetail.edit
  const merging = st.state === 'merging'
  const n = included.size
  const canCommit = !rebasing && !merging && conflicts === 0 && n > 0 && (draft.amend || draft.message.trim().length > 0)
  const reason = rebasing
    ? 'Rebasing: use Continue'
    : merging
      ? 'Merging: commit from the staging area view'
      : conflicts
        ? 'Resolve conflicts first'
        : !n
          ? 'Tick files to commit'
          : !draft.message.trim() && !draft.amend
            ? 'Write a commit message'
            : `${n} file${n === 1 ? '' : 's'} included${partial.size ? `, ${partial.size} partly` : ''}`
  return (
    <CommitForm
      pid={pid}
      st={st}
      height={height}
      canCommit={canCommit}
      reason={reason}
      placeholder="Commit message"
      body={() => ({
        message: draft.message,
        amend: draft.amend,
        signoff: draft.signoff,
        paths: [...included].filter((p) => !partial.has(p)),
        partial: [...partial].map((p) => ({ path: p, fingerprint: partialSel[p].fingerprint, lines: refsOf(partialSel[p].keys) })),
      })}
      onCommitted={() => useInclusion.getState().reset(pid)}
    />
  )
}

function CommitForm({
  pid,
  st,
  height,
  canCommit,
  reason,
  placeholder,
  body,
  onCommitted,
}: {
  pid: string
  st: GitStatus
  height: number
  canCommit: boolean
  reason: string
  placeholder: string
  body: () => Record<string, unknown>
  onCommitted?: () => void
}) {
  const draft = useDraft(pid)
  const update = useDrafts((s) => s.update)
  const focusTick = useDrafts((s) => s.focusTick)
  const setAmend = useAmend(pid)
  const ref = useRef<HTMLTextAreaElement>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    if (focusTick) ref.current?.focus()
  }, [focusTick])

  const commit = async (andPush: boolean) => {
    if (!canCommit || busy) return
    setBusy(true)
    try {
      const r = await gitApi.post<{ sha: string; summary: string }>(pid, 'commit', body())
      const subject = draft.message.split('\n')[0]
      toast('success', `Committed ${shortSha(r.sha)}${subject ? `: ${subject}` : ''}`, { action: { label: 'Show commit', run: () => openCommit(pid, r.sha) } })
      useDrafts.getState().reset(pid)
      onCommitted?.()
      if (andPush) openPush(pid)
    } catch (e) {
      toastError(e, 'Commit failed')
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="git-commit-box" style={{ height }}>
      <TextArea
        ref={ref}
        value={draft.message}
        placeholder={placeholder}
        spellCheck
        onChange={(e) => update(pid, { message: e.target.value })}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
            e.preventDefault()
            void commit(e.shiftKey)
          }
        }}
        aria-label="Commit message"
      />
      <div className="git-commit-opts">
        <Checkbox checked={draft.amend} onChange={(v) => void setAmend(v)} disabled={!st.head}>
          Amend
        </Checkbox>
        <Checkbox checked={draft.signoff} onChange={(v) => update(pid, { signoff: v })}>
          Sign-off
        </Checkbox>
        <span style={{ flex: 1 }} />
        <IconButton size="small" icon={Sparkles} label="Ask agent for a commit message" onClick={() => void askCommitMessage(pid)} />
      </div>
      <div className="git-commit-actions">
        <Button variant="primary" size="small" loading={busy} disabled={!canCommit} onClick={() => void commit(false)} title="Ctrl+Enter">
          {draft.amend ? 'Amend Commit' : 'Commit'}
        </Button>
        <Button size="small" disabled={!canCommit || busy} onClick={() => void commit(true)} title="Ctrl+Shift+Enter">
          Commit and Push…
        </Button>
        <span className="hint">{reason}</span>
      </div>
    </div>
  )
}
