// Tool windows: 'agents' (left: compact session list) and 'terminal' (bottom: the
// project's shells, runs and commands as tabs).

import { useEffect } from 'react'
import { Bot, Container, Globe, Laptop, Plus, SquareTerminal, X } from 'lucide-react'
import { useProjects, useTerminals } from '@/api/queries'
import type { TerminalInfo } from '@/api/types'
import { toastError } from '@/shell/actions'
import { EmptyState, ErrorBox, IconButton, Loading, Section, showMenu, showMenuAt, TimeAgo, Toolbar } from '@/ui'
import { closeTerminal, terminalMenu } from './actions'
import { openAgentsHome, openTerminal, terminalsApi } from './api'
import { agentSessions, colorCss, counts, isRunning, lastActivity, sortSessions } from './lib/sessions'
import { ContainerBadge, ProviderBadge, StateDot } from './parts'
import { useAgentsUi } from './store'
import { ExitBanner } from './TerminalPanel'
import { TerminalView } from './TerminalView'

function AgentRow({ t, showProject }: { t: TerminalInfo; showProject: boolean }) {
  const a = t.agent
  const sub = isRunning(t) ? (a?.attention ?? a?.lastMessage ?? (a?.state === 'working' ? 'Working…' : '')) : 'Stopped'
  const color = colorCss(t.color)
  return (
    <div
      className={`wb-list-row wb-ag-row ${a?.attention && isRunning(t) ? 'attention' : ''}`}
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
          <span className={`wb-ellipsis ${a?.unread && isRunning(t) ? 'unread' : ''}`}>{t.title}</span>
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

export function AgentsToolWindow({ projectId }: { projectId: string | null }) {
  const { data, isLoading, error } = useTerminals()
  const all = useAgentsUi((s) => s.allProjects)
  const setAll = useAgentsUi((s) => s.setAllProjects)
  const openDialog = useAgentsUi((s) => s.openDialog)
  const mine = agentSessions(data, projectId, all)
  const open = sortSessions(mine.filter((t) => t.open))
  const closed = mine.filter((t) => !t.open).sort((a, b) => lastActivity(b) - lastActivity(a)).slice(0, 30)
  return (
    <div className="wb-fill">
      <Toolbar>
        <IconButton icon={Plus} size="small" label="New session (Ctrl+Shift+A)" onClick={() => openDialog({ kind: 'new' })} />
        <IconButton icon={Bot} size="small" label="Agents home" onClick={() => openAgentsHome()} />
        <span style={{ flex: 1 }} />
        <IconButton icon={Globe} size="small" label={all ? 'Showing all projects' : 'Show all projects'} active={all} onClick={() => setAll(!all)} />
      </Toolbar>
      <div className="wb-scroll">
        {error ? (
          <ErrorBox error={error} />
        ) : isLoading ? (
          <Loading />
        ) : !open.length && !closed.length ? (
          <EmptyState icon={Bot} title="No agent sessions" action={<button className="wb-ag-linkish" onClick={() => openDialog({ kind: 'new' })}>Start a session</button>}>
            Agent sessions you start (Claude Code, Codex, Kimi…) appear here.
          </EmptyState>
        ) : (
          <>
            <Section title="Open" count={open.length}>
              {open.map((t) => (
                <AgentRow key={t.id} t={t} showProject={all} />
              ))}
              {!open.length && <div className="wb-muted wb-small wb-pad">No open sessions.</div>}
            </Section>
            {closed.length > 0 && (
              <Section title="Recently closed" count={closed.length} defaultOpen={false}>
                {closed.map((t) => (
                  <AgentRow key={t.id} t={t} showProject={all} />
                ))}
              </Section>
            )}
          </>
        )}
      </div>
    </div>
  )
}

export function AgentsBadge() {
  const { data } = useTerminals()
  const n = counts(data).attention
  return n ? <span className="wb-ag-badge">{n}</span> : null
}

/** Bottom tool window: non-agent terminals of the project as tabs. */
export function TerminalToolWindow({ projectId }: { projectId: string | null }) {
  const { data, isLoading, error } = useTerminals()
  const tabs = (data ?? []).filter((t) => t.kind !== 'agent' && t.open && t.projectId === projectId)
  const selected = useAgentsUi((s) => (projectId ? s.bottomTab[projectId] : undefined))
  const setSelected = useAgentsUi((s) => s.setBottomTab)
  const current = tabs.find((t) => t.id === selected) ?? tabs[tabs.length - 1]
  /** A run started again gets a new terminal: switch to it. */
  const select = (next: TerminalInfo) => {
    if (projectId) setSelected(projectId, next.id)
  }

  const projects = useProjects()
  const dc = projects.data?.find((p) => p.id === projectId)?.devcontainer
  const newShell = async (container?: boolean) => {
    try {
      const t = await terminalsApi.createShell(projectId, undefined, container)
      if (projectId) setSelected(projectId, t.id)
    } catch (e) {
      toastError(e, 'Could not start a shell')
    }
  }
  /** With a running dev container: choose where (the default follows the project). */
  const plus = (el: HTMLElement) => {
    if (dc?.state !== 'running') return void newShell()
    showMenuAt(el, [
      { label: `Container shell${dc.inContainer ? ' (default)' : ''}`, icon: Container, run: () => void newShell(true) },
      { label: `Host shell${dc.inContainer ? '' : ' (default)'}`, icon: Laptop, run: () => void newShell(false) },
    ])
  }

  // Keep the selection valid when tabs come and go.
  useEffect(() => {
    if (projectId && current && current.id !== selected) setSelected(projectId, current.id)
  }, [projectId, current, selected, setSelected])

  if (error) return <ErrorBox error={error} />
  if (isLoading) return <Loading />
  return (
    <div className="wb-fill">
      <div className="wb-ag-tabs">
        {tabs.map((t) => (
          <div
            key={t.id}
            className={t.id === current?.id ? 'wb-ag-tab active' : 'wb-ag-tab'}
            onClick={() => projectId && setSelected(projectId, t.id)}
            onAuxClick={(e) => e.button === 1 && void closeTerminal(t)}
            onContextMenu={(e) => showMenu(e, terminalMenu(t, { open: false, onReplaced: select }))}
            title={t.argv.join(' ')}
          >
            <StateDot t={t} size={7} />
            <ContainerBadge t={t} compact />
            <span className="wb-ellipsis">{t.title}</span>
            <button
              className="wb-ag-tab-close"
              aria-label="Close"
              onClick={(e) => {
                e.stopPropagation()
                void closeTerminal(t)
              }}
            >
              <X size={12} />
            </button>
          </div>
        ))}
        <IconButton
          icon={Plus}
          size="small"
          label={dc?.state === 'running' ? 'New shell (in the dev container or on the host)' : 'New shell'}
          onClick={(e) => plus(e.currentTarget)}
          disabled={!projectId}
        />
      </div>
      {current ? (
        <div className="wb-ag-bottom-body">
          <TerminalView key={current.id} terminalId={current.id} visible autoFocus />
          {!isRunning(current) && <ExitBanner t={current} onReplaced={select} />}
        </div>
      ) : (
        <EmptyState icon={SquareTerminal} title="No terminals" action={<button className="wb-ag-linkish" onClick={() => void newShell()} disabled={!projectId}>Open a shell</button>}>
          Shells, run configurations and commands of this project show here.
        </EmptyState>
      )}
    </div>
  )
}
