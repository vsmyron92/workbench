// State icons of pull requests and issues, shared by the lists (tool window,
// phone tab) and the panels without making the lists load the panels.

import { CircleCheck, CircleDot, CircleSlash, GitMerge, GitPullRequest, GitPullRequestClosed, GitPullRequestDraft } from 'lucide-react'
import { prState } from './logic'
import type { Pull } from './types'

export function PrStateIcon({ pr, size = 15 }: { pr: Pick<Pull, 'state' | 'draft' | 'merged' | 'mergedAt'>; size?: number }) {
  const s = prState(pr)
  if (s === 'merged') return <span className="gh-status gh-tone-accent" title="merged"><GitMerge size={size} /></span>
  if (s === 'closed') return <span className="gh-status gh-tone-danger" title="closed"><GitPullRequestClosed size={size} /></span>
  if (s === 'draft') return <span className="gh-status gh-tone-muted" title="draft"><GitPullRequestDraft size={size} /></span>
  return <span className="gh-status gh-tone-success" title="open"><GitPullRequest size={size} /></span>
}

export function IssueStateIcon({ state, reason, size = 15 }: { state: string; reason?: string | null; size?: number }) {
  if (state !== 'closed')
    return (
      <span className="gh-status gh-tone-success" title="open">
        <CircleDot size={size} />
      </span>
    )
  return reason === 'not_planned' ? (
    <span className="gh-status gh-tone-muted" title="closed as not planned">
      <CircleSlash size={size} />
    </span>
  ) : (
    <span className="gh-status gh-tone-accent" title="closed as completed">
      <CircleCheck size={size} />
    </span>
  )
}
