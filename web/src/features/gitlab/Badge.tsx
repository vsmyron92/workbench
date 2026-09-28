// Dot on the GitLab stripe icon / CI tab: the current branch's pipeline is
// running (pulsing) or failed.

import { StatusDot } from '@/ui'
import { useGitlabSummary, useHasGitlab } from './api'
import { isActive } from './logic'

export function GitlabBadge({ projectId }: { projectId: string | null }) {
  const has = useHasGitlab(projectId)
  const summary = useGitlabSummary(projectId, has)
  const p = summary.data?.branchPipeline
  if (!has || !p) return null
  if (p.status === 'failed') return <StatusDot tone="danger" title="Pipeline failed" />
  if (isActive(p.status)) return <StatusDot tone="accent" pulse title="Pipeline running" />
  return null
}
