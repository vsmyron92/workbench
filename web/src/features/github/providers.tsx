// Invisible GitHub provider: keeps cached GitHub views fresh from server events,
// toasts when a watched run finishes, and hosts the feature's dialogs.

import { lazy, Suspense, useEffect, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { subscribe } from '@/api/events'
import { eventMatches, eventRepo, scopeOfEvent } from '@/api/repos'
import { toast } from '@/shell/actions'
import { useUi } from '@/state/store'
import { openRun, useGhUi } from './components'
import { isActive } from './logic'
import type { JobLog, RunEvent } from './types'

// Loaded the first time a dialog opens.
const GithubDialogs = lazy(() => import('./Dialogs').then((m) => ({ default: m.GithubDialogs })))

export function GithubProvider({ children }: { children?: ReactNode }) {
  const qc = useQueryClient()
  const dialogOpen = useGhUi((s) => !!s.createPr || !!s.runWorkflow)
  useEffect(() => {
    // Queries ['github', <scope>, ...rest] of the event's repository (prefix match, like a query key).
    const inv = (pid: string, data: unknown, ...rest: unknown[]) =>
      void qc.invalidateQueries({
        predicate: (q) => q.queryKey[0] === 'github' && eventMatches(q.queryKey[1], pid, data) && rest.every((r, i) => q.queryKey[i + 2] === r),
      })
    // Pull request queries: ['github', scope, 'pull', n] (detail) and ['github', scope, 'pull', n, part].
    const invPull = (pid: string, data: unknown, n: number | null, parts: string[]) =>
      void qc.invalidateQueries({
        predicate: (q) => {
          const k = q.queryKey
          if (k[0] !== 'github' || !eventMatches(k[1], pid, data) || k[2] !== 'pull' || (n !== null && k[3] !== n)) return false
          return k.length === 4 || parts.includes(String(k[4]))
        },
      })
    // Job details, and job logs not published yet (a run moved, so its jobs
    // did; anonymous views do not poll). Published logs never change.
    const invJobs = (pid: string, data: unknown) =>
      void qc.invalidateQueries({
        predicate: (q) => {
          const k = q.queryKey
          if (k[0] !== 'github' || !eventMatches(k[1], pid, data) || k[2] !== 'job') return false
          return k.length === 4 || (k[4] === 'log' && (q.state.data as JobLog | undefined)?.available === false)
        },
      })
    const offs = [
      subscribe('github.run', (ev) => {
        const pid = ev.projectId
        if (!pid) return
        const d = ev.data as RunEvent
        inv(pid, d, 'summary')
        inv(pid, d, 'runs')
        if (d.runId) inv(pid, d, 'run', d.runId)
        invJobs(pid, d)
        invPull(pid, d, null, [])
        const finished = d.state === 'success' || d.state === 'failed'
        if (d.runId && pid === useUi.getState().projectId && finished && d.previousState && isActive(d.previousState)) {
          const scope = scopeOfEvent(pid, d)
          // Another repository of the project than the default one: say which.
          const where = eventRepo(d) && scope !== pid ? ` (${eventRepo(d)})` : ''
          const name = `${d.name ?? 'Workflow'} #${d.runNumber ?? d.runId}${d.branch ? ` on ${d.branch}` : ''}${where}`
          const runId = d.runId
          toast(d.state === 'failed' ? 'error' : 'success', d.state === 'failed' ? `${name} failed` : `${name} passed`, {
            action: { label: 'Open', run: () => openRun(scope, runId, `${d.name ?? 'Run'} #${d.runNumber ?? runId}`) },
          })
        }
      }),
      subscribe('github.job', (ev) => {
        const pid = ev.projectId
        if (!pid) return
        const d = ev.data as { jobId: number; runId: number }
        inv(pid, d, 'job', d.jobId)
        inv(pid, d, 'run', d.runId)
      }),
      subscribe('github.pr', (ev) => {
        const pid = ev.projectId
        if (!pid) return
        const d = ev.data as { number: number; action: string }
        inv(pid, d, 'pulls')
        const reshaped = ['updated', 'merged'].includes(d.action)
        invPull(pid, d, d.number, reshaped ? ['threads', 'comments', 'reviews', 'commits', 'files'] : ['threads', 'comments', 'reviews'])
        inv(pid, d, 'summary')
      }),
      subscribe('github.issue', (ev) => {
        const pid = ev.projectId
        if (!pid) return
        const d = ev.data as { number: number }
        inv(pid, d, 'issues')
        inv(pid, d, 'issue', d.number)
        inv(pid, d, 'summary')
      }),
      // A new HEAD or branch changes the current branch's runs, pull request and checks.
      subscribe('git.changed', (ev) => (ev.projectId ? inv(ev.projectId, ev.data, 'summary') : void qc.invalidateQueries({ queryKey: ['github'] }))),
      subscribe('resync', () => void qc.invalidateQueries({ queryKey: ['github'] })),
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
