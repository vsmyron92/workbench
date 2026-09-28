// Review Changes: what one agent session changed, from Local History's agent
// attribution (Claude Code sessions; Codex, Kimi and other CLIs are not
// attributed). Per file: the version before the session's first edit against the
// file now (the editor buffer, editable), with Revert; and Revert All.

import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Bot, Clock, ExternalLink, FileMinus, FilePen, FilePlus, RefreshCw, RotateCcw, Save, Trash2, TriangleAlert, Undo2 } from 'lucide-react'
import { useInvalidateOn } from '@/api/events'
import { confirmDialog, toast, toastError } from '@/shell/actions'
import type { PanelProps } from '@/shell/types'
import { Button, EmptyState, ErrorBox, IconButton, Loading } from '@/ui'
import { filesApi } from '../api'
import { applyText, bufferKey, useBuffers } from '../buffers'
import { FileIcon } from '../icons'
import { openFile } from '../openers'
import { basename, dirname } from '../paths'
import { historyApi, hk, type SessionFile } from './api'
import { ConflictBanner, HistoryDiff, saveFromHistory, useFileBuffer, useRevisionText } from './LocalHistoryPanel'
import { clockTime, shortWhen } from './model'
import { showLocalHistory } from './open'

export interface AgentChangesParams {
  projectId: string
  terminalId: string
  /** The session's title when the panel was opened. */
  title?: string
}

export function agentChangesPanelId(terminalId: string) {
  return `agentChanges:${terminalId}`
}

type Status = 'modified' | 'created' | 'deleted'
const statusOf = (f: SessionFile): Status => (f.deleted ? 'deleted' : f.before === null ? 'created' : 'modified')
const STATUS_ICON = { modified: FilePen, created: FilePlus, deleted: FileMinus }

export function AgentChangesPanel({ params, setTitle }: PanelProps<AgentChangesParams>) {
  const { projectId, terminalId } = params
  const qc = useQueryClient()
  const q = useQuery({
    queryKey: hk.session(projectId, terminalId),
    queryFn: ({ signal }) => historyApi.session(projectId, terminalId, signal),
  })
  useInvalidateOn(qc, ['files.history'], (ev) => (ev.projectId === projectId ? hk.session(projectId, terminalId) : null))
  const who = q.data?.who ?? params.title ?? 'agent session'
  useEffect(() => setTitle(`Changes · ${who}`), [who, setTitle])
  const files = useMemo(() => q.data?.files ?? [], [q.data])
  const [selected, setSelected] = useState<string | null>(null)
  const cur = files.find((f) => f.path === selected) ?? files[0] ?? null

  if (q.error) return <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
  if (!q.data) return <Loading label="Collecting the session's changes…" />
  if (!files.length) {
    return (
      <EmptyState icon={Bot} title="No changes recorded for this session">
        Local History attributes the files a Claude Code session writes with its Edit and Write tools. Changes made through shell commands, and by other agent CLIs, show as
        "Changed on disk" in Local History instead.
      </EmptyState>
    )
  }

  const revertAll = async () => {
    const ok = await confirmDialog({
      title: `Revert everything ${who} changed?`,
      message: `${files.length} file${files.length === 1 ? '' : 's'}: modified files go back to their version before the session, files it created are moved to the trash, files it deleted come back. Local History keeps the current versions.`,
      confirmLabel: 'Revert All',
      danger: true,
    })
    if (!ok) return
    const failed: string[] = []
    for (const f of files) {
      try {
        await revertOnDisk(projectId, f)
      } catch (e) {
        failed.push(`${f.path}: ${e instanceof Error ? e.message : String(e)}`)
      }
    }
    void q.refetch()
    if (failed.length) toast('warning', `Reverted ${files.length - failed.length} of ${files.length} files`, { detail: failed.join('\n') })
    else toast('success', `Reverted ${files.length} file${files.length === 1 ? '' : 's'}`)
  }

  const counts = files.reduce<Record<Status, number>>((c, f) => ({ ...c, [statusOf(f)]: c[statusOf(f)] + 1 }), { modified: 0, created: 0, deleted: 0 })
  return (
    <div className="wb-fill wb-ac">
      <div className="wb-ac-bar">
        <Bot size={14} className="wb-subtle" />
        <span className="wb-ac-title wb-ellipsis">{who}</span>
        <span className="wb-subtle wb-small">
          {[counts.modified && `${counts.modified} modified`, counts.created && `${counts.created} created`, counts.deleted && `${counts.deleted} deleted`].filter(Boolean).join(' · ')}
        </span>
        <span className="wb-grow" />
        <IconButton icon={RefreshCw} size="small" label="Refresh" onClick={() => void q.refetch()} />
        <Button size="small" icon={RotateCcw} onClick={() => void revertAll()}>
          Revert All…
        </Button>
      </div>
      <div className="wb-ac-body">
        <div className="wb-scroll wb-ac-list" role="listbox" aria-label="Changed files">
          {files.map((f) => {
            const st = statusOf(f)
            const Icon = STATUS_ICON[st]
            return (
              <div
                key={f.path}
                role="option"
                aria-selected={f === cur}
                className={`wb-list-row wb-ac-row${f === cur ? ' selected' : ''}`}
                onClick={() => setSelected(f.path)}
                title={`${f.path}\n${f.edits} edit${f.edits === 1 ? '' : 's'} by the session, last ${clockTime(f.last.ts)}${f.changedSince ? '\nChanged by someone else since' : ''}`}
              >
                <FileIcon path={f.path} />
                <span className={`wb-ac-name wb-ellipsis ${st}`}>{basename(f.path)}</span>
                <span className="wb-subtle wb-small wb-ellipsis">{dirname(f.path) === '/' ? '' : dirname(f.path)}</span>
                <span className="wb-grow" />
                {f.changedSince && <TriangleAlert size={12} className="wb-ac-since" />}
                <Icon size={13} className={`wb-ac-st ${st}`} />
              </div>
            )
          })}
        </div>
        <div className="wb-ac-detail">{cur && <FileChange key={cur.path} projectId={projectId} file={cur} who={who} />}</div>
      </div>
    </div>
  )
}

/** Put one file back as it was before the session (Revert All). */
async function revertOnDisk(projectId: string, f: SessionFile) {
  const st = statusOf(f)
  if (st === 'created') {
    await filesApi.op(projectId, 'delete', f.path)
    return
  }
  const r = await historyApi.revision(projectId, f.before!.id)
  if (r.content === null) throw new Error('its earlier version is gone from Local History')
  // A deleted file comes back; a modified one is replaced only if it is still the
  // version Local History last saw (else someone changed it: a conflict to look at).
  await filesApi.write(projectId, f.path, r.content, st === 'deleted' ? null : (f.latest.hash ?? null))
}

function FileChange({ projectId, file, who }: { projectId: string; file: SessionFile; who: string }) {
  const st = statusOf(file)
  const before = useRevisionText(projectId, file.before?.id ?? null)
  const buf = useFileBuffer(projectId, file.path, st !== 'deleted')
  const key = bufferKey(projectId, file.path)
  const dirty = useBuffers((s) => s.buffers[key]?.dirty ?? false)
  const conflict = useBuffers((s) => s.buffers[key]?.conflict ?? null)
  const beforeText = file.before ? (before.data?.content ?? null) : ''
  const beforeLabel = file.before ? `Before ${who} · ${shortWhen(file.before.ts)}` : 'Did not exist'
  const since = file.changedSince ? ' · changed by someone else since' : ''

  if (before.error) return <ErrorBox error={before.error} onRetry={() => void before.refetch()} />
  if (beforeText === null) return <Loading label="Loading the earlier version…" />

  const actions = (extra: ReactNode) => (
    <div className="wb-lh-actions">
      {extra}
      <Button size="small" icon={Clock} onClick={() => showLocalHistory(projectId, file.path, false, file.last.id)}>
        Show File History
      </Button>
      <span className="wb-subtle wb-small wb-ellipsis">
        {file.edits} edit{file.edits === 1 ? '' : 's'} by the session, the last at {clockTime(file.last.ts)}
      </span>
    </div>
  )

  if (st === 'deleted') {
    const restore = async () => {
      try {
        await filesApi.write(projectId, file.path, beforeText, null)
        toast('success', `Restored ${basename(file.path)}`)
      } catch (e) {
        toastError(e, `Could not restore ${basename(file.path)}`)
      }
    }
    return (
      <div className="wb-fill">
        <div className="wb-lh-head">
          <span className="side wb-ellipsis">{beforeLabel}</span>
          <span className="side wb-ellipsis wb-vcs-deleted">Deleted{since}</span>
        </div>
        <HistoryDiff path={file.path} original={beforeText} modified="" />
        {actions(
          file.before && (
            <Button size="small" variant="primary" icon={Undo2} onClick={() => void restore()}>
              Restore File
            </Button>
          ),
        )}
      </div>
    )
  }

  if (buf.error) return <ErrorBox error={buf.error} onRetry={buf.reload} />
  if (buf.missing) return <EmptyState icon={FileMinus} title="Not on disk now">{file.path}</EmptyState>
  if (!buf.model) return <Loading label="Opening the file…" />
  const model = buf.model

  const revert = () => {
    if (model.isDisposed()) return
    applyText(model, beforeText)
    toast('success', `Reverted ${basename(file.path)} to before ${who}`, {
      detail: 'Not saved yet: Ctrl+Z undoes it, Ctrl+S saves.',
      action: { label: 'Save', run: () => void saveFromHistory(key, file.path) },
    })
  }
  const remove = async () => {
    if (!(await confirmDialog({ title: `Move ${basename(file.path)} to the trash?`, message: `${who} created it.`, confirmLabel: 'Move to Trash', danger: true }))) return
    try {
      await filesApi.op(projectId, 'delete', file.path)
      toast('success', `${basename(file.path)} moved to the trash`)
    } catch (e) {
      toastError(e, `Could not delete ${basename(file.path)}`)
    }
  }
  const state = conflict?.kind === 'deleted' ? ' (deleted on disk)' : conflict ? ' (changed on disk)' : dirty ? ' (unsaved)' : ''
  return (
    <div className="wb-fill">
      {conflict && <ConflictBanner conflict={conflict} bufKey={key} readOnly={buf.readOnly} onOpen={() => openFile({ projectId, path: file.path })} />}
      <div className="wb-lh-head">
        <span className="side wb-ellipsis">{beforeLabel}</span>
        <span className={`side wb-ellipsis${file.changedSince ? ' wb-lh-conflict' : ''}`}>
          Now{state}
          {since}
        </span>
      </div>
      <HistoryDiff path={file.path} original={beforeText} modified={model} readOnly={buf.readOnly} onSave={() => void saveFromHistory(key, file.path)} />
      {actions(
        <>
          {st === 'created' ? (
            <Button size="small" icon={Trash2} onClick={() => void remove()}>
              Delete File…
            </Button>
          ) : (
            <Button size="small" variant="primary" icon={RotateCcw} disabled={buf.readOnly} onClick={revert} title="Replace the buffer with the version before the session (undoable; not saved)">
              Revert File
            </Button>
          )}
          {dirty && (
            <Button size="small" icon={Save} onClick={() => void saveFromHistory(key, file.path)}>
              Save
            </Button>
          )}
          <Button size="small" icon={ExternalLink} onClick={() => openFile({ projectId, path: file.path })}>
            Open File
          </Button>
        </>,
      )}
    </div>
  )
}
