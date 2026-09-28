// Invisible GitLab provider: keeps cached GitLab views fresh from server events,
// toasts when a watched pipeline finishes, and hosts the feature's dialogs.

import { useEffect, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { subscribe } from '@/api/events'
import { toast } from '@/shell/actions'
import { useUi } from '@/state/store'
import { glk } from './api'
import { openPipeline } from './components'
import { GitlabDialogs } from './Dialogs'
import { isActive } from './logic'
import type { PipelineEvent } from './types'

export function GitlabProvider({ children }: { children?: ReactNode }) {
  const qc = useQueryClient()
  useEffect(() => {
    const inv = (key: readonly unknown[]) => void qc.invalidateQueries({ queryKey: key })
    // MR queries: ['gitlab', pid, 'mr', iid] (detail) and ['gitlab', pid, 'mr', iid, part].
    const invMr = (pid: string, iid: number | null, parts: string[]) =>
      void qc.invalidateQueries({
        predicate: (q) => {
          const k = q.queryKey
          if (k[0] !== 'gitlab' || k[1] !== pid || k[2] !== 'mr' || (iid !== null && k[3] !== iid)) return false
          return k.length === 4 || parts.includes(String(k[4]))
        },
      })
    const offs = [
      subscribe('gitlab.pipeline', (ev) => {
        const pid = ev.projectId
        if (!pid) return
        const d = ev.data as PipelineEvent
        inv(glk.summary(pid))
        inv(glk.pipelines(pid))
        inv(glk.pipeline(pid, d.pipelineId))
        invMr(pid, null, ['pipelines'])
        const finished = d.status === 'success' || d.status === 'failed'
        if (pid === useUi.getState().projectId && finished && d.previousStatus && isActive(d.previousStatus)) {
          const name = `Pipeline #${d.iid ?? d.pipelineId} on ${d.ref}`
          toast(d.status === 'failed' ? 'error' : 'success', d.status === 'failed' ? `${name} failed` : `${name} passed`, {
            action: { label: 'Open', run: () => openPipeline(pid, d.pipelineId, d.iid) },
          })
        }
      }),
      subscribe('gitlab.job', (ev) => {
        const pid = ev.projectId
        if (!pid) return
        const d = ev.data as { jobId: number; previousJobId: number; pipelineId: number | null }
        inv(glk.job(pid, d.jobId))
        inv(glk.job(pid, d.previousJobId))
        if (d.pipelineId) inv(glk.pipeline(pid, d.pipelineId))
      }),
      subscribe('gitlab.mr', (ev) => {
        const pid = ev.projectId
        if (!pid) return
        const d = ev.data as { iid: number; action: string }
        inv(glk.mrs(pid))
        const reshaped = ['updated', 'rebase', 'merged'].includes(d.action)
        invMr(pid, d.iid, reshaped ? ['discussions', 'commits', 'pipelines', 'diffs'] : ['discussions'])
        inv(glk.summary(pid))
      }),
      subscribe('gitlab.issue', (ev) => {
        const pid = ev.projectId
        if (!pid) return
        const d = ev.data as { iid: number }
        inv(glk.issues(pid))
        inv(glk.issue(pid, d.iid))
        inv(glk.summary(pid))
      }),
      // A new HEAD or branch changes "current branch" pipeline, MR and CI status.
      subscribe('git.changed', (ev) => inv(ev.projectId ? glk.summary(ev.projectId) : ['gitlab'])),
      subscribe('resync', () => inv(['gitlab'])),
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
