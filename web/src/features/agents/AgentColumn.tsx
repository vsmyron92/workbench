// The agents column (desktop): agent sessions and terminals as tabs, left of the
// workspace area. The first tab is the agents home (the prompt for a new session, the
// project's sessions, the history); "+" starts an agent session or a shell. Panels of
// kind `terminal` and `agents.home` open here, not in the dock, whoever asks: a run's
// output, a deploy, a container shell, "Ask agent" (shell/actions `setColumnHost`). The
// shells of the Terminal tool window (under the dock) are the exception: they stay there.

import { useEffect, useMemo, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Bot, Container, History, Laptop, Plus, Radio, SquareTerminal, X } from 'lucide-react'
import { qk, useProjects, useTerminals } from '@/api/queries'
import type { TerminalInfo } from '@/api/types'
import { setColumnHost, type ColumnHost } from '@/shell/actions'
import { useUi } from '@/state/store'
import { IconButton, Loading, showMenu, showMenuAt, type MenuEntry } from '@/ui'
import { closeTerminal, terminalMenu } from './actions'
import { AgentsHome } from './AgentsHome'
import { newShell } from './commands'
import { colorCss, columnTabs, counts, isRunning, tabAfterClose, tabPlace, terminalPanelId } from './lib/sessions'
import { ContainerBadge, ProviderBadge, StateDot } from './parts'
import { cachedProjectIds, cachedTerminals } from './queryAccess'
import { useAgentsUi } from './store'
import { TerminalPanel } from './TerminalPanel'
import { showBottomTerminal } from './TerminalToolWindow'

const NO_EXTRAS: string[] = []
/** Terminals kept mounted (their screens stay as they are when their tab is shown again). */
const MAX_MOUNTED = 8
/** How long a tab that was asked for may be missing from the terminals list (it was just created). */
const ARRIVAL_MS = 5000
const requested = new Map<string, number>()

const projectKey = () => useUi.getState().projectId ?? ''
const terminalOf = (panelId: string) => (panelId.startsWith('terminal:') ? panelId.slice('terminal:'.length) : null)
const noop = () => {}
/** A shell of the Terminal tool window (under the dock) of the current project. */
const isBottom = (id: string) => (useAgentsUi.getState().bottomTerminals[projectKey()] ?? NO_EXTRAS).includes(id)
/** The column's tabs of the current project, from the caches. */
const currentTabs = () => {
  const pid = useUi.getState().projectId
  const ui = useAgentsUi.getState()
  return columnTabs(cachedTerminals(), pid, ui.columnExtras[pid ?? ''], ui.bottomTerminals[pid ?? ''])
}

/**
 * Show a terminal as a tab of the column of the project it belongs to. Another project's
 * terminal is shown in that project's column (the UI switches to it when `focus`), never
 * grafted onto the current one, so switching projects never shows a mix. A terminal not
 * in the list yet is placed under the current project until its `terminal.created`
 * arrives (`ProjectColumn` moves it then).
 */
function showTerminal(id: string, focus: boolean) {
  const ui = useAgentsUi.getState()
  const current = useUi.getState().projectId
  const t = cachedTerminals()?.find((x) => x.id === id)
  const place = tabPlace(t, current, cachedProjectIds())
  const key = place.project ?? ''
  if (!t) requested.set(id, Date.now())
  if (place.extra) ui.addColumnExtra(key, id)
  if (!focus) return
  if (place.project !== current) useUi.getState().setProject(place.project)
  ui.selectColumnTab(key, id)
}

const host: ColumnHost = {
  open: (p) => {
    const key = projectKey()
    const ui = useAgentsUi.getState()
    if (p.kind === 'agents.home') {
      if (p.focus) ui.selectColumnTab(key, null)
      return
    }
    const id = typeof p.params.terminalId === 'string' ? p.params.terminalId : ''
    if (!id) return
    if (isBottom(id)) {
      if (p.focus) showBottomTerminal(useUi.getState().projectId, id)
      return
    }
    showTerminal(id, p.focus)
  },
  close: (panelId) => {
    const id = terminalOf(panelId)
    if (!id) return
    const ui = useAgentsUi.getState()
    ui.removeColumnExtra(projectKey(), id)
    ui.removeBottomTerminal(projectKey(), id)
  },
  isOpen: (panelId) => {
    const id = terminalOf(panelId)
    return !!id && (isBottom(id) || currentTabs().some((t) => t.id === id))
  },
}

function Tab({ t, active, onSelect, onClose }: { t: TerminalInfo; active: boolean; onSelect: () => void; onClose: () => void }) {
  const a = t.agent
  const running = isRunning(t)
  const color = colorCss(t.color)
  const waiting = running && !!a?.attention
  return (
    <div
      role="tab"
      aria-selected={active}
      className={['wb-ag-tab', active && 'active', waiting && 'attention'].filter(Boolean).join(' ')}
      style={color ? { boxShadow: `inset 0 2px 0 ${color}` } : undefined}
      title={t.kind === 'agent' ? t.title : `${t.title}\n${t.argv.join(' ')}`}
      onClick={onSelect}
      onAuxClick={(e) => e.button === 1 && onClose()}
      onContextMenu={(e) => showMenu(e, terminalMenu(t, { open: false }))}
    >
      <StateDot t={t} size={7} />
      {t.kind === 'agent' ? <ProviderBadge t={t} compact /> : <SquareTerminal size={12} className="wb-subtle" />}
      <ContainerBadge t={t} compact />
      <span className={a?.unread && running ? 'wb-ellipsis unread' : 'wb-ellipsis'}>{t.title}</span>
      <button
        className="wb-ag-tab-close"
        aria-label={t.open ? (t.kind === 'agent' ? 'Close (stops the session; resume it from the history)' : 'Close (stops the process)') : 'Close'}
        title={t.open ? (t.kind === 'agent' ? 'Close: stops the session; resume it from the history' : 'Close: stops the process') : 'Close'}
        onClick={(e) => {
          e.stopPropagation()
          onClose()
        }}
      >
        <X size={12} />
      </button>
    </div>
  )
}

/** One project's tabs. Keyed by the project, so switching projects starts from its own tabs. */
function ProjectColumn({ projectId }: { projectId: string | null }) {
  const key = projectId ?? ''
  const qc = useQueryClient()
  const { data } = useTerminals()
  const shown = true
  const selected = useAgentsUi((s) => s.columnTab[key] ?? null)
  const extras = useAgentsUi((s) => s.columnExtras[key] ?? NO_EXTRAS)
  // The Terminal tool window's shells are its tabs, not the column's.
  const bottom = useAgentsUi((s) => s.bottomTerminals[key] ?? NO_EXTRAS)
  const select = useAgentsUi((s) => s.selectColumnTab)
  const openDialog = useAgentsUi((s) => s.openDialog)
  const projects = useProjects()
  const dc = projects.data?.find((p) => p.id === projectId)?.devcontainer

  const tabs = useMemo(() => columnTabs(data, projectId, extras, bottom), [data, projectId, extras, bottom])
  const current = tabs.find((t) => t.id === selected) ?? null
  const currentId = current?.id ?? null
  // Asked for a moment ago and not in the list yet: its `terminal.created` is on the way.
  const [, setTick] = useState(0)
  const arriving = !!selected && !current && Date.now() - (requested.get(selected) ?? 0) < ARRIVAL_MS
  useEffect(() => {
    if (!arriving) return
    void qc.invalidateQueries({ queryKey: qk.terminals })
    const timer = window.setTimeout(() => setTick((n) => n + 1), ARRIVAL_MS)
    return () => window.clearTimeout(timer)
  }, [arriving, qc])

  // Extras that became the project's own open terminals are tabs anyway; ones that are gone
  // are forgotten; another project's terminal (asked for before it was listed, or left by an
  // earlier version) is shown in its own column instead: selected there when it was selected here.
  const projectIds = projects.data
  useEffect(() => {
    if (!data || !projectIds) return
    const ui = useAgentsUi.getState()
    const ids = projectIds.map((p) => p.id)
    for (const id of extras) {
      const t = data.find((x) => x.id === id)
      const gone = !t && Date.now() - (requested.get(id) ?? 0) >= ARRIVAL_MS
      if (gone) {
        ui.removeColumnExtra(key, id)
        requested.delete(id)
        continue
      }
      if (!t) continue
      const place = tabPlace(t, projectId, ids)
      if (place.project === projectId) {
        if (!place.extra) ui.removeColumnExtra(key, id)
        continue
      }
      ui.removeColumnExtra(key, id)
      if (ui.columnTab[key] !== id) continue
      select(key, null)
      showTerminal(id, true)
    }
  }, [data, extras, key, projectId, projectIds, select])

  const [mounted, setMounted] = useState<string[]>([])
  useEffect(() => {
    if (currentId) setMounted((m) => (m[m.length - 1] === currentId ? m : [...m.filter((x) => x !== currentId), currentId].slice(-MAX_MOUNTED)))
  }, [currentId])

  /** Take a tab away without stopping anything (a closed terminal's saved screen, a terminal of a project that is gone). */
  const drop = (id: string) => {
    if (useAgentsUi.getState().columnTab[key] === id) select(key, tabAfterClose(tabs, id))
    useAgentsUi.getState().removeColumnExtra(key, id)
  }
  const close = async (t: TerminalInfo) => {
    if (!(t.open && t.projectId === projectId)) return drop(t.id)
    const next = tabAfterClose(tabs, t.id)
    // The project's own tab is its terminal: closing it stops the process (asked first while an agent works).
    if ((await closeTerminal(t)) && useAgentsUi.getState().columnTab[key] === t.id) select(key, next)
  }

  const plus = (el: HTMLElement) => {
    const shells: MenuEntry[] =
      dc?.state === 'running'
        ? [
            { label: `New shell in the dev container${dc.inContainer ? ' (default)' : ''}`, icon: Container, run: () => void newShell(projectId, true) },
            { label: `New shell on the host${dc.inContainer ? '' : ' (default)'}`, icon: Laptop, run: () => void newShell(projectId, false) },
          ]
        : [{ label: 'New shell', icon: SquareTerminal, run: () => void newShell(projectId) }]
    showMenuAt(el, [
      {
        label: 'New agent session',
        icon: Bot,
        disabled: !projectId,
        run: () => {
          select(key, null)
          useAgentsUi.getState().focusComposer()
        },
      },
      ...shells,
      'separator',
      { label: 'Resume a session…', icon: History, disabled: !projectId, run: () => openDialog({ kind: 'resume' }) },
      { label: 'Start a Remote Control server…', icon: Radio, disabled: !projectId, run: () => openDialog({ kind: 'remote' }) },
    ])
  }

  const home = !current && !arriving
  const waiting = counts(data).attention
  return (
    <div className="wb-agcol">
      <div className="wb-agcol-tabs" role="tablist" aria-label="Agents and terminals">
        <button
          role="tab"
          aria-selected={home}
          className={home ? 'wb-agcol-home active' : 'wb-agcol-home'}
          title="Agents: start a session or a shell, this project's sessions, their history"
          onClick={() => select(key, null)}
        >
          <Bot size={14} />
          <span>Agents</span>
          {waiting > 0 && <span className="wb-ag-badge">{waiting}</span>}
        </button>
        {tabs.map((t) => (
          <Tab key={t.id} t={t} active={t.id === currentId} onSelect={() => select(key, t.id)} onClose={() => void close(t)} />
        ))}
        <IconButton icon={Plus} size="small" label="New agent session or shell" onClick={(e) => plus(e.currentTarget)} />
      </div>
      <div className="wb-agcol-body">
        {/* Stays mounted, so a prompt being written survives a look at a terminal. */}
        <div className="wb-agcol-pane" style={home ? undefined : { display: 'none' }}>
          <AgentsHome visible={shown && home} />
        </div>
        {arriving && <Loading label="Opening the terminal…" />}
        {tabs
          .filter((t) => t.id === currentId || mounted.includes(t.id))
          .map((t) => {
            const visible = shown && t.id === currentId
            return (
              <div key={t.id} className="wb-agcol-pane" style={t.id === currentId ? undefined : { display: 'none' }}>
                <TerminalPanel
                  id={terminalPanelId(t.id)}
                  params={{ terminalId: t.id }}
                  setParams={noop}
                  setTitle={noop}
                  close={() => drop(t.id)}
                  visible={visible}
                  active={visible}
                />
              </div>
            )
          })}
      </div>
    </div>
  )
}

export function AgentColumn({ projectId }: { projectId: string | null }) {
  useEffect(() => {
    setColumnHost(host)
    return () => setColumnHost(null)
  }, [])
  return <ProjectColumn key={projectId ?? ''} projectId={projectId} />
}
