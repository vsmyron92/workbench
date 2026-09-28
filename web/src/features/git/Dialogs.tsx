// Git dialogs: push (with outgoing commits), stash, new branch, reset, compare.

import { useEffect, useMemo, useState } from 'react'
import { ArrowDownToLine, ArrowUpFromLine, FileText, GitCommitHorizontal } from 'lucide-react'
import { toast, toastError } from '@/shell/actions'
import { Button, Checkbox, EmptyState, ErrorBox, Field, Input, Loading, Modal, Select, Tabs, TimeAgo } from '@/ui'
import { gitApi, useCompare, useGitStatus, usePushPreview } from './api'
import { openCommit, openDiff, runRemoteOp, updateProject } from './actions'
import { BisectStartDialog, ChangelistDialog, PickBranchDialog, ShelveDialog, UnshelveDialog } from './FlowDialogs'
import { shortSha, splitPath, statusClass } from './logic'
import { RebaseDialog } from './RebaseDialog'
import { useGitUi, type GitDialog } from './store'
import type { ChangedFile, LogCommit } from './types'

export function GitDialogs() {
  const dialog = useGitUi((s) => s.dialog)
  const close = useGitUi((s) => s.closeDialog)
  if (!dialog) return null
  switch (dialog.kind) {
    case 'push':
      return <PushDialog key={dialog.projectId} pid={dialog.projectId} onClose={close} />
    case 'stash':
      return <StashDialog pid={dialog.projectId} onClose={close} />
    case 'newBranch':
      return <NewBranchDialog d={dialog} onClose={close} />
    case 'reset':
      return <ResetDialog d={dialog} onClose={close} />
    case 'compare':
      return <CompareDialog d={dialog} onClose={close} />
    case 'log':
      return (
        <Modal title={dialog.title} onClose={close} wide footer={<Button onClick={close}>Close</Button>}>
          <pre className="git-op-log">{dialog.lines.join('\n')}</pre>
        </Modal>
      )
    case 'rebase':
      return <RebaseDialog key={`${dialog.from ?? ''}:${dialog.onto ?? ''}`} d={dialog} onClose={close} />
    case 'bisectStart':
      return <BisectStartDialog d={dialog} onClose={close} />
    case 'shelve':
      return <ShelveDialog d={dialog} onClose={close} />
    case 'unshelve':
      return <UnshelveDialog d={dialog} onClose={close} />
    case 'pickBranch':
      return <PickBranchDialog d={dialog} onClose={close} />
    case 'changelist':
      return <ChangelistDialog d={dialog} onClose={close} />
  }
}

export function CommitRow({ c, onClick }: { c: LogCommit; onClick?: () => void }) {
  return (
    <div className="git-row" onClick={onClick} title={`${c.sha}\n${c.author} <${c.email}>`}>
      <GitCommitHorizontal size={14} className="icon" />
      <span className="git-mono wb-subtle">{shortSha(c.sha)}</span>
      <span className="wb-grow wb-ellipsis">{c.subject}</span>
      <span className="wb-subtle wb-small wb-ellipsis" style={{ maxWidth: 140 }}>
        {c.author}
      </span>
      <span className="wb-subtle wb-small" style={{ width: 80, textAlign: 'right', flex: 'none' }}>
        <TimeAgo time={c.time} />
      </span>
    </div>
  )
}

export function FileRow({ f, onClick }: { f: ChangedFile; onClick?: () => void }) {
  const { name, dir } = splitPath(f.path)
  return (
    <div className="git-row" onClick={onClick} title={f.oldPath ? `${f.oldPath} → ${f.path}` : f.path}>
      <FileText size={14} className="icon" />
      <span className={`name ${statusClass(f.status)}`}>{name}</span>
      <span className="dir">{dir}</span>
      {!f.binary && (
        <span className="stat">
          <span className="wb-success">+{f.additions}</span> <span className="wb-danger">−{f.deletions}</span>
        </span>
      )}
    </div>
  )
}

// ---------------------------------------------------------------- push

function PushDialog({ pid, onClose }: { pid: string; onClose: () => void }) {
  const [remote, setRemote] = useState<string | undefined>()
  const [branch, setBranch] = useState<string | undefined>()
  const preview = usePushPreview(pid, true, remote, branch)
  const status = useGitStatus(pid)
  const behind = status.data?.behind ?? 0
  const [force, setForce] = useState(false)
  const [tags, setTags] = useState(false)
  const t = preview.data?.target
  const [branchInput, setBranchInput] = useState('')
  useEffect(() => {
    if (t && !branchInput) setBranchInput(t.remoteBranch)
  }, [t, branchInput])

  const push = () => {
    if (!t) return
    onClose()
    const target = branchInput.trim() || t.remoteBranch
    void runRemoteOp(
      pid,
      'push',
      { remote: t.remote, branch: target, forceWithLease: force, followTags: tags },
      `Push ${t.localBranch} → ${t.remote}/${target}${force ? ' (force)' : ''}`,
    )
  }
  const commits = preview.data?.commits ?? []
  return (
    <Modal
      title="Push commits"
      wide
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant={force ? 'danger' : 'primary'} icon={ArrowUpFromLine} disabled={!t} onClick={push} autoFocus>
            {force ? 'Force Push' : 'Push'}
          </Button>
        </>
      }
    >
      {preview.error && <ErrorBox error={preview.error} onRetry={() => void preview.refetch()} />}
      {preview.isLoading && <Loading />}
      {t && (
        <>
          <div className="wb-row" style={{ flexWrap: 'wrap', gap: 8 }}>
            <span className="git-mono">{t.localBranch}</span>
            <span className="wb-muted">→</span>
            {t.remotes.length > 1 ? (
              <Select
                value={t.remote}
                onChange={(e) => {
                  setRemote(e.target.value)
                  setBranch(undefined)
                  setBranchInput('')
                }}
              >
                {t.remotes.map((r) => (
                  <option key={r}>{r}</option>
                ))}
              </Select>
            ) : (
              <span className="git-mono">{t.remote}</span>
            )}
            <span className="wb-muted">/</span>
            <Input
              small
              style={{ width: 220 }}
              value={branchInput}
              onChange={(e) => setBranchInput(e.target.value)}
              onBlur={() => branchInput.trim() && branchInput.trim() !== t.remoteBranch && setBranch(branchInput.trim())}
              aria-label="Remote branch"
            />
            {!t.remoteExists && <span className="wb-badge accent">new branch</span>}
            {!t.hasUpstream && <span className="wb-small wb-muted">sets the upstream</span>}
          </div>
          {behind > 0 && !force && (
            <div className="git-banner" style={{ margin: 0 }}>
              <span className="text">
                {status.data?.upstream ?? 'The remote branch'} has {behind} commit{behind === 1 ? '' : 's'} you do not have; the push will be rejected.
              </span>
              <Button
                size="small"
                icon={ArrowDownToLine}
                onClick={() => {
                  onClose()
                  void updateProject(pid)
                }}
              >
                Update Project
              </Button>
            </div>
          )}
          <div className="git-dlg-list">
            {commits.length === 0 ? (
              <EmptyState title="Nothing to push">The remote branch already has every commit.</EmptyState>
            ) : (
              commits.map((c) => <CommitRow key={c.sha} c={c} onClick={() => (onClose(), openCommit(pid, c.sha))} />)
            )}
            {preview.data?.hasMore && <div className="git-more">…and more</div>}
          </div>
          <div className="wb-row" style={{ gap: 16 }}>
            <Checkbox checked={force} onChange={setForce}>
              Force push (with lease)
            </Checkbox>
            <Checkbox checked={tags} onChange={setTags}>
              Push tags
            </Checkbox>
          </div>
        </>
      )}
    </Modal>
  )
}

// ---------------------------------------------------------------- stash

function StashDialog({ pid, onClose }: { pid: string; onClose: () => void }) {
  const [message, setMessage] = useState('')
  const [untracked, setUntracked] = useState(false)
  const [keepIndex, setKeepIndex] = useState(false)
  const [busy, setBusy] = useState(false)
  const run = async () => {
    setBusy(true)
    try {
      await gitApi.post(pid, 'stashes', { message: message.trim() || undefined, includeUntracked: untracked, keepIndex })
      toast('success', 'Changes stashed')
      onClose()
    } catch (e) {
      toastError(e, 'Stash failed')
    } finally {
      setBusy(false)
    }
  }
  return (
    <Modal
      title="Stash changes"
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={busy} onClick={run}>
            Stash
          </Button>
        </>
      }
    >
      <Field label="Message">
        <Input autoFocus value={message} placeholder="Work in progress" onChange={(e) => setMessage(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && run()} />
      </Field>
      <Checkbox checked={untracked} onChange={setUntracked}>
        Include unversioned files
      </Checkbox>
      <Checkbox checked={keepIndex} onChange={setKeepIndex}>
        Keep staged changes in the working tree (--keep-index)
      </Checkbox>
    </Modal>
  )
}

// ---------------------------------------------------------------- new branch

const BAD_NAME = /(^[-/.])|(\.\.)|[\s~^:?*[\\]|(\/$)|(\.lock$)|(@\{)|(\.$)/

function NewBranchDialog({ d, onClose }: { d: Extract<GitDialog, { kind: 'newBranch' }>; onClose: () => void }) {
  const [name, setName] = useState('')
  const [checkout, setCheckout] = useState(true)
  const [busy, setBusy] = useState(false)
  const invalid = name.length > 0 && BAD_NAME.test(name)
  const run = async () => {
    const n = name.trim()
    if (!n || invalid) return
    setBusy(true)
    try {
      await gitApi.post(d.projectId, 'branches', { name: n, startPoint: d.startPoint, checkout })
      toast('success', checkout ? `Created and checked out ${n}` : `Created branch ${n}`)
      onClose()
    } catch (e) {
      toastError(e, 'Create branch failed')
    } finally {
      setBusy(false)
    }
  }
  return (
    <Modal
      title={d.startLabel ? `New branch from '${d.startLabel}'` : 'New branch'}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={busy} disabled={!name.trim() || invalid} onClick={run}>
            Create
          </Button>
        </>
      }
    >
      <Field label="Branch name" hint={invalid ? <span className="wb-danger">Not a valid branch name</span> : undefined}>
        <Input autoFocus value={name} placeholder="feature/my-change" onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && run()} />
      </Field>
      <Checkbox checked={checkout} onChange={setCheckout}>
        Checkout branch
      </Checkbox>
    </Modal>
  )
}

// ---------------------------------------------------------------- reset

const RESET_MODES = [
  { id: 'soft', title: 'Soft', text: 'Moves the branch; your files and staged changes stay. The undone commits become staged changes.' },
  { id: 'mixed', title: 'Mixed', text: 'Moves the branch and resets the index; your files stay. The undone commits become unstaged changes.' },
  { id: 'hard', title: 'Hard', text: 'Moves the branch and discards every staged and unstaged change. Cannot be undone for uncommitted work.' },
  { id: 'keep', title: 'Keep', text: 'Moves the branch and resets the index, keeping local changes; refuses if they would be lost.' },
] as const

function ResetDialog({ d, onClose }: { d: Extract<GitDialog, { kind: 'reset' }>; onClose: () => void }) {
  const [mode, setMode] = useState<(typeof RESET_MODES)[number]['id']>('mixed')
  const [busy, setBusy] = useState(false)
  const run = async () => {
    setBusy(true)
    try {
      await gitApi.post(d.projectId, 'reset', { ref: d.sha, mode })
      toast('success', `Reset (${mode}) to ${shortSha(d.sha)}`)
      onClose()
    } catch (e) {
      toastError(e, 'Reset failed')
    } finally {
      setBusy(false)
    }
  }
  return (
    <Modal
      title="Reset current branch"
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant={mode === 'hard' ? 'danger' : 'primary'} loading={busy} onClick={run}>
            Reset
          </Button>
        </>
      }
    >
      <div className="wb-small">
        Reset to <span className="git-mono">{shortSha(d.sha)}</span> “{d.subject}”
      </div>
      {RESET_MODES.map((m) => (
        <label key={m.id} className="git-radio">
          <input type="radio" name="reset-mode" checked={mode === m.id} onChange={() => setMode(m.id)} />
          <span>
            <b className={m.id === 'hard' ? 'wb-danger' : undefined}>{m.title}</b>
            <div className="wb-small wb-muted">{m.text}</div>
          </span>
        </label>
      ))}
    </Modal>
  )
}

// ---------------------------------------------------------------- compare

function CompareDialog({ d, onClose }: { d: Extract<GitDialog, { kind: 'compare' }>; onClose: () => void }) {
  const cmp = useCompare(d.projectId, d.base, d.head)
  const [tab, setTab] = useState<'head' | 'base' | 'files'>('head')
  const data = cmp.data
  const list = useMemo(() => (tab === 'head' ? data?.headOnly : tab === 'base' ? data?.baseOnly : null) ?? [], [data, tab])
  return (
    <Modal title={`Compare ${d.head} with ${d.base}`} wide onClose={onClose} footer={<Button onClick={onClose}>Close</Button>}>
      {cmp.error && <ErrorBox error={cmp.error} />}
      {cmp.isLoading && <Loading />}
      {data && (
        <>
          <Tabs
            value={tab}
            onChange={setTab}
            tabs={[
              { id: 'head', label: `In ${d.head}, not in ${d.base}`, badge: <span className="wb-badge">{data.headOnly.length}</span> },
              { id: 'base', label: `In ${d.base}, not in ${d.head}`, badge: <span className="wb-badge">{data.baseOnly.length}</span> },
              { id: 'files', label: 'Files', badge: <span className="wb-badge">{data.files.length}</span> },
            ]}
          />
          <div className="git-dlg-list" style={{ maxHeight: 380 }}>
            {tab !== 'files' &&
              (list.length ? (
                list.map((c) => <CommitRow key={c.sha} c={c} onClick={() => (onClose(), openCommit(d.projectId, c.sha))} />)
              ) : (
                <EmptyState title="No commits" />
              ))}
            {tab === 'files' &&
              (data.files.length ? (
                data.files.map((f) => (
                  <FileRow
                    key={f.path}
                    f={f}
                    onClick={() => (onClose(), openDiff(d.projectId, f.path, 'compare', { base: data.mergeBase ?? d.base, head: d.head, oldPath: f.oldPath }))}
                  />
                ))
              ) : (
                <EmptyState title="No file changes" />
              ))}
          </div>
          <div className="wb-small wb-muted">Files: changes on {d.head} since it forked from {d.base}.</div>
        </>
      )}
    </Modal>
  )
}
