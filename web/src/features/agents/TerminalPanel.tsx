// The 'terminal' panel: a header strip (agent state, title, model · effort · context ·
// cost, Remote Control link, actions), the attention strip, the terminal itself, and a
// banner with Resume/Restart once the process has exited.

import { useEffect, useRef } from 'react'
import { AlertTriangle, CircleStop, MoreHorizontal, OctagonX, Palette, Pencil, Play, Power, Rocket, RotateCcw, Search, ShieldQuestion, X } from 'lucide-react'
import { useTerminals } from '@/api/queries'
import type { TerminalInfo } from '@/api/types'
import { showToolWindow } from '@/shell/actions'
import type { PanelProps } from '@/shell/types'
import { Button, EmptyState, IconButton, Loading, showMenuAt, TimeAgo } from '@/ui'
import { closeTerminal, colorMenu, interrupt, killTerminal, ownerHint, renameTerminal, restartTerminal, terminalMenu } from './actions'
import { openTerminal, terminalsApi } from './api'
import { pendingOf } from './lib/permission'
import { dialogSeenOnScreen, resumes, SEEN_ON_SCREEN } from './lib/providers'
import { agentMeta, colorCss, isRunning, lingering, restartMode } from './lib/sessions'
import { ContainerBadge, ProviderBadge, RemoteLink, StateChip, StateDot } from './parts'
import { PermissionActions, PermissionDetail, PermissionRule, PermissionSummary } from './Permission'
import { dismissPermissionToasts } from './permissionToasts'
import { TerminalView, type TerminalViewHandle } from './TerminalView'

/** Acknowledge an unread answer after 600 ms of the terminal being visible in a focused window. */
export function useMarkSeen(t: TerminalInfo | undefined, visible: boolean) {
  const unread = !!t?.agent?.unread && t.status !== 'exited'
  const id = t?.id
  useEffect(() => {
    if (!id || !unread || !visible) return
    let timer: number | undefined
    const arm = () => {
      window.clearTimeout(timer)
      if (document.visibilityState === 'visible' && document.hasFocus()) {
        timer = window.setTimeout(() => void terminalsApi.seen(id).catch(() => {}), 600)
      }
    }
    const disarm = () => window.clearTimeout(timer)
    arm()
    window.addEventListener('focus', arm)
    window.addEventListener('blur', disarm)
    document.addEventListener('visibilitychange', arm)
    return () => {
      disarm()
      window.removeEventListener('focus', arm)
      window.removeEventListener('blur', disarm)
      document.removeEventListener('visibilitychange', arm)
    }
  }, [id, unread, visible])
}

/**
 * Shown under an exited terminal. `onReplaced` receives the new terminal when a run
 * configuration is started again (runs get a new terminal each time).
 */
export function ExitBanner({ t, onReplaced }: { t: TerminalInfo; onReplaced?: (next: TerminalInfo) => void }) {
  const e = t.exit
  const agent = t.kind === 'agent'
  const mode = restartMode(t)
  const left = lingering(t)
  const detail = e?.signal && e.signal !== 'Hangup' ? e.signal : e?.code !== null && e?.code !== undefined && e.code !== 0 && e.code !== 129 ? `code ${e.code}` : null
  return (
    <div className="wb-ag-exit">
      <CircleStop size={14} />
      <span className="wb-ellipsis">
        {agent ? 'Session stopped' : 'Process exited'}
        {detail && <span className="wb-muted"> · {detail}</span>}
        {e?.at ? (
          <span className="wb-subtle">
            {' '}
            · <TimeAgo time={e.at} />
          </span>
        ) : null}
        {left > 0 && (
          <span className="wb-warning">
            {' '}
            · {left} background process{left === 1 ? '' : 'es'} still running
          </span>
        )}
      </span>
      <span style={{ flex: 1 }} />
      {left > 0 && (
        <Button size="small" variant="danger" icon={Power} onClick={() => void killTerminal(t)} title="End what the process left running">
          Kill
        </Button>
      )}
      {mode === 'self' && (
        <Button size="small" variant="primary" icon={Play} onClick={() => void restartTerminal(t)}>
          {agent ? (resumes(t) ? 'Resume' : 'Start again') : 'Restart'}
        </Button>
      )}
      {mode === 'run' && (
        <Button size="small" variant="primary" icon={Play} onClick={() => void restartTerminal(t, onReplaced)}>
          Run again
        </Button>
      )}
      {mode === 'owner' && typeof t.meta?.env === 'string' && (
        <Button size="small" icon={Rocket} title={ownerHint(t)} onClick={() => showToolWindow('apps')}>
          Open environment
        </Button>
      )}
      {t.open && (
        <Button size="small" onClick={() => void closeTerminal(t)}>
          Close
        </Button>
      )}
    </div>
  )
}

function AgentHeader({ t, view }: { t: TerminalInfo; view: React.RefObject<TerminalViewHandle | null> }) {
  const a = t.agent!
  const running = isRunning(t)
  const meta = agentMeta(a)
  const color = colorCss(t.color)
  return (
    <div className="wb-ag-header" style={color ? { boxShadow: `inset 3px 0 0 ${color}` } : undefined}>
      <StateChip t={t} />
      <ProviderBadge t={t} />
      <ContainerBadge t={t} />
      <span className="wb-ag-header-title wb-ellipsis" title={t.title} onDoubleClick={() => void renameTerminal(t)}>
        {t.title}
      </span>
      {meta.length > 0 && <span className="wb-ag-header-meta wb-ellipsis">{meta.join(' · ')}</span>}
      {a.permissionMode && a.permissionMode !== 'default' && <span className="wb-badge">{a.permissionMode}</span>}
      <span style={{ flex: 1 }} />
      {a.remoteUrl && <RemoteLink url={a.remoteUrl} />}
      {running && (
        <IconButton
          icon={OctagonX}
          size="small"
          label="Interrupt (Esc)"
          onClick={() => {
            if (view.current) view.current.send('\x1b')
            else void interrupt(t)
          }}
        />
      )}
      <IconButton
        icon={running ? RotateCcw : Play}
        size="small"
        label={resumes(t) ? (running ? 'Restart (resume the session)' : 'Resume session') : running ? 'Restart (a new conversation)' : 'Start again'}
        onClick={() => void restartTerminal(t)}
      />
      <IconButton icon={Search} size="small" label="Find (Ctrl+F)" onClick={() => view.current?.openSearch()} />
      <IconButton icon={Pencil} size="small" label="Rename" onClick={() => void renameTerminal(t)} />
      <IconButton icon={Palette} size="small" label="Colour" onClick={(e) => showMenuAt(e.currentTarget, colorMenu(t))} />
      <IconButton icon={MoreHorizontal} size="small" label="More" onClick={(e) => showMenuAt(e.currentTarget, terminalMenu(t, { open: false }))} />
    </div>
  )
}

function PlainHeader({ t, view }: { t: TerminalInfo; view: React.RefObject<TerminalViewHandle | null> }) {
  const running = isRunning(t)
  const mode = restartMode(t)
  const restartLabel = mode === 'run' ? (running ? 'Restart the run configuration' : 'Run again') : running ? 'Restart' : 'Start again'
  return (
    <div className="wb-ag-header">
      <StateDot t={t} />
      <span className="wb-ag-header-title wb-ellipsis" title={t.argv.join(' ')}>
        {t.title}
      </span>
      <ContainerBadge t={t} />
      <span className="wb-ag-header-meta wb-ellipsis mono">{t.cwd}</span>
      <span style={{ flex: 1 }} />
      <IconButton icon={Search} size="small" label="Find (Ctrl+F)" onClick={() => view.current?.openSearch()} />
      {/* Deploys and env commands start again only from their environment (gates, confirmation). */}
      {mode !== 'owner' && <IconButton icon={running ? RotateCcw : Play} size="small" label={restartLabel} onClick={() => void restartTerminal(t)} />}
      <IconButton icon={MoreHorizontal} size="small" label="More" onClick={(e) => showMenuAt(e.currentTarget, terminalMenu(t, { open: false }))} />
    </div>
  )
}

/** A run configuration started again in a new terminal: show that one instead. */
function replacePanel(old: TerminalInfo, next: TerminalInfo, close: () => void) {
  openTerminal({ id: next.id, title: next.title })
  if (next.id !== old.id) close()
}

export function TerminalPanel({ params, visible, active, setTitle, close }: PanelProps<{ terminalId?: string }>) {
  const { data, isLoading } = useTerminals()
  const id = params.terminalId ?? ''
  const t = data?.find((x) => x.id === id)
  const view = useRef<TerminalViewHandle | null>(null)
  useMarkSeen(t, visible)
  // On screen, its request's Allow / Deny are right here: the toast is redundant.
  const pendingId = pendingOf(t)?.id
  useEffect(() => {
    if (visible && pendingId) dismissPermissionToasts(id)
  }, [visible, pendingId, id])

  // The host passes a new setTitle on every render; only react to the title itself.
  const setTitleRef = useRef(setTitle)
  setTitleRef.current = setTitle
  const title = t?.title
  useEffect(() => {
    if (title) setTitleRef.current(title)
  }, [title])

  if (isLoading && !data) return <Loading />
  if (!t) {
    return (
      <EmptyState icon={X} title="This terminal no longer exists" action={<Button onClick={close}>Close tab</Button>}>
        It was removed from Workbench's history.
      </EmptyState>
    )
  }
  const attention = t.agent?.attention && isRunning(t) ? t.agent.attention : null
  const pending = pendingOf(t)
  return (
    <div className="wb-ag-panel">
      {t.kind === 'agent' && t.agent ? <AgentHeader t={t} view={view} /> : <PlainHeader t={t} view={view} />}
      {pending ? (
        <div className="wb-ag-attention perm">
          <ShieldQuestion size={13} />
          <PermissionSummary p={pending} />
          <PermissionActions t={t} p={pending} />
          <div className="wb-ag-perm-extra">
            <PermissionDetail key={pending.id} p={pending} />
            <PermissionRule p={pending} />
          </div>
        </div>
      ) : (
        attention && (
          <div className={`wb-ag-attention ${t.agent?.state === 'error' ? 'error' : ''}`} title={dialogSeenOnScreen(t) ? SEEN_ON_SCREEN : undefined}>
            <AlertTriangle size={13} />
            <span className="wb-ellipsis">{attention}</span>
          </div>
        )
      )}
      <TerminalView ref={view} terminalId={t.id} visible={visible} autoFocus={active} />
      {!isRunning(t) && <ExitBanner t={t} onReplaced={(next) => replacePanel(t, next, close)} />}
    </div>
  )
}
