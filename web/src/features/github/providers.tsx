// Invisible GitHub provider: keeps cached GitHub views fresh from server events,
// toasts when a watched run finishes, and hosts the feature's dialogs.

import { lazy, Suspense, useEffect, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { subscribe } from '@/api/events'
import { toast } from '@/shell/actions'
import { useUi } from '@/state/store'
import { ghk } from './api'
import { openRun, useGhUi } from './components'
import { isActive } from './logic'
import type { JobLog, RunEvent } from './types'

// Loaded the first time a dialog opens.
const GithubDialogs = lazy(() => import('./Dialogs').then((m) => ({ default: m.GithubDialogs })))

export function GithubProvider({ children }: { children?: ReactNode }) {
  const qc = useQueryClient()
  const dialogOpen = useGhUi((s) => !!s.createPr || !!s.runWorkflow)
  useEffect(() => {
    const inv = (key: readonly unknown[]) => void qc.invalidateQueries({ queryKey: key })
    // Pull request queries: ['github', pid, 'pull', n] (detail) and ['github', pid, 'pull', n, part].
    const invPull = (pid: string, n: number | null, parts: string[]) =>
      void qc.invalidateQueries({
        predicate: (q) => {
          const k = q.queryKey
          if (k[0] !== 'github' || k[1] !== pid || k[2] !== 'pull' || (n !== null && k[3] !== n)) return false
          return k.length === 4 || parts.includes(String(k[4]))
        },
      })
    // Job details, and job logs not published yet (a run moved, so its jobs
    // did; anonymous views do not poll). Published logs never change.
    const invJobs = (pid: string) =>
      void qc.invalidateQueries({
        predicate: (q) => {
          const k = q.queryKey
          if (k[0] !== 'github' || k[1] !== pid || k[2] !== 'job') return false
          return k.length === 4 || (k[4] === 'log' && (q.state.data as JobLog | undefined)?.available === false)
        },
      })
    const offs = [
      subscribe('github.run', (ev) => {
        const pid = ev.projectId
        if (!pid) return
        const d = ev.data as RunEvent
        inv(ghk.summary(pid))
        inv(ghk.runs(pid))
        if (d.runId) inv(ghk.run(pid, d.runId))
        invJobs(pid)
        invPull(pid, null, [])
        const finished = d.state === 'success' || d.state === 'failed'
        if (d.runId && pid === useUi.getState().projectId && finished && d.previousState && isActive(d.previousState)) {
          const name = `${d.name ?? 'Workflow'} #${d.runNumber ?? d.runId}${d.branch ? ` on ${d.branch}` : ''}`
          const runId = d.runId
          toast(d.state === 'failed' ? 'error' : 'success', d.state === 'failed' ? `${name} failed` : `${name} passed`, {
            action: { label: 'Open', run: () => openRun(pid, runId, `${d.name ?? 'Run'} #${d.runNumber ?? runId}`) },
          })
        }
      }),
      subscribe('github.job', (ev) => {
        const pid = ev.projectId
        if (!pid) return
        const d = ev.data as { jobId: number; runId: number }
        inv(ghk.job(pid, d.jobId))
        inv(ghk.run(pid, d.runId))
      }),
      subscribe('github.pr', (ev) => {
        const pid = ev.projectId
        if (!pid) return
        const d = ev.data as { number: number; action: string }
        inv(ghk.pulls(pid))
        const reshaped = ['updated', 'merged'].includes(d.action)
        invPull(pid, d.number, reshaped ? ['threads', 'comments', 'reviews', 'commits', 'files'] : ['threads', 'comments', 'reviews'])
        inv(ghk.summary(pid))
      }),
      subscribe('github.issue', (ev) => {
        const pid = ev.projectId
        if (!pid) return
        const d = ev.data as { number: number }
        inv(ghk.issues(pid))
        inv(ghk.issue(pid, d.number))
        inv(ghk.summary(pid))
      }),
      // A new HEAD or branch changes the current branch's runs, pull request and checks.
      subscribe('git.changed', (ev) => inv(ev.projectId ? ghk.summary(ev.projectId) : ['github'])),
      subscribe('resync', () => inv(['github'])),
    ]
    return () => offs.forEach((off) => off())
  }, [qc])
  return (
    <>
      {children}
      {dialogOpen && (
        <Suspense fallback={null}>
          <GithubDialogs />
        </Suspense>
      )}
    </>
  )
}
