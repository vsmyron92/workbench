// Dot on the GitHub stripe icon / phone tab: the current branch's newest run
// is running (pulsing) or failed.

import { StatusDot } from '@/ui'
import { useGithubSummary, useHasGithub } from './api'
import { isActive } from './logic'

export function GithubBadge({ projectId }: { projectId: string | null }) {
  const has = useHasGithub(projectId)
  const summary = useGithubSummary(projectId, has)
  const r = summary.data?.branchRun
  if (!has || !r) return null
  if (r.state === 'failed') return <StatusDot tone="danger" title="Workflow run failed" />
  if (isActive(r.state)) return <StatusDot tone="accent" pulse title="Workflow running" />
  return null
}
