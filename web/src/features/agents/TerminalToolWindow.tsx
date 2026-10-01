// The Terminal tool window (bottom, under the dock): shells started from it, as tabs,
// beside whatever the agents column shows. Its shells are its own (`bottomTerminals` in
// the store); the column leaves them out, and opening one of them anywhere shows it here.

import { useEffect, useMemo } from 'react'
import { Container, Laptop, Plus, SquareTerminal, X } from 'lucide-react'
import { useProjects, useTerminals } from '@/api/queries'
import type { TerminalInfo } from '@/api/types'
import { showToolWindow, toastError } from '@/shell/actions'
import { EmptyState, ErrorBox, IconButton, Loading, showMenu, showMenuAt } from '@/ui'
import { closeTerminal, terminalMenu } from './actions'
import { terminalsApi } from './api'
import { bottomTabs, isRunning, tabAfterClose } from './lib/sessions'
import { ContainerBadge, StateDot } from './parts'
import { updateCachedTerminal } from './queryAccess'
import { useAgentsUi } from './store'
import { ExitBanner } from './TerminalPanel'
import { TerminalView } from './TerminalView'

const NO_IDS: string[] = []

/** Start a login shell in the project as a tab of the Terminal tool window, and show it. */
export async function newBottomShell(projectId: string | null, container?: boolean): Promise<TerminalInfo | null> {
  try {
    const t = await terminalsApi.createShell(projectId, undefined, container)
    const ui = useAgentsUi.getState()
    ui.addBottomTerminal(projectId ?? '', t.id)
    ui.setBottomTab(projectId ?? '', t.id)
    updateCachedTerminal(t)
    showToolWindow('terminal', 'bottom')
    return t
  } catch (e) {
    toastError(e, 'Could not start a shell')
    return null
  }
}

/** Show a shell of the Terminal tool window (one of `bottomTerminals`) there. */
export function showBottomTerminal(projectId: string | null, terminalId: string) {
  useAgentsUi.getState().setBottomTab(projectId ?? '', terminalId)
  showToolWindow('terminal', 'bottom')
}

export function TerminalToolWindow({ projectId }: { projectId: string | null }) {
  const key = projectId ?? ''
  const { data, isLoading, error } = useTerminals()
  const ids = useAgentsUi((s) => s.bottomTerminals[key] ?? NO_IDS)
  const selected = useAgentsUi((s) => s.bottomTab[key] ?? null)
  const setSelected = useAgentsUi((s) => s.setBottomTab)
  const tabs = useMemo(() => bottomTabs(data, projectId, ids), [data, projectId, ids])
  const current = tabs.find((t) => t.id === selected) ?? tabs[tabs.length - 1]

  const projects = useProjects()
  const dc = projects.data?.find((p) => p.id === projectId)?.devcontainer
  /** With a running dev container: choose where (the default follows the project). */
  const plus = (el: HTMLElement) => {
    if (dc?.state !== 'running') return void newBottomShell(projectId)
    showMenuAt(el, [
      { label: `Container shell${dc.inContainer ? ' (default)' : ''}`, icon: Container, run: () => void newBottomShell(projectId, true) },
      { label: `Host shell${dc.inContainer ? '' : ' (default)'}`, icon: Laptop, run: () => void newBottomShell(projectId, false) },
    ])
  }

  /** A terminal started again under a new id (a run) takes the old tab's place. */
  const replaced = (old: TerminalInfo, next: TerminalInfo) => {
    const ui = useAgentsUi.getState()
    if (next.id !== old.id) {
      ui.removeBottomTerminal(key, old.id)
      ui.addBottomTerminal(key, next.id)
    }
    updateCachedTerminal(next)
    ui.setBottomTab(key, next.id)
  }
  const close = async (t: TerminalInfo) => {
    const next = tabAfterClose(tabs, t.id)
    if (!(await closeTerminal(t))) return
    const ui = useAgentsUi.getState()
    ui.removeBottomTerminal(key, t.id)
    if (ui.bottomTab[key] === t.id) ui.setBottomTab(key, next)
  }

  // Keep the selection valid when tabs come and go, and forget shells that are gone.
  useEffect(() => {
    if (current && current.id !== selected) setSelected(key, current.id)
  }, [key, current, selected, setSelected])
  useEffect(() => {
    if (!data) return
    const ui = useAgentsUi.getState()
    for (const id of ids) {
      const t = data.find((x) => x.id === id)
      if (!t || !t.open || t.projectId !== projectId) ui.removeBottomTerminal(key, id)
    }
  }, [data, ids, key, projectId])

  if (error) return <ErrorBox error={error} />
  if (isLoading) return <Loading />
  return (
    <div className="wb-fill">
      <div className="wb-ag-tabs" role="tablist" aria-label="Terminals">
        {tabs.map((t) => (
          <div
            key={t.id}
            role="tab"
            aria-selected={t.id === current?.id}
            className={t.id === current?.id ? 'wb-ag-tab active' : 'wb-ag-tab'}
            onClick={() => setSelected(key, t.id)}
            onAuxClick={(e) => e.button === 1 && void close(t)}
            onContextMenu={(e) => showMenu(e, terminalMenu(t, { open: false, onReplaced: (next) => replaced(t, next) }))}
            title={t.argv.join(' ')}
          >
            <StateDot t={t} size={7} />
            <ContainerBadge t={t} compact />
            <span className="wb-ellipsis">{t.title}</span>
            <button
              className="wb-ag-tab-close"
              aria-label="Close (stops the shell)"
              title="Close: stops the shell"
              onClick={(e) => {
                e.stopPropagation()
                void close(t)
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
          {!isRunning(current) && <ExitBanner t={current} onReplaced={(next) => replaced(current, next)} />}
        </div>
      ) : (
        <EmptyState
          icon={SquareTerminal}
          title="No terminals"
          action={
            <button className="wb-ag-linkish" onClick={() => void newBottomShell(projectId)} disabled={!projectId}>
              Open a shell
            </button>
          }
        >
          Shells you start here show under the editor, beside the agents column.
        </EmptyState>
      )}
    </div>
  )
}
