// Dialogs of the history and changes workflows: pick a branch, start a bisect, shelve and unshelve
// changes, create or edit a changelist.

import { useMemo, useState } from 'react'
import { Cloud, FileText, GitBranch, Tag } from 'lucide-react'
import { useQueryClient } from '@tanstack/react-query'
import { api } from '@/api/client'
import { toast, toastError } from '@/shell/actions'
import { Button, Checkbox, ErrorBox, Field, Input, Loading, Modal, Select, TextArea } from '@/ui'
import { gitApi, gitUrl, gk, useBranches, useChangelists } from './api'
import { unshelve } from './actions'
import { matches, shortSha, splitPath } from './logic'
import { useGitPrefs, type GitDialog } from './store'
import type { BisectState, ShelfMeta } from './types'

// ---------------------------------------------------------------- branch picker

export function PickBranchDialog({ d, onClose }: { d: Extract<GitDialog, { kind: 'pickBranch' }>; onClose: () => void }) {
  const br = useBranches(d.projectId)
  const [q, setQ] = useState('')
  const [active, setActive] = useState(0)
  const items = useMemo(() => {
    const b = br.data
    if (!b) return []
    const v: { name: string; kind: 'local' | 'remote' | 'tag'; current?: boolean }[] = []
    for (const l of b.local) if (matches(l.name, q)) v.push({ name: l.name, kind: 'local', current: l.current })
    for (const r of b.remote) if (matches(r.name, q)) v.push({ name: r.name, kind: 'remote' })
    if (q) for (const t of b.tags) if (matches(t.name, q)) v.push({ name: t.name, kind: 'tag' })
    return v.slice(0, 300)
  }, [br.data, q])
  const pick = (name: string) => {
    onClose()
    d.onPick(name)
  }
  return (
    <Modal
      title={d.title}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" disabled={!items[active] && !q.trim()} onClick={() => pick(items[active]?.name ?? q.trim())}>
            {d.confirmLabel ?? 'Choose'}
          </Button>
        </>
      }
    >
      <Input
        autoFocus
        placeholder="Branch, tag or revision"
        value={q}
        onChange={(e) => {
          setQ(e.target.value)
          setActive(0)
        }}
        onKeyDown={(e) => {
          if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
            e.preventDefault()
            setActive((a) => Math.max(0, Math.min(items.length - 1, a + (e.key === 'ArrowDown' ? 1 : -1))))
          } else if (e.key === 'Enter') {
            e.preventDefault()
            const name = items[active]?.name ?? q.trim()
            if (name) pick(name)
          }
        }}
      />
      {br.error && <ErrorBox error={br.error} />}
      {br.isLoading && <Loading />}
      <div className="git-dlg-list git-pick-list" role="listbox">
        {items.map((it, i) => (
          <div
            key={`${it.kind}:${it.name}`}
            role="option"
            aria-selected={i === active}
            className={`git-row${i === active ? ' selected' : ''}`}
            onMouseMove={() => i !== active && setActive(i)}
            onClick={() => pick(it.name)}
          >
            {it.kind === 'remote' ? <Cloud size={14} className="icon" /> : it.kind === 'tag' ? <Tag size={14} className="icon" /> : <GitBranch size={14} className="icon" />}
            <span className="name">{it.name}</span>
            <span className="dir">{it.current ? 'current' : it.kind === 'local' ? '' : it.kind}</span>
          </div>
        ))}
        {br.data && !items.length && <div className="git-more">Nothing matches; Enter uses “{q}” as a revision.</div>}
      </div>
    </Modal>
  )
}

// ---------------------------------------------------------------- bisect

export function BisectStartDialog({ d, onClose }: { d: Extract<GitDialog, { kind: 'bisectStart' }>; onClose: () => void }) {
  const [bad, setBad] = useState(d.bad ?? 'HEAD')
  const [good, setGood] = useState(d.good ?? '')
  const [busy, setBusy] = useState(false)
  const qc = useQueryClient()
  const run = async () => {
    if (!good.trim() || !bad.trim()) return
    setBusy(true)
    try {
      const r = await gitApi.post<{ message: string; state: BisectState }>(d.projectId, 'bisect/start', { bad: bad.trim(), good: [good.trim()] })
      qc.setQueryData(gk.bisect(d.projectId), r.state)
      toast('info', r.message || 'Bisect started')
      onClose()
    } catch (e) {
      toastError(e, 'Bisect failed to start')
    } finally {
      setBusy(false)
    }
  }
  return (
    <Modal
      title="Start bisect"
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={busy} disabled={!good.trim() || !bad.trim()} onClick={() => void run()}>
            Start Bisect
          </Button>
        </>
      }
    >
      <div className="wb-small wb-muted">
        Git checks out a commit halfway between a good and a bad one. Test it and mark it good or bad (or skip it) until the first bad commit is found.
      </div>
      <Field label="Bad commit (has the problem)">
        <Input value={bad} onChange={(e) => setBad(e.target.value)} placeholder="HEAD" />
      </Field>
      <Field label="Good commit (before the problem)">
        <Input autoFocus={!d.good} value={good} onChange={(e) => setGood(e.target.value)} placeholder="v1.2.0, a branch or a commit hash" onKeyDown={(e) => e.key === 'Enter' && void run()} />
      </Field>
    </Modal>
  )
}

// ---------------------------------------------------------------- shelve / unshelve

function FileChecks({ paths, checked, onChange }: { paths: string[]; checked: Set<string>; onChange: (s: Set<string>) => void }) {
  return (
    <div className="git-dlg-list git-check-list">
      {paths.map((p) => {
        const { name, dir } = splitPath(p)
        return (
          <label key={p} className="git-row">
            <input
              type="checkbox"
              checked={checked.has(p)}
              onChange={(e) => {
                const n = new Set(checked)
                if (e.target.checked) n.add(p)
                else n.delete(p)
                onChange(n)
              }}
            />
            <FileText size={14} className="icon" />
            <span className="name">{name}</span>
            <span className="dir">{dir}</span>
          </label>
        )
      })}
    </div>
  )
}

export function ShelveDialog({ d, onClose }: { d: Extract<GitDialog, { kind: 'shelve' }>; onClose: () => void }) {
  const [name, setName] = useState(d.name ?? '')
  const [checked, setChecked] = useState(() => new Set(d.paths))
  const [keep, setKeep] = useState(false)
  const [busy, setBusy] = useState(false)
  const run = async () => {
    if (!name.trim() || !checked.size) return
    setBusy(true)
    try {
      const m = await gitApi.post<ShelfMeta>(d.projectId, 'shelf', { name: name.trim(), paths: d.paths.filter((p) => checked.has(p)), keep })
      toast('success', `Shelved ${m.files.length} file${m.files.length === 1 ? '' : 's'} as “${m.name}”`, {
        action: { label: 'Show Shelf', run: () => useGitPrefs.getState().setTab('shelf') },
      })
      onClose()
    } catch (e) {
      toastError(e, 'Shelve failed')
    } finally {
      setBusy(false)
    }
  }
  return (
    <Modal
      title="Shelve changes"
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={busy} disabled={!name.trim() || !checked.size} onClick={() => void run()}>
            {keep ? 'Save to Shelf' : 'Shelve Changes'}
          </Button>
        </>
      }
    >
      <Field label="Name">
        <Input autoFocus value={name} placeholder="What these changes are" onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && void run()} />
      </Field>
      <FileChecks paths={d.paths} checked={checked} onChange={setChecked} />
      <Checkbox checked={keep} onChange={setKeep}>
        Keep the changes in the working tree (save a copy only)
      </Checkbox>
      {!keep && <div className="wb-xs wb-muted">The files are rolled back once they are safely on the shelf.</div>}
    </Modal>
  )
}

export function UnshelveDialog({ d, onClose }: { d: Extract<GitDialog, { kind: 'unshelve' }>; onClose: () => void }) {
  const all = d.shelf.files.map((f) => f.path)
  const [checked, setChecked] = useState(() => new Set(d.paths ?? all))
  const [remove, setRemove] = useState(false)
  const cls = useChangelists(d.projectId)
  const [target, setTarget] = useState('')
  const [busy, setBusy] = useState(false)
  const run = async () => {
    setBusy(true)
    const paths = all.filter((p) => checked.has(p))
    const r = await unshelve(d.projectId, d.shelf, { paths: paths.length === all.length ? undefined : paths, remove, changelist: target || undefined })
    setBusy(false)
    if (r) onClose()
  }
  return (
    <Modal
      title={`Unshelve “${d.shelf.name}”`}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={busy} disabled={!checked.size} onClick={() => void run()}>
            Unshelve
          </Button>
        </>
      }
    >
      <div className="wb-small wb-muted">
        Shelved {d.shelf.branch ? `on ${d.shelf.branch} ` : ''}at {shortSha(d.shelf.base)}. Changes that no longer apply cleanly are merged; conflicts open in the Commit window.
      </div>
      <FileChecks paths={all} checked={checked} onChange={setChecked} />
      {cls.data && cls.data.lists.length > 1 && (
        <Field label="Into changelist">
          <Select value={target} onChange={(e) => setTarget(e.target.value)}>
            <option value="">{d.shelf.files.some((f) => f.changelist) ? 'Where they were shelved from' : 'The active changelist'}</option>
            {cls.data.lists.map((l) => (
              <option key={l.id} value={l.id}>
                {l.name}
                {l.active ? ' (active)' : ''}
              </option>
            ))}
          </Select>
        </Field>
      )}
      <Checkbox checked={remove} onChange={setRemove}>
        Remove the unshelved files from the shelf
      </Checkbox>
    </Modal>
  )
}

// ---------------------------------------------------------------- changelist

export function ChangelistDialog({ d, onClose }: { d: Extract<GitDialog, { kind: 'changelist' }>; onClose: () => void }) {
  const editing = !!d.list
  const [name, setName] = useState(d.list?.name ?? '')
  const [comment, setComment] = useState(d.list?.comment ?? '')
  const [active, setActive] = useState(!editing)
  const [busy, setBusy] = useState(false)
  const run = async () => {
    if (!name.trim()) return
    setBusy(true)
    try {
      if (d.list) await api.patch(gitUrl(d.projectId, `changelists/${encodeURIComponent(d.list.id)}`), { name: name.trim(), comment })
      else await gitApi.post(d.projectId, 'changelists', { name: name.trim(), comment, active, paths: d.paths ?? [] })
      onClose()
    } catch (e) {
      toastError(e, editing ? 'Rename failed' : 'Create failed')
    } finally {
      setBusy(false)
    }
  }
  return (
    <Modal
      title={editing ? `Edit changelist “${d.list!.name}”` : 'New changelist'}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={busy} disabled={!name.trim()} onClick={() => void run()}>
            {editing ? 'Save' : 'Create'}
          </Button>
        </>
      }
    >
      <Field label="Name">
        <Input autoFocus value={name} placeholder="Refactoring, Bug 123…" onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && void run()} />
      </Field>
      <Field label="Comment">
        <TextArea value={comment} rows={3} placeholder="Optional notes" onChange={(e) => setComment(e.target.value)} />
      </Field>
      {!editing && (
        <Checkbox checked={active} onChange={setActive}>
          Make it the active changelist (new changes go there)
        </Checkbox>
      )}
      {!!d.paths?.length && (
        <div className="wb-xs wb-muted">
          {d.paths.length} selected file{d.paths.length === 1 ? '' : 's'} move{d.paths.length === 1 ? 's' : ''} into it.
        </div>
      )}
    </Modal>
  )
}
