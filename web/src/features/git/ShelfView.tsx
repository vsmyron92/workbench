// The Commit window's Stash and Shelf tabs. Stash is git's own; the shelf is
// Workbench's (JetBrains): named patches outside the repository, unshelved with a
// 3-way merge, kept until deleted.

import { useState } from 'react'
import {
  Archive,
  ArchiveRestore,
  ChevronDown,
  ChevronRight,
  FileDiff,
  PackageOpen,
  PackagePlus,
  Pencil,
  RefreshCw,
  Trash2,
} from 'lucide-react'
import { useQueryClient } from '@tanstack/react-query'
import { confirmDialog, toast, toastError } from '@/shell/actions'
import { EmptyState, ErrorBox, IconButton, Loading, TimeAgo, Toolbar, showMenu } from '@/ui'
import { gitApi, gk, useShelf, useShelves, useStashDetails, useStashes } from './api'
import { deleteShelf, openCommit, openDiff, renameShelf, reportOutcome, shelveChanges, unshelve } from './actions'
import { FileRow } from './Dialogs'
import { shortSha, splitPath, statusClass, statusLabel } from './logic'
import { useGitUi } from './store'
import type { ShelfFile, ShelfMeta, StashEntry } from './types'

// ---------------------------------------------------------------- shelf

export function ShelfView({ pid, changed }: { pid: string; changed: string[] }) {
  const q = useShelves(pid)
  const qc = useQueryClient()
  const [open, setOpen] = useState<string | null>(null)
  const list = q.data ?? []
  const menu = (e: React.MouseEvent, s: ShelfMeta) =>
    showMenu(e, [
      { label: 'Unshelve', icon: PackageOpen, run: () => void unshelve(pid, s) },
      { label: 'Unshelve…', icon: PackageOpen, run: () => useGitUi.getState().openDialog({ kind: 'unshelve', projectId: pid, shelf: s }) },
      { label: open === s.id ? 'Hide Files' : 'Show Files', icon: FileDiff, run: () => setOpen(open === s.id ? null : s.id) },
      'separator',
      { label: 'Rename…', icon: Pencil, run: () => void renameShelf(pid, s) },
      { label: 'Delete…', icon: Trash2, danger: true, run: () => void deleteShelf(pid, s) },
    ])
  return (
    <div className="git-tabbody">
      <Toolbar>
        <IconButton
          size="small"
          icon={PackagePlus}
          label="Shelve changes…"
          disabled={!changed.length}
          onClick={() => shelveChanges(pid, changed, { name: '' })}
        />
        <span style={{ flex: 1 }} />
        <IconButton size="small" icon={RefreshCw} label="Refresh" onClick={() => void qc.invalidateQueries({ queryKey: gk.shelves(pid) })} />
      </Toolbar>
      <div className="git-changes" tabIndex={0}>
        {q.isLoading && <Loading />}
        {q.error && <ErrorBox error={q.error} onRetry={() => void q.refetch()} />}
        {q.data && !list.length && (
          <EmptyState icon={Archive} title="Nothing on the shelf">
            Shelve changes from the Changes tab (or a changelist’s menu) to set them aside without committing. They stay here until you delete them.
          </EmptyState>
        )}
        {list.map((s) => (
          <div key={s.id}>
            <div className="git-row git-shelf-row" onClick={() => setOpen(open === s.id ? null : s.id)} onContextMenu={(e) => menu(e, s)} title={`${s.name}\nShelved ${new Date(s.created).toLocaleString()}${s.branch ? ` on ${s.branch}` : ''}`}>
              {open === s.id ? <ChevronDown size={14} className="icon" /> : <ChevronRight size={14} className="icon" />}
              <span className="msg">{s.name}</span>
              <span className="meta">
                {s.files.length} file{s.files.length === 1 ? '' : 's'}
                {s.branch ? ` · ${s.branch}` : ''}
              </span>
              <span className="when">
                <TimeAgo time={s.created} />
              </span>
              <span className="actions" onClick={(e) => e.stopPropagation()}>
                <IconButton size="small" icon={PackageOpen} label="Unshelve" onClick={() => void unshelve(pid, s)} />
                <IconButton
                  size="small"
                  icon={ArchiveRestore}
                  label="Unshelve…"
                  onClick={() => useGitUi.getState().openDialog({ kind: 'unshelve', projectId: pid, shelf: s })}
                />
                <IconButton size="small" icon={Trash2} label="Delete…" onClick={() => void deleteShelf(pid, s)} />
              </span>
            </div>
            {open === s.id && <ShelfFiles pid={pid} shelf={s} />}
          </div>
        ))}
      </div>
    </div>
  )
}

function ShelfFiles({ pid, shelf }: { pid: string; shelf: ShelfMeta }) {
  const q = useShelf(pid, shelf.id)
  const commit = q.data?.viewCommit
  const show = (f: ShelfFile, preview: boolean) => commit && openDiff(pid, f.path, 'commit', { sha: commit, oldPath: f.oldPath, preview, label: 'shelved' })
  return (
    <div className="git-shelf-files">
      {q.error && <div className="git-more wb-warning">Cannot show the diff: {(q.error as Error).message}</div>}
      {shelf.files.map((f) => {
        const { name, dir } = splitPath(f.path)
        return (
          <div
            key={f.path}
            className="git-row"
            onClick={() => show(f, true)}
            onDoubleClick={() => show(f, false)}
            onContextMenu={(e) =>
              showMenu(e, [
                { label: 'Show Diff', icon: FileDiff, disabled: !commit, run: () => show(f, false) },
                { label: 'Unshelve This File', icon: PackageOpen, run: () => void unshelve(pid, shelf, { paths: [f.path] }) },
              ])
            }
            title={`${statusLabel(f.status)}: ${f.oldPath ? `${f.oldPath} → ` : ''}${f.path}${f.binary ? ' (binary)' : ''}`}
          >
            <span className={`name ${statusClass(f.status)}`}>{name}</span>
            <span className="dir">{f.oldPath ? `${dir}${dir ? ' ' : ''}← ${f.oldPath}` : dir}</span>
            {f.binary && <span className="wb-xs wb-subtle">binary</span>}
            <span className={`letter ${statusClass(f.status)}`}>{f.status}</span>
          </div>
        )
      })}
      {q.isLoading && <div className="git-more">Preparing the diff…</div>}
      {shelf.base && <div className="git-more">Shelved on top of {shortSha(shelf.base)}</div>}
    </div>
  )
}

// ---------------------------------------------------------------- stash

export function StashView({ pid }: { pid: string }) {
  const stashes = useStashes(pid)
  const qc = useQueryClient()
  const [expanded, setExpanded] = useState<string | null>(null)
  const list = stashes.data ?? []
  const act = async (s: StashEntry, what: 'apply' | 'pop' | 'drop') => {
    if (what === 'drop') {
      const ok = await confirmDialog({ title: `Drop stash “${s.message}”?`, message: 'The stashed changes are deleted.', confirmLabel: 'Drop', danger: true })
      if (!ok) return
    }
    try {
      if (what === 'drop') {
        await gitApi.post(pid, 'stash/drop', { index: s.index, sha: s.sha })
        toast('success', 'Stash dropped')
      } else {
        const o = await gitApi.outcome(pid, `stash/${what}`, { index: s.index, sha: s.sha })
        reportOutcome(pid, o)
      }
    } catch (e) {
      toastError(e, `Stash ${what} failed`)
    }
  }
  return (
    <div className="git-tabbody">
      <Toolbar>
        <IconButton size="small" icon={Archive} label="Stash changes…" onClick={() => useGitUi.getState().openDialog({ kind: 'stash', projectId: pid })} />
        <span style={{ flex: 1 }} />
        <IconButton size="small" icon={RefreshCw} label="Refresh" onClick={() => void qc.invalidateQueries({ queryKey: gk.stashes(pid) })} />
      </Toolbar>
      <div className="git-changes" tabIndex={0}>
        {stashes.isLoading && <Loading />}
        {stashes.error && <ErrorBox error={stashes.error} onRetry={() => void stashes.refetch()} />}
        {stashes.data && !list.length && (
          <EmptyState icon={Archive} title="No stashes">
            Stash changes to set them aside in git’s stash (shared with the command line).
          </EmptyState>
        )}
        {list.map((s) => {
          const k = `${s.index}:${s.sha}`
          return (
            <div key={k}>
              <div
                className="git-row git-stash-row"
                onClick={() => setExpanded(expanded === k ? null : k)}
                onContextMenu={(e) =>
                  showMenu(e, [
                    { label: 'Apply', run: () => void act(s, 'apply') },
                    { label: 'Pop', run: () => void act(s, 'pop') },
                    { label: expanded === k ? 'Hide Changes' : 'Show Changes', run: () => setExpanded(expanded === k ? null : k) },
                    { label: 'Show as Commit', run: () => openCommit(pid, s.sha) },
                    'separator',
                    { label: 'Drop…', danger: true, run: () => void act(s, 'drop') },
                  ])
                }
                title={`${s.ref}${s.branch ? ` on ${s.branch}` : ''}`}
              >
                {expanded === k ? <ChevronDown size={14} className="icon" /> : <ChevronRight size={14} className="icon" />}
                <span className="msg">{s.message}</span>
                <span className="when">
                  <TimeAgo time={s.time} />
                </span>
                <span className="actions" onClick={(e) => e.stopPropagation()}>
                  <IconButton size="small" icon={ArchiveRestore} label="Apply" onClick={() => void act(s, 'apply')} />
                  <IconButton size="small" icon={PackageOpen} label="Pop (apply and drop)" onClick={() => void act(s, 'pop')} />
                  <IconButton size="small" icon={Trash2} label="Drop…" onClick={() => void act(s, 'drop')} />
                </span>
              </div>
              {expanded === k && <StashFiles pid={pid} s={s} />}
            </div>
          )
        })}
      </div>
    </div>
  )
}

function StashFiles({ pid, s }: { pid: string; s: StashEntry }) {
  const d = useStashDetails(pid, s)
  if (d.isLoading) return <div className="git-more">Loading…</div>
  if (d.error) return <ErrorBox error={d.error} />
  if (!d.data) return null
  const { sha, untrackedSha, files, untracked } = d.data
  return (
    <div style={{ paddingLeft: 18 }}>
      {files.map((f) => (
        <FileRow key={f.path} f={f} onClick={() => openDiff(pid, f.path, 'commit', { sha, oldPath: f.oldPath })} />
      ))}
      {untrackedSha &&
        untracked.map((f) => <FileRow key={`u-${f.path}`} f={{ ...f, status: 'A' }} onClick={() => openDiff(pid, f.path, 'commit', { sha: untrackedSha })} />)}
      {!files.length && !untracked.length && <div className="git-more">No file changes</div>}
    </div>
  )
}
