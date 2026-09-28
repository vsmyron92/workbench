// The Activity tool window: MCP tool calls made by agents (writes highlighted)
// and notable events — agent attention, environment transitions, deploys, pipelines.

import { useMemo, useState } from 'react'
import {
  Activity as ActivityIcon,
  Bell,
  CheckCircle2,
  Eraser,
  Globe,
  MessageSquare,
  Rocket,
  SquareTerminal,
  Workflow,
  XCircle,
} from 'lucide-react'
import { openPanel } from '@/shell/actions'
import { Badge, Checkbox, EmptyState, ErrorBox, IconButton, Loading, Tabs, Toolbar } from '@/ui'
import { useActivityFeed } from './api'
import { clock, formatMs, timeline, type TimelineFilter, type TimelineItem } from './lib'
import type { ActivityEvent, McpCall } from './types'
import './platform.css'

const EVENT_ICONS: Record<string, typeof Bell> = {
  attention: Bell,
  env: Globe,
  deploy: Rocket,
  pipeline: Workflow,
  notify: MessageSquare,
}

function CallRow({ c, open, onToggle }: { c: McpCall; open: boolean; onToggle: () => void }) {
  return (
    <>
      <div
        className={['wb-act-row', c.mutating && 'writes', open && 'open'].filter(Boolean).join(' ')}
        onClick={onToggle}
        title={c.error ?? c.summary}
      >
        <span className="wb-act-time">{clock(c.at)}</span>
        <span className={`wb-act-icon ${c.ok ? 'success' : 'error'}`}>{c.ok ? <CheckCircle2 size={13} /> : <XCircle size={13} />}</span>
        <span className="wb-act-session wb-ellipsis" title={c.terminalId ? `terminal ${c.terminalId}` : 'Called with the Workbench token (script or CLI)'}>
          {c.session ?? (c.terminalId ? 'agent' : 'script')}
        </span>
        <span className="wb-act-tool">{c.tool}</span>
        {c.mutating && <Badge tone="warning">writes</Badge>}
        <span className="wb-act-summary wb-ellipsis">{c.ok ? c.summary : (c.error ?? c.summary)}</span>
        <span className="wb-act-ms">{formatMs(c.ms)}</span>
      </div>
      {open && (
        <div className="wb-act-detail">
          {c.summary && <div className="mono">{c.summary}</div>}
          {c.error && <div className="wb-danger">{c.error}</div>}
          <div className="wb-row">
            <span>{new Date(c.at).toLocaleString()}</span>
            {c.projectId && <span>· {c.projectId}</span>}
            {c.terminalId && (
              <button
                className="wb-btn small ghost"
                onClick={() => openPanel({ kind: 'terminal', id: `terminal:${c.terminalId}`, title: c.session ?? 'Agent', params: { terminalId: c.terminalId } })}
              >
                <SquareTerminal size={12} /> Open session
              </button>
            )}
          </div>
        </div>
      )}
    </>
  )
}

function EventRow({ e, open, onToggle }: { e: ActivityEvent; open: boolean; onToggle: () => void }) {
  const I = EVENT_ICONS[e.kind] ?? ActivityIcon
  return (
    <>
      <div className={open ? 'wb-act-row open' : 'wb-act-row'} onClick={onToggle} title={e.message}>
        <span className="wb-act-time">{clock(e.at)}</span>
        <span className={`wb-act-icon ${e.level}`}>
          <I size={13} />
        </span>
        <span className="wb-act-title wb-ellipsis">{e.title}</span>
        <span className="wb-act-msg wb-ellipsis">{e.message}</span>
      </div>
      {open && (
        <div className="wb-act-detail">
          <div>{e.message}</div>
          <div className="wb-row">
            <span>{new Date(e.at).toLocaleString()}</span>
            {e.terminalId && (
              <button
                className="wb-btn small ghost"
                onClick={() => openPanel({ kind: 'terminal', id: `terminal:${e.terminalId}`, title: 'Agent', params: { terminalId: e.terminalId } })}
              >
                <SquareTerminal size={12} /> Open session
              </button>
            )}
          </div>
        </div>
      )}
    </>
  )
}

const itemKey = (i: TimelineItem) => (i.type === 'call' ? `c${i.call.id}` : `e${i.event.id}`)

/** The activity list. `compact` hides the filter toolbar (phone). */
export function ActivityList({ projectId, compact, limit = 500 }: { projectId?: string | null; compact?: boolean; limit?: number }) {
  const feed = useActivityFeed()
  const [filter, setFilter] = useState<TimelineFilter>({ show: 'all', writesOnly: false, errorsOnly: false, since: 0 })
  const [thisProject, setThisProject] = useState(false)
  const [open, setOpen] = useState<string | null>(null)
  const items = useMemo(() => {
    if (!feed.data) return []
    const byProject = <T extends { projectId: string | null }>(xs: T[]) => (thisProject && projectId ? xs.filter((x) => x.projectId === projectId) : xs)
    return timeline(byProject(feed.data.calls), byProject(feed.data.events), filter).slice(0, limit)
  }, [feed.data, filter, thisProject, projectId, limit])

  return (
    <div className="wb-act">
      {!compact && (
        <Toolbar>
          <Tabs
            value={filter.show}
            onChange={(show) => setFilter({ ...filter, show })}
            tabs={[
              { id: 'all', label: 'All' },
              { id: 'calls', label: 'Tool calls' },
              { id: 'events', label: 'Events' },
            ]}
          />
          <span style={{ flex: 1 }} />
          <Checkbox checked={filter.writesOnly} onChange={(writesOnly) => setFilter({ ...filter, writesOnly })}>
            <span className="wb-small">Writes</span>
          </Checkbox>
          <span style={{ width: 8 }} />
          <Checkbox checked={filter.errorsOnly} onChange={(errorsOnly) => setFilter({ ...filter, errorsOnly })}>
            <span className="wb-small">Errors</span>
          </Checkbox>
          {projectId && (
            <>
              <span style={{ width: 8 }} />
              <Checkbox checked={thisProject} onChange={setThisProject}>
                <span className="wb-small">This project</span>
              </Checkbox>
            </>
          )}
          <span style={{ width: 4 }} />
          <IconButton icon={Eraser} size="small" label="Clear" onClick={() => setFilter({ ...filter, since: Date.now() })} />
        </Toolbar>
      )}
      <div className="wb-act-list">
        {feed.isLoading ? (
          <Loading />
        ) : feed.error ? (
          <ErrorBox error={feed.error} onRetry={() => void feed.refetch()} />
        ) : items.length === 0 ? (
          <EmptyState icon={ActivityIcon} title="No activity yet">
            MCP tool calls made by agents and notable events (agents needing attention, environments going down, deploys) appear here.
          </EmptyState>
        ) : (
          items.map((i) => {
            const k = itemKey(i)
            const toggle = () => setOpen(open === k ? null : k)
            return i.type === 'call' ? (
              <CallRow key={k} c={i.call} open={open === k} onToggle={toggle} />
            ) : (
              <EventRow key={k} e={i.event} open={open === k} onToggle={toggle} />
            )
          })
        )}
      </div>
    </div>
  )
}

export function ActivityToolWindow({ projectId }: { projectId: string | null }) {
  return <ActivityList projectId={projectId} />
}
