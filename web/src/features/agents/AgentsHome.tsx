// 'agents.home' — the first tab of the agents column: a composer for a new session (and
// a button for a shell), the project's sessions (attention first), the ones closed
// lately, the session history (resume / fork), sessions running elsewhere on this
// machine, and Remote Control servers.

import { useMemo, useState } from 'react'
import {
  AlertTriangle,
  Bot,
  Copy,
  ExternalLink,
  GitBranch,
  GitFork,
  Hash,
  History,
  MonitorSmartphone,
  MoreHorizontal,
  Play,
  Power,
  Radio,
  SquareTerminal,
} from 'lucide-react'
import { useProjects, useTerminals } from '@/api/queries'
import type { TerminalInfo } from '@/api/types'
import { useUi } from '@/state/store'
import { Badge, Button, Checkbox, EmptyState, ErrorBox, IconButton, Input, Loading, Section, showMenu, showMenuAt, Spinner, Tabs, TimeAgo, formatBytes } from '@/ui'
import { copyText, killTerminal, openRemote, restartTerminal, terminalMenu } from './actions'
import { openTerminal, startAgent, useAgentDefaults, useAgentHistory, useExternalSessions, type HistoryEntry, type ProviderInfo } from './api'
import { pendingOf } from './lib/permission'
import { dialogSeenOnScreen, displayLabel, historyProviders, providerKindOf, reportsAnswers, resumes, SEEN_ON_SCREEN } from './lib/providers'
import { newShell } from './commands'
import { agentMeta, agentSessions, colorCss, counts, isRunning, lastActivity, sortSessions, tone } from './lib/sessions'
import { NewSessionForm } from './NewSession'
import { ColorBar, ContainerBadge, ProviderBadge, ProviderIcon, RemoteLink, StateChip, StateDot } from './parts'
import { PermissionRequest } from './Permission'
import { useAgentsUi } from './store'

function contractHome(p: string): string {
  return p.replace(/^\/home\/[^/]+/, '~')
}

/** What a card says before (or without) an answer. */
function emptyMessage(t: TerminalInfo, running: boolean): string {
  if (!running) return 'Stopped'
  // CLIs that report nothing (Kimi, Gemini, Aider, custom): their answers stay in the terminal.
  if (!reportsAnswers(providerKindOf(t))) return t.agent?.state === 'working' ? 'Working…' : 'Its answers are in the terminal'
  return 'No answer yet'
}

export function SessionCard({ t, showProject }: { t: TerminalInfo; showProject?: boolean }) {
  const a = t.agent
  const running = isRunning(t)
  const attention = running ? a?.attention : null
  const pending = pendingOf(t)
  const meta = agentMeta(a)
  return (
    <div
      className={`wb-ag-card ${tone(t)}`}
      role="button"
      tabIndex={0}
      onClick={() => openTerminal(t)}
      onKeyDown={(e) => e.key === 'Enter' && openTerminal(t)}
      onContextMenu={(e) => showMenu(e, terminalMenu(t))}
    >
      <ColorBar color={t.color} />
      <div className="wb-ag-card-top">
        <StateChip t={t} />
        <span className="wb-ag-card-title wb-ellipsis" title={t.title}>
          {t.title}
        </span>
        <span style={{ flex: 1 }} />
        <span className="wb-subtle wb-xs">
          <TimeAgo time={Math.max(a?.lastEventAt ?? 0, t.lastOutputAt) || t.createdAt} />
        </span>
        <IconButton
          icon={MoreHorizontal}
          size="small"
          label="More"
          onClick={(e) => {
            e.stopPropagation()
            showMenuAt(e.currentTarget, terminalMenu(t))
          }}
        />
      </div>
      {pending ? (
        <PermissionRequest t={t} />
      ) : attention ? (
        <div className="wb-ag-card-attention" title={dialogSeenOnScreen(t) ? SEEN_ON_SCREEN : undefined}>
          <AlertTriangle size={12} />
          <span>{attention}</span>
        </div>
      ) : (
        <div className={a?.lastMessage ? 'wb-ag-card-msg' : 'wb-ag-card-msg empty'}>{a?.lastMessage ?? emptyMessage(t, running)}</div>
      )}
      <div className="wb-ag-card-foot">
        <ProviderBadge t={t} />
        <ContainerBadge t={t} />
        {showProject && t.projectId && <Badge>{t.projectId}</Badge>}
        <span className="wb-ellipsis wb-muted">{meta.join(' · ')}</span>
        <span style={{ flex: 1 }} />
        {a?.remoteUrl && (
          <span onClick={(e) => e.stopPropagation()}>
            <RemoteLink url={a.remoteUrl} compact />
          </span>
        )}
        {running ? (
          <Button
            size="small"
            variant="ghost"
            icon={Power}
            onClick={(e) => {
              e.stopPropagation()
              void killTerminal(t)
            }}
          >
            Stop
          </Button>
        ) : (
          <Button
            size="small"
            icon={Play}
            onClick={(e) => {
              e.stopPropagation()
              void restartTerminal(t)
            }}
          >
            {resumes(t) ? 'Resume' : 'Start again'}
          </Button>
        )}
      </div>
    </div>
  )
}

/** A session as one line (the ones closed lately). */
function SessionRow({ t, showProject }: { t: TerminalInfo; showProject: boolean }) {
  const a = t.agent
  const sub = isRunning(t) ? (a?.attention ?? a?.lastMessage ?? (a?.state === 'working' ? 'Working…' : '')) : 'Stopped'
  const color = colorCss(t.color)
  return (
    <div
      className="wb-list-row wb-ag-row"
      onClick={() => openTerminal(t)}
      onContextMenu={(e) => showMenu(e, terminalMenu(t))}
      title={t.title}
      style={color ? { boxShadow: `inset 3px 0 0 ${color}` } : undefined}
    >
      <StateDot t={t} />
      <div className="wb-grow">
        <div className="wb-ag-row-title">
          <ProviderBadge t={t} compact />
          <ContainerBadge t={t} compact />
          <span className="wb-ellipsis">{t.title}</span>
          <span className="wb-subtle wb-xs" style={{ marginLeft: 'auto', flex: 'none' }}>
            {showProject && t.projectId ? `${t.projectId} · ` : ''}
            <TimeAgo time={lastActivity(t)} />
          </span>
        </div>
        {sub && <div className="wb-ag-row-sub wb-ellipsis">{sub}</div>}
      </div>
    </div>
  )
}

/** Past conversations of the project, per provider (resume / fork). */
export function HistoryList({ projectId, onDone, limit = 12 }: { projectId: string; onDone?: () => void; limit?: number }) {
  const defaults = useAgentDefaults(projectId)
  const tabs = historyProviders(defaults.data?.providers ?? [])
  const remembered = useAgentsUi((s) => s.historyProvider)
  const setRemembered = useAgentsUi((s) => s.setHistoryProvider)
  const provider = tabs.find((p) => p.id === remembered) ?? tabs[0]
  return (
    <div className="wb-ag-history">
      {tabs.length > 1 && (
        <div className="wb-ag-htabs">
          <Tabs
            tabs={tabs.map((p) => ({
              id: p.id,
              label: (
                <span className="wb-ag-htab">
                  <ProviderIcon kind={p.kind} size={12} />
                  {displayLabel(p)}
                </span>
              ),
            }))}
            value={provider?.id ?? 'claude'}
            onChange={setRemembered}
          />
        </div>
      )}
      <ProviderHistory key={provider?.id ?? 'claude'} projectId={projectId} provider={provider} onDone={onDone} limit={limit} />
    </div>
  )
}

function ProviderHistory({ projectId, provider, onDone, limit }: { projectId: string; provider: ProviderInfo | undefined; onDone?: () => void; limit: number }) {
  const id = provider?.id ?? 'claude'
  const label = provider ? displayLabel(provider) : 'Claude Code'
  const { data, isLoading, error, refetch } = useAgentHistory(projectId, id)
  const [filter, setFilter] = useState('')
  const [shown, setShown] = useState(limit)
  const rows = useMemo(() => {
    const f = filter.trim().toLowerCase()
    return (data ?? []).filter(
      (h) => !f || h.title.toLowerCase().includes(f) || (h.firstPrompt ?? '').toLowerCase().includes(f) || (h.lastMessage ?? '').toLowerCase().includes(f) || h.id.startsWith(f),
    )
  }, [data, filter])
  const canStart = provider ? provider.available : true
  const resume = async (h: HistoryEntry, fork: boolean) => {
    const t = await startAgent({ projectId, provider: id, resume: h.id, fork })
    if (t) onDone?.()
  }
  if (isLoading) return <Loading label="Reading session history…" />
  if (error) return <ErrorBox error={error} onRetry={() => void refetch()} />
  if (!data?.length) {
    return (
      <EmptyState icon={History} title="No sessions yet">
        {label} conversations started in this project appear here.
      </EmptyState>
    )
  }
  return (
    <>
      <Input small placeholder={`Filter ${data.length} sessions…`} value={filter} onChange={(e) => setFilter(e.target.value)} />
      <div className="wb-ag-history-rows">
        {rows.slice(0, shown).map((h) => (
          <div key={h.id} className="wb-ag-hrow" onDoubleClick={() => void (h.terminalId ? openTerminal({ id: h.terminalId, title: h.title }) : resume(h, false))}>
            <div className="wb-grow">
              <div className="wb-ellipsis wb-ag-hrow-title" title={h.title}>
                {h.title}
              </div>
              <div className="wb-ag-hrow-sub wb-ellipsis">
                <TimeAgo time={h.lastActivity} />
                {h.gitBranch && (
                  <>
                    {' · '}
                    <GitBranch size={11} /> {h.gitBranch}
                  </>
                )}
                {h.sizeBytes > 0 && (
                  <>
                    {' · '}
                    {formatBytes(h.sizeBytes)}
                  </>
                )}
                {(h.firstPrompt ?? h.lastMessage) && <span className="wb-subtle"> · {h.firstPrompt ?? h.lastMessage}</span>}
              </div>
            </div>
            {h.terminalId && h.open ? (
              <Button size="small" icon={SquareTerminal} onClick={() => openTerminal({ id: h.terminalId!, title: h.title })}>
                Open
              </Button>
            ) : (
              <>
                <Button
                  size="small"
                  icon={Play}
                  disabled={!canStart}
                  onClick={() => void resume(h, false)}
                  title={canStart ? 'Resume this conversation' : (provider?.reason ?? undefined)}
                >
                  Resume
                </Button>
                {provider?.supports.fork !== false && (
                  <IconButton icon={GitFork} size="small" label="Fork: continue in a new session" disabled={!canStart} onClick={() => void resume(h, true)} />
                )}
              </>
            )}
          </div>
        ))}
        {rows.length > shown && (
          <button className="wb-ag-linkish wb-pad" onClick={() => setShown((n) => n + 30)}>
            Show {Math.min(30, rows.length - shown)} more…
          </button>
        )}
        {!rows.length && <div className="wb-muted wb-small wb-pad">No session matches “{filter}”.</div>}
      </div>
    </>
  )
}

/** Claude sessions running in other terminals on this machine. `enabled`: on screen (it polls). */
function ExternalList({ projectId, all, enabled }: { projectId: string | null; all: boolean; enabled: boolean }) {
  const { data, isLoading } = useExternalSessions(enabled)
  const list = (data ?? []).filter((s) => all || !projectId || s.projectId === projectId)
  if (isLoading) return <Loading />
  if (!list.length) return <div className="wb-muted wb-small wb-pad">No other Claude sessions are running{all ? '' : ' in this project'}.</div>
  return (
    <div>
      {list.map((s) => (
        <div key={s.pid} className="wb-ag-hrow">
          <span className={`wb-ag-dot ${s.status === 'busy' ? 'working' : 'idle'}`} title={s.status ?? ''} />
          <div className="wb-grow">
            <div className="wb-ellipsis wb-ag-hrow-title">{s.name ?? s.sessionId.slice(0, 8)}</div>
            <div className="wb-ag-hrow-sub wb-ellipsis mono">
              {contractHome(s.cwd)} · pid {s.pid}
              {s.status && ` · ${s.status}`}
            </div>
          </div>
          {s.remoteUrl && <RemoteLink url={s.remoteUrl} compact />}
          <IconButton icon={Hash} size="small" label="Copy session id" onClick={() => void copyText(s.sessionId, 'Session id copied')} />
        </div>
      ))}
    </div>
  )
}

function RemoteServers({ list }: { list: TerminalInfo[] }) {
  const openDialog = useAgentsUi((s) => s.openDialog)
  return (
    <div>
      {list.map((t) => {
        const urls = (Array.isArray(t.meta.urls) ? t.meta.urls : []) as string[]
        return (
          <div key={t.id} className="wb-ag-hrow">
            <span className={`wb-ag-dot ${isRunning(t) ? 'working' : 'exited'}`} />
            <div className="wb-grow">
              <div className="wb-ellipsis wb-ag-hrow-title">{t.title}</div>
              <div className="wb-ag-hrow-sub">
                {urls.length ? (
                  urls.map((u) => (
                    <span key={u} className="wb-ag-url">
                      <a href={u} target="_blank" rel="noopener noreferrer" onClick={(e) => (e.preventDefault(), openRemote(u))}>
                        {u.replace('https://', '')}
                      </a>
                      <IconButton icon={Copy} size="small" label="Copy link" onClick={() => void copyText(u, 'Link copied')} />
                    </span>
                  ))
                ) : (
                  <span className="wb-muted">{isRunning(t) ? 'Waiting for its claude.ai link…' : 'Stopped'}</span>
                )}
              </div>
            </div>
            <IconButton icon={SquareTerminal} size="small" label="Show its terminal" onClick={() => openTerminal(t)} />
            {isRunning(t) ? (
              <IconButton icon={Power} size="small" label="Stop the server" onClick={() => void killTerminal(t)} />
            ) : (
              <IconButton icon={Play} size="small" label="Start again" onClick={() => void restartTerminal(t)} />
            )}
          </div>
        )
      })}
      <div className="wb-pad">
        <Button size="small" icon={Radio} className="wb-ag-wrap-btn" title="Start a Remote Control server…" onClick={() => openDialog({ kind: 'remote' })}>
          <span className="wb-ellipsis">Start a Remote Control server…</span>
        </Button>
      </div>
    </div>
  )
}

/** `visible`: on screen (the column is open and this is its tab). */
export function AgentsHome({ visible = true }: { visible?: boolean }) {
  const projectId = useUi((s) => s.projectId)
  const { data: projects } = useProjects()
  const project = projects?.find((p) => p.id === projectId) ?? null
  const { data: terminals, isLoading, error } = useTerminals()
  const all = useAgentsUi((s) => s.allProjects)
  const setAll = useAgentsUi((s) => s.setAllProjects)
  const openDialog = useAgentsUi((s) => s.openDialog)
  const mine = agentSessions(terminals, projectId, all)
  const sessions = sortSessions(mine.filter((t) => t.open))
  const closed = mine
    .filter((t) => !t.open)
    .sort((a, b) => lastActivity(b) - lastActivity(a))
    .slice(0, 30)
  const c = counts(mine)
  const composerFocus = useAgentsUi((s) => s.composerFocus)
  const servers = (terminals ?? []).filter((t) => t.meta?.remoteControlServer && t.open && (all || t.projectId === projectId))

  return (
    <div className="wb-ag-home">
      <div className="wb-ag-home-inner">
        <div className="wb-ag-home-head">
          <Bot size={20} className="wb-ag-accent" />
          <h2>Agents</h2>
          <span className="wb-muted">{all ? 'all projects' : project?.name}</span>
          {c.working > 0 && (
            <span className="wb-ag-chip working">
              <Spinner size={10} /> {c.working} working
            </span>
          )}
          {c.attention > 0 && (
            <span className="wb-ag-chip attention">
              {c.attention} need{c.attention === 1 ? 's' : ''} you
            </span>
          )}
          <span style={{ flex: 1 }} />
          <Checkbox checked={all} onChange={setAll}>
            All projects
          </Checkbox>
          <Button size="small" icon={SquareTerminal} onClick={() => void newShell(projectId)} title="A shell in this project, as a tab of this column">
            New shell
          </Button>
          <Button size="small" icon={History} onClick={() => openDialog({ kind: 'resume' })} disabled={!projectId}>
            Resume…
          </Button>
        </div>

        <NewSessionForm projectId={projectId} focusToken={composerFocus} />

        <div className="wb-ag-section-title">
          Sessions <span className="wb-subtle">{sessions.length}</span>
        </div>
        {error ? (
          <ErrorBox error={error} />
        ) : isLoading ? (
          <Loading />
        ) : sessions.length ? (
          <div className="wb-ag-cards">
            {sessions.map((t) => (
              <SessionCard key={t.id} t={t} showProject={all} />
            ))}
          </div>
        ) : (
          <div className="wb-ag-empty-cards">
            <Bot size={22} />
            <span>No open sessions{all ? '' : ' in this project'}. Start one above, or resume one from the history.</span>
          </div>
        )}
        {closed.length > 0 && (
          <Section title="Recently closed" count={closed.length} defaultOpen={false}>
            {closed.map((t) => (
              <SessionRow key={t.id} t={t} showProject={all} />
            ))}
          </Section>
        )}

        <div className="wb-ag-home-cols">
          <div className="wb-ag-box">
            <div className="wb-ag-section-title">
              <History size={13} /> History
            </div>
            {projectId ? <HistoryList projectId={projectId} /> : <div className="wb-muted wb-pad">No project</div>}
          </div>
          <div className="wb-ag-box">
            <div className="wb-ag-section-title">
              <MonitorSmartphone size={13} /> Remote Control
            </div>
            <RemoteServers list={servers} />
            <div className="wb-ag-section-title">
              <ExternalLink size={13} /> Claude Code running elsewhere on this machine
            </div>
            <ExternalList projectId={projectId} all={all} enabled={visible} />
          </div>
        </div>
      </div>
    </div>
  )
}
