// Mounted once while signed in: keeps git queries fresh from events, tracks
// remote operations, receives agent-written commit messages, and hosts the
// branches popover, git dialogs and the progress cards of fetch/pull/push.

import { useEffect, type ReactNode } from 'react'
import type { editor } from 'monaco-editor'
import { useQueryClient, type Query } from '@tanstack/react-query'
import { ArrowDownToLine, CheckCircle2, GitMerge, PauseCircle, Play, ScrollText, TriangleAlert, X, XCircle } from 'lucide-react'
import { api } from '@/api/client'
import { subscribe } from '@/api/events'
import { inProject, scopeOfEvent, scopeOfFile, scopeRepo, setActiveRepo, splitScope } from '@/api/repos'
import { modelFile } from '@/features/files/modelAccess'
import { showToolWindow, toast } from '@/shell/actions'
import { Button, IconButton, Spinner } from '@/ui'
import { gitUrl } from './api'
import { openGitLog, resolveConflicts, sequencer, updateProject } from './actions'
import { BranchesPopover } from './BranchesPopover'
import { GitDialogs } from './Dialogs'
import { RepoPopover } from './RepoSwitcher'
import { useDrafts, useOps } from './store'
import type { GitOpEvent } from './types'
import './git.css'

/** Diff queries that depend on the working tree or index (commit diffs never change). */
const liveDiff = (q: Query) => q.queryKey[2] !== 'diff' || q.queryKey[3] === 'working' || q.queryKey[3] === 'staged' || q.queryKey[3] === 'compare'

export function GitProvider({ children }: { children?: ReactNode }) {
  const qc = useQueryClient()
  useEffect(() => {
    const fsTimers = new Map<string, number>()
    // The repository switcher's list runs a status per repository, so it refreshes less eagerly
    // than the views: soon after git moved, later after file edits (an earlier due time wins).
    const repoDue = new Map<string, { at: number; timer: number }>()
    // The views of every repository of a project: their keys start with ['git', <scope>].
    const invalidate = (projectId: string, keep: (q: Query) => boolean = () => true) =>
      void qc.invalidateQueries({ predicate: (q) => q.queryKey[0] === 'git' && inProject(q.queryKey[1], projectId) && keep(q) })
    const refreshRepos = (projectId: string, delay: number) => {
      const at = Date.now() + delay
      const cur = repoDue.get(projectId)
      if (cur && cur.at <= at) return
      window.clearTimeout(cur?.timer)
      const timer = window.setTimeout(() => {
        repoDue.delete(projectId)
        invalidate(projectId, (q) => q.queryKey[2] === 'repos')
      }, delay)
      repoDue.set(projectId, { at, timer })
    }
    const offs = [
      subscribe('git.changed', (ev) => {
        if (!ev.projectId) return
        invalidate(ev.projectId, (q) => q.queryKey[2] !== 'repos' && liveDiff(q))
        refreshRepos(ev.projectId, 400)
      }),
      // File edits change status and working diffs; coalesce bursts (formatters, agents).
      subscribe('fs.changed', (ev) => {
        const pid = ev.projectId
        if (!pid) return
        window.clearTimeout(fsTimers.get(pid))
        fsTimers.set(
          pid,
          window.setTimeout(() => {
            fsTimers.delete(pid)
            invalidate(pid, (q) => q.queryKey[2] === 'status' || q.queryKey[2] === 'changelists' || (q.queryKey[2] === 'diff' && liveDiff(q)))
          }, 350),
        )
        refreshRepos(pid, 2000)
      }),
      subscribe('git.op', (ev) => {
        if (ev.projectId) useOps.getState().event(scopeOfEvent(ev.projectId, ev.data), ev.data as GitOpEvent)
      }),
      subscribe('git.changelists', (ev) => {
        if (ev.projectId) invalidate(ev.projectId, (q) => q.queryKey[2] === 'changelists')
      }),
      subscribe('git.shelf', (ev) => {
        if (ev.projectId) invalidate(ev.projectId, (q) => q.queryKey[2] === 'shelf')
      }),
      subscribe('git.commitMessage', (ev) => {
        const d = ev.data as { projectId?: string; repo?: string; message?: string }
        const pid = d.projectId ?? ev.projectId
        if (!pid || !d.message) return
        const scope = scopeOfEvent(pid, d)
        useDrafts.getState().update(scope, { message: d.message, amend: false, amendLoaded: undefined })
        // The Commit window shows the repository the message is for.
        setActiveRepo(pid, scopeRepo(scope))
        showToolWindow('commit')
        useDrafts.getState().focus()
        toast('info', 'An agent wrote a commit message — review it in the Commit window', { timeout: 6000 })
      }),
      subscribe('resync', () => void qc.invalidateQueries({ queryKey: ['git'] })),
    ]
    return () => {
      offs.forEach((o) => o())
      fsTimers.forEach((t) => window.clearTimeout(t))
      repoDue.forEach((d) => window.clearTimeout(d.timer))
    }
  }, [qc])
  useHistoryForSelection()
  return (
    <>
      {children}
      <OpsCards />
      <BranchesPopover />
      <RepoPopover />
      <GitDialogs />
    </>
  )
}

function OpsCards() {
  const ops = useOps((s) => s.ops)
  const { dismiss, toggleLog } = useOps.getState()
  const list = Object.values(ops).sort((a, b) => a.startedAt - b.startedAt)
  if (!list.length) return null
  return (
    <div className="git-ops" aria-live="polite">
      {list.map((op) => (
        <div key={op.opId} className={`wb-toast git-op-card ${op.done ? (op.ok ? 'success' : op.conflicts || op.stopped ? 'warning' : 'error') : 'info'}`}>
          <span className="icon" style={{ marginTop: 2 }}>
            {!op.done ? (
              <Spinner size={15} />
            ) : op.ok ? (
              <CheckCircle2 size={16} />
            ) : op.conflicts ? (
              <TriangleAlert size={16} />
            ) : op.stopped ? (
              <PauseCircle size={16} />
            ) : (
              <XCircle size={16} />
            )}
          </span>
          <div className="wb-grow">
            <div className="title">
              {op.title}
              {op.projectId && splitScope(op.projectId).repo && <span className="wb-muted"> · {splitScope(op.projectId).repo}</span>}
            </div>
            {!op.done && <div className="line">{op.lastLine || 'Working…'}</div>}
            {op.done && op.ok && <div className="wb-small wb-muted">{op.message}</div>}
            {op.done && !op.ok && (
              <>
                <div className={op.conflicts || op.stopped ? 'wb-small' : 'err'}>{op.message}</div>
                <div className="wb-row" style={{ marginTop: 6 }}>
                  {op.conflicts && op.projectId && (
                    <Button size="small" variant="primary" icon={GitMerge} onClick={() => void resolveConflicts(op.projectId!)}>
                      Resolve Conflicts
                    </Button>
                  )}
                  {op.stopped && !op.conflicts && op.projectId && (
                    <Button
                      size="small"
                      variant="primary"
                      icon={Play}
                      onClick={() => {
                        dismiss(op.opId)
                        void sequencer(op.projectId!, 'continue', 'Rebasing')
                      }}
                    >
                      Continue
                    </Button>
                  )}
                  {op.stopped && op.projectId && (
                    <Button
                      size="small"
                      onClick={() => {
                        dismiss(op.opId)
                        void sequencer(op.projectId!, 'abort', 'Rebasing')
                      }}
                    >
                      Abort
                    </Button>
                  )}
                  {op.lines.length > 0 && (
                    <Button size="small" icon={ScrollText} onClick={() => toggleLog(op.opId)}>
                      {op.showLog ? 'Hide log' : 'Show log'}
                    </Button>
                  )}
                  {op.op === 'push' && op.projectId && /non-fast-forward|fetch first/.test(op.message ?? '') && (
                    <Button
                      size="small"
                      icon={ArrowDownToLine}
                      onClick={() => {
                        dismiss(op.opId)
                        void updateProject(op.projectId!)
                      }}
                    >
                      Update Project
                    </Button>
                  )}
                </div>
                {op.showLog && <pre className="git-op-log">{op.lines.join('\n')}</pre>}
              </>
            )}
          </div>
          {op.done ? (
            <IconButton icon={X} size="small" label="Dismiss" onClick={() => dismiss(op.opId)} />
          ) : (
            <IconButton
              icon={X}
              size="small"
              label="Cancel"
              onClick={() => {
                if (op.projectId) void api.post(gitUrl(op.projectId, `ops/${encodeURIComponent(op.opId)}/cancel`)).catch(() => {})
              }}
            />
          )}
        </div>
      ))}
    </div>
  )
}

/**
 * "Show History for Selection" in every file editor (CLion's Git ▸ Show History for
 * Selection): an editor action on models of project files (`file:///<projectId>/<path>`,
 * the files contract), opening the log of the selected lines (`git log -L`).
 */
function useHistoryForSelection() {
  useEffect(() => {
    let alive = true
    const subs: { dispose(): void }[] = []
    void import('@/lib/monacoSetup').then(({ monaco }) => {
      if (!alive) return
      const projectFile = (uri: Parameters<typeof modelFile>[0] | undefined) => (uri ? modelFile(uri, true) : null)
      const seen = new WeakSet<object>()
      const hook = (created: editor.ICodeEditor) => {
        // Code editors are standalone editors (the diff editor's sides too).
        const ed = created as editor.IStandaloneCodeEditor
        if (seen.has(ed) || typeof ed.addAction !== 'function' || typeof ed.createContextKey !== 'function') return
        seen.add(ed)
        const isFile = ed.createContextKey<boolean>('wbGitProjectFile', false)
        const sync = () => isFile.set(!!projectFile(ed.getModel()?.uri))
        sync()
        ed.onDidChangeModel(sync)
        ed.addAction({
          id: 'wb.git.historyForSelection',
          label: 'Git: Show History for Selection',
          precondition: 'wbGitProjectFile',
          contextMenuGroupId: '9_git',
          contextMenuOrder: 1,
          run: (e: editor.ICodeEditor) => {
            const f = projectFile(e.getModel()?.uri)
            const s = e.getSelection()
            if (!f || !s) return
            const end = s.endLineNumber > s.startLineNumber && s.endColumn === 1 ? s.endLineNumber - 1 : s.endLineNumber
            // Editor lines are working-tree lines: the server maps them onto HEAD's version.
            openGitLog(scopeOfFile(f.projectId, f.path), { path: f.path, lines: [s.startLineNumber, Math.max(s.startLineNumber, end)], worktreeLines: true })
          },
        })
      }
      // The event fires inside the base editor's constructor, before a standalone
      // editor can take actions: add ours once construction is over.
      subs.push(monaco.editor.onDidCreateEditor((ed) => queueMicrotask(() => alive && hook(ed))))
      monaco.editor.getEditors().forEach(hook)
    })
    return () => {
      alive = false
      subs.forEach((s) => s.dispose())
    }
  }, [])
}
