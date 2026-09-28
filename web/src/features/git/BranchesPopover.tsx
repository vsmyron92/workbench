// CLion's branches popup: search field, quick actions, recent/local/remote
// branches (and tags when searching). A branch opens its action menu.

import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ComponentType } from 'react'
import {
  ArrowDownToLine,
  ArrowUpFromLine,
  Check,
  Cloud,
  GitBranch,
  GitBranchPlus,
  GitCommitHorizontal,
  GitCompareArrows,
  GitMerge,
  GitPullRequestArrow,
  History,
  Pencil,
  RefreshCw,
  Tag,
  Trash2,
} from 'lucide-react'
import { showToolWindow } from '@/shell/actions'
import { ErrorBox, Input, Spinner, showMenuAt, type MenuEntry } from '@/ui'
import { useBranches, useGitStatus } from './api'
import {
  checkoutBranch,
  checkoutRemote,
  checkoutRevision,
  compareWithCurrent,
  deleteBranch,
  deleteRemoteBranch,
  deleteTag,
  fetchAll,
  mergeInto,
  newBranch,
  openGitLog,
  openPush,
  openRebase,
  rebaseOnto,
  renameBranch,
  updateProject,
} from './actions'
import { matches } from './logic'
import { useDrafts, useGitUi } from './store'
import type { Branches, LocalBranch, RemoteBranch, TagInfo } from './types'

type Icon = ComponentType<{ size?: number; className?: string }>

type Item =
  | { type: 'head'; label: string; key: string }
  | { type: 'action'; key: string; label: string; icon: Icon; shortcut?: string; run: () => void }
  | { type: 'local'; key: string; b: LocalBranch }
  | { type: 'remote'; key: string; b: RemoteBranch }
  | { type: 'tag'; key: string; t: TagInfo }
  | { type: 'more'; key: string; label: string }

const SECTION_CAP = 200

export function BranchesPopover() {
  const pop = useGitUi((s) => s.popover)
  if (!pop) return null
  return <PopoverBody key={pop.projectId} projectId={pop.projectId} anchor={pop.anchor} from={pop.from} />
}

function buildItems(pid: string, br: Branches, q: string, close: () => void): Item[] {
  const items: Item[] = []
  const action = (key: string, label: string, icon: Icon, run: () => void, shortcut?: string) => {
    if (matches(label, q)) items.push({ type: 'action', key, label, icon, shortcut, run: () => (close(), run()) })
  }
  action('update', 'Update Project…', ArrowDownToLine, () => updateProject(pid), 'Ctrl+T')
  action('commit', 'Commit…', GitCommitHorizontal, () => {
    showToolWindow('commit')
    useDrafts.getState().focus()
  }, 'Alt+0')
  action('push', 'Push…', ArrowUpFromLine, () => openPush(pid), 'Ctrl+Shift+K')
  action('new', 'New Branch…', GitBranchPlus, () => newBranch(pid))
  action('rev', 'Checkout Tag or Revision…', Tag, () => void checkoutRevision(pid))
  action('fetch', 'Fetch', RefreshCw, () => void fetchAll(pid))

  const section = <T,>(label: string, list: T[], make: (x: T) => Item) => {
    if (!list.length) return
    items.push({ type: 'head', key: `h-${label}`, label: `${label}${q ? ` (${list.length})` : ''}` })
    for (const x of list.slice(0, SECTION_CAP)) items.push(make(x))
    if (list.length > SECTION_CAP) items.push({ type: 'more', key: `m-${label}`, label: `${list.length - SECTION_CAP} more — refine the search` })
  }
  const local = br.local.filter((b) => matches(b.name, q))
  local.sort((a, b) => (a.current === b.current ? a.name.localeCompare(b.name) : a.current ? -1 : 1))
  if (!q) {
    const recent = br.recent.map((n) => br.local.find((b) => b.name === n)).filter((b): b is LocalBranch => !!b)
    section('Recent', recent, (b) => ({ type: 'local', key: `r-${b.name}`, b }))
  }
  section('Local', local, (b) => ({ type: 'local', key: `l-${b.name}`, b }))
  section(
    'Remote',
    br.remote.filter((b) => matches(b.name, q)).sort((a, b) => a.name.localeCompare(b.name)),
    (b) => ({ type: 'remote', key: `o-${b.name}`, b }),
  )
  if (q) section('Tags', br.tags.filter((t) => matches(t.name, q)), (t) => ({ type: 'tag', key: `t-${t.name}`, t }))
  return items
}

function localMenu(pid: string, b: LocalBranch, current: string | null): MenuEntry[] {
  if (b.current) {
    return [
      { label: `New Branch from '${b.name}'…`, icon: GitBranchPlus, run: () => newBranch(pid, b.name, b.name) },
      ...(b.upstream ? [{ label: 'Update', icon: ArrowDownToLine, run: () => void updateProject(pid) }] : []),
      { label: 'Push…', icon: ArrowUpFromLine, run: () => openPush(pid) },
      { label: 'Show Log', icon: History, run: () => openGitLog(pid, { ref: b.name, panel: true }) },
      'separator',
      { label: 'Rename…', icon: Pencil, run: () => void renameBranch(pid, b.name) },
    ]
  }
  return [
    { label: 'Checkout', icon: Check, run: () => void checkoutBranch(pid, b.name) },
    { label: `New Branch from '${b.name}'…`, icon: GitBranchPlus, run: () => newBranch(pid, b.name, b.name) },
    { label: `Compare with '${current ?? 'HEAD'}'`, icon: GitCompareArrows, run: () => compareWithCurrent(pid, b.name, current) },
    { label: 'Show Log', icon: History, run: () => openGitLog(pid, { ref: b.name, panel: true }) },
    'separator',
    { label: `Rebase '${current ?? 'HEAD'}' onto '${b.name}'`, icon: GitPullRequestArrow, run: () => void rebaseOnto(pid, b.name, current) },
    { label: `Interactively Rebase '${current ?? 'HEAD'}' onto '${b.name}'…`, icon: GitPullRequestArrow, run: () => openRebase(pid, { onto: b.name }) },
    { label: `Merge '${b.name}' into '${current ?? 'HEAD'}'`, icon: GitMerge, run: () => void mergeInto(pid, b.name, current) },
    'separator',
    { label: 'Rename…', icon: Pencil, run: () => void renameBranch(pid, b.name) },
    { label: 'Delete', icon: Trash2, danger: true, run: () => void deleteBranch(pid, b.name) },
  ]
}

function remoteMenu(pid: string, b: RemoteBranch, br: Branches): MenuEntry[] {
  const current = br.current
  const localExists = br.local.some((l) => l.name === b.branch)
  return [
    { label: localExists ? `Checkout '${b.branch}'` : 'Checkout', icon: Check, run: () => void checkoutRemote(pid, b.name, b.branch, localExists) },
    { label: `New Branch from '${b.name}'…`, icon: GitBranchPlus, run: () => newBranch(pid, b.name, b.name) },
    { label: `Compare with '${current ?? 'HEAD'}'`, icon: GitCompareArrows, run: () => compareWithCurrent(pid, b.name, current) },
    { label: 'Show Log', icon: History, run: () => openGitLog(pid, { ref: b.name, panel: true }) },
    'separator',
    { label: `Rebase '${current ?? 'HEAD'}' onto '${b.name}'`, icon: GitPullRequestArrow, run: () => void rebaseOnto(pid, b.name, current) },
    { label: `Interactively Rebase '${current ?? 'HEAD'}' onto '${b.name}'…`, icon: GitPullRequestArrow, run: () => openRebase(pid, { onto: b.name }) },
    { label: `Merge '${b.name}' into '${current ?? 'HEAD'}'`, icon: GitMerge, run: () => void mergeInto(pid, b.name, current) },
    'separator',
    { label: 'Delete on Remote…', icon: Trash2, danger: true, run: () => void deleteRemoteBranch(pid, b.remote, b.branch) },
  ]
}

function tagMenu(pid: string, t: TagInfo, current: string | null): MenuEntry[] {
  return [
    { label: 'Checkout', icon: Check, run: () => void checkoutRevision(pid, t.name) },
    { label: `New Branch from '${t.name}'…`, icon: GitBranchPlus, run: () => newBranch(pid, t.name, t.name) },
    { label: `Compare with '${current ?? 'HEAD'}'`, icon: GitCompareArrows, run: () => compareWithCurrent(pid, t.name, current) },
    { label: `Merge '${t.name}' into '${current ?? 'HEAD'}'`, icon: GitMerge, run: () => void mergeInto(pid, t.name, current) },
    { label: 'Show Log', icon: History, run: () => openGitLog(pid, { ref: t.name, panel: true }) },
    'separator',
    { label: 'Delete Tag', icon: Trash2, danger: true, run: () => void deleteTag(pid, t.name) },
  ]
}

function PopoverBody({ projectId, anchor, from }: { projectId: string; anchor: DOMRect | null; from: 'topbar' | 'statusbar' }) {
  const close = useGitUi((s) => s.closePopover)
  const branches = useBranches(projectId)
  const status = useGitStatus(projectId)
  const [q, setQ] = useState('')
  const [active, setActive] = useState(-1)
  const ref = useRef<HTMLDivElement>(null)
  const listRef = useRef<HTMLDivElement>(null)

  const items = useMemo(() => (branches.data ? buildItems(projectId, branches.data, q, close) : []), [branches.data, q, projectId, close])
  const selectable = (i: number) => items[i] && items[i].type !== 'head' && items[i].type !== 'more'

  useEffect(() => {
    // Start on the first branch when not searching, on the first hit when searching.
    const first = items.findIndex((it, i) => selectable(i) && (q ? true : it.type !== 'action'))
    setActive(first >= 0 ? first : items.findIndex((_, i) => selectable(i)))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [q, branches.data])

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const t = e.target as HTMLElement
      // The anchor buttons toggle the popover themselves.
      if (ref.current?.contains(t) || t.closest?.('.wb-menu') || t.closest?.('[data-git-branch-anchor]')) return
      close()
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !document.querySelector('.wb-menu')) close()
    }
    window.addEventListener('mousedown', onDown, true)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('mousedown', onDown, true)
      window.removeEventListener('keydown', onKey)
    }
  }, [close])

  useLayoutEffect(() => {
    listRef.current?.querySelector('.git-pop-row.active')?.scrollIntoView({ block: 'nearest' })
  }, [active])

  const activate = (i: number, el: HTMLElement | null) => {
    const it = items[i]
    const br = branches.data
    if (!it || !br || !el) return
    const current = br.current ?? status.data?.branch ?? null
    if (it.type === 'action') it.run()
    else if (it.type === 'local') showMenuAt(el, localMenu(projectId, it.b, current).map((m) => wrap(m, close)))
    else if (it.type === 'remote') showMenuAt(el, remoteMenu(projectId, it.b, br).map((m) => wrap(m, close)))
    else if (it.type === 'tag') showMenuAt(el, tagMenu(projectId, it.t, current).map((m) => wrap(m, close)))
  }

  const move = (d: number) => {
    let i = active
    for (let n = 0; n < items.length; n++) {
      i = (i + d + items.length) % items.length
      if (selectable(i)) break
    }
    setActive(i)
  }

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      move(1)
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      move(-1)
    } else if (e.key === 'Enter' || e.key === 'ArrowRight') {
      if (e.key === 'ArrowRight' && items[active]?.type === 'action') return
      e.preventDefault()
      activate(active, listRef.current?.querySelector('.git-pop-row.active') as HTMLElement | null)
    }
  }

  const width = Math.min(400, window.innerWidth - 16)
  const left = anchor ? Math.max(8, Math.min(anchor.left, window.innerWidth - width - 8)) : (window.innerWidth - width) / 2
  const style: React.CSSProperties =
    from === 'statusbar' && anchor ? { left, bottom: window.innerHeight - anchor.top + 4 } : { left, top: anchor ? anchor.bottom + 4 : 56 }

  return (
    <div ref={ref} className="git-popover" style={style} role="dialog" aria-label="Branches">
      <div className="search">
        <Input
          small
          autoFocus
          placeholder="Search for branches and actions"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={onKeyDown}
        />
      </div>
      <div className="git-pop-list" ref={listRef}>
        {branches.isLoading && (
          <div className="git-pop-empty">
            <Spinner />
          </div>
        )}
        {branches.error && <ErrorBox error={branches.error} />}
        {branches.data && !items.length && <div className="git-pop-empty">Nothing matches “{q}”</div>}
        {items.map((it, i) => {
          if (it.type === 'head') return <div key={it.key} className="git-pop-head">{it.label}</div>
          if (it.type === 'more') return <div key={it.key} className="git-pop-empty wb-small">{it.label}</div>
          const cls = `git-pop-row${i === active ? ' active' : ''}${it.type === 'local' && it.b.current ? ' current' : ''}`
          return (
            <div
              key={it.key}
              className={cls}
              onMouseMove={() => i !== active && setActive(i)}
              onClick={(e) => activate(i, e.currentTarget)}
              title={it.type === 'local' ? it.b.subject : it.type === 'remote' ? it.b.subject : it.type === 'tag' ? it.t.subject : undefined}
            >
              {it.type === 'action' && (
                <>
                  <it.icon size={14} className="icon" />
                  <span className="label">{it.label}</span>
                  {it.shortcut && <span className="meta">{it.shortcut}</span>}
                </>
              )}
              {it.type === 'local' && (
                <>
                  {it.b.current ? <Check size={14} className="icon" /> : <GitBranch size={14} className="icon" />}
                  <span className="label">{it.b.name}</span>
                  <span className="meta">
                    {(it.b.ahead > 0 || it.b.behind > 0) && (
                      <span className="git-sync">
                        {it.b.ahead > 0 && `↑${it.b.ahead}`} {it.b.behind > 0 && `↓${it.b.behind}`}
                      </span>
                    )}
                    {it.b.gone ? <span className="wb-warning">upstream gone</span> : it.b.upstream && <span>{it.b.upstream}</span>}
                  </span>
                </>
              )}
              {it.type === 'remote' && (
                <>
                  <Cloud size={14} className="icon" />
                  <span className="label">{it.b.name}</span>
                </>
              )}
              {it.type === 'tag' && (
                <>
                  <Tag size={14} className="icon" />
                  <span className="label">{it.t.name}</span>
                  <span className="meta">{it.t.sha.slice(0, 8)}</span>
                </>
              )}
            </div>
          )
        })}
      </div>
    </div>
  )
}

/** Menu entries also close the popover. */
function wrap(m: MenuEntry, close: () => void): MenuEntry {
  if (m === 'separator') return m
  return { ...m, run: () => (close(), m.run()) }
}

