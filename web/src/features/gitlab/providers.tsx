// Invisible GitLab provider: keeps cached GitLab views fresh from server events,
// toasts when a watched pipeline finishes, and hosts the feature's dialogs.
//
// Views are keyed by repository scope (`api/repos.ts`); an event names the project and, in
// `data.repo`, the repository it is about, so it refreshes that repository's views (every
// repository of the project when it names none).

import { useEffect, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { subscribe } from '@/api/events'
import { eventMatches, eventRepo, scopeOfEvent } from '@/api/repos'
import { toast } from '@/shell/actions'
import { useUi } from '@/state/store'
import { openPipeline } from './components'
import { GitlabDialogs } from './Dialogs'
import { isActive } from './logic'
import type { PipelineEvent } from './types'

export function GitlabProvider({ children }: { children?: ReactNode }) {
  const qc = useQueryClient()
  useEffect(() => {
    // Queries ['gitlab', <scope>, ...rest] of the event's repository (prefix match, like a query key).
    const inv = (pid: string, data: unknown, ...rest: unknown[]) =>
      void qc.invalidateQueries({
        predicate: (q) => q.queryKey[0] === 'gitlab' && eventMatches(q.queryKey[1], pid, data) && rest.every((r, i) => q.queryKey[i + 2] === r),
      })
    // MR queries: ['gitlab', scope, 'mr', iid] (detail) and ['gitlab', scope, 'mr', iid, part].
    const invMr = (pid: string, data: unknown, iid: number | null, parts: string[]) =>
      void qc.invalidateQueries({
        predicate: (q) => {
          const k = q.queryKey
          if (k[0] !== 'gitlab' || !eventMatches(k[1], pid, data) || k[2] !== 'mr' || (iid !== null && k[3] !== iid)) return false
          return k.length === 4 || parts.includes(String(k[4]))
        },
      })
    const offs = [
      subscribe('gitlab.pipeline', (ev) => {
        const pid = ev.projectId
        if (!pid) return
        const d = ev.data as PipelineEvent
        inv(pid, d, 'summary')
        inv(pid, d, 'pipelines')
        inv(pid, d, 'pipeline', d.pipelineId)
        invMr(pid, d, null, ['pipelines'])
        const finished = d.status === 'success' || d.status === 'failed'
        if (pid === useUi.getState().projectId && finished && d.previousStatus && isActive(d.previousStatus)) {
          const scope = scopeOfEvent(pid, d)
          // Another repository of the project than the default one: say which.
          const where = eventRepo(d) && scope !== pid ? ` (${eventRepo(d)})` : ''
          const name = `Pipeline #${d.iid ?? d.pipelineId} on ${d.ref}${where}`
          toast(d.status === 'failed' ? 'error' : 'success', d.status === 'failed' ? `${name} failed` : `${name} passed`, {
            action: { label: 'Open', run: () => openPipeline(scope, d.pipelineId, d.iid) },
          })
        }
      }),
      subscribe('gitlab.job', (ev) => {
        const pid = ev.projectId
        if (!pid) return
        const d = ev.data as { jobId: number; previousJobId: number; pipelineId: number | null }
        inv(pid, d, 'job', d.jobId)
        inv(pid, d, 'job', d.previousJobId)
        if (d.pipelineId) inv(pid, d, 'pipeline', d.pipelineId)
      }),
      subscribe('gitlab.mr', (ev) => {
        const pid = ev.projectId
        if (!pid) return
        const d = ev.data as { iid: number; action: string }
        inv(pid, d, 'mrs')
        const reshaped = ['updated', 'rebase', 'merged'].includes(d.action)
        invMr(pid, d, d.iid, reshaped ? ['discussions', 'commits', 'pipelines', 'diffs'] : ['discussions'])
        inv(pid, d, 'summary')
      }),
      subscribe('gitlab.issue', (ev) => {
        const pid = ev.projectId
        if (!pid) return
        const d = ev.data as { iid: number }
        inv(pid, d, 'issues')
        inv(pid, d, 'issue', d.iid)
        inv(pid, d, 'summary')
      }),
      // A new HEAD or branch changes "current branch" pipeline, MR and CI status.
      subscribe('git.changed', (ev) => (ev.projectId ? inv(ev.projectId, ev.data, 'summary') : void qc.invalidateQueries({ queryKey: ['gitlab'] }))),
      subscribe('resync', () => void qc.invalidateQueries({ queryKey: ['gitlab'] })),
    ]
    return () => offs.forEach((off) => off())
  }, [qc])
  return (
    <>
      {children}
      <GitlabDialogs />
    </>
  )
}
