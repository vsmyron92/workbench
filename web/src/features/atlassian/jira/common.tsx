// Small Jira presentation helpers shared by the tool window and the issue panel.

import { Bookmark, Bug, CheckSquare, ListTree, Zap } from 'lucide-react'
import { Badge } from '@/ui'
import type { JiraStatus, Named } from '../api'
import { statusTone } from '../links'

export function IssueTypeIcon({ type, size = 14 }: { type: Named | null; size?: number }) {
  const n = (type?.name ?? '').toLowerCase()
  if (n.includes('bug')) return <span className="jira-type bug" title={type?.name}><Bug size={size} /></span>
  if (n.includes('story')) return <span className="jira-type story" title={type?.name}><Bookmark size={size} /></span>
  if (n.includes('epic')) return <span className="jira-type epic" title={type?.name}><Zap size={size} /></span>
  if (n.includes('sub')) return <span className="jira-type" title={type?.name}><ListTree size={size} /></span>
  return <span className="jira-type" title={type?.name ?? 'Task'}><CheckSquare size={size} /></span>
}

export function StatusBadge({ status }: { status: JiraStatus | null }) {
  if (!status) return null
  return <Badge tone={statusTone(status.category)}>{status.name}</Badge>
}
