// Top bar: while the current project has a live debug session, a chip with its
// state (click: the Debug tool window) and a Stop button.

import { useMemo } from 'react'
import { Bug, Square } from 'lucide-react'
import { showToolWindow } from '@/shell/actions'
import { StatusDot } from '@/ui'
import { stopSession } from './actions'
import { isLive, stateLabel, stateTone } from './logic'
import { activeSession, sessionsOf, useDebug } from './store'

export function DebugStatus({ projectId }: { projectId: string | null }) {
  const sessions = useDebug((s) => s.sessions)
  const active = useDebug((s) => s.active)
  const s = useMemo(() => {
    const a = activeSession({ sessions, active }, projectId)
    return a && isLive(a) ? a : (sessionsOf(sessions, projectId).find(isLive) ?? null)
  }, [sessions, active, projectId])
  if (!s) return null
  const n = sessionsOf(sessions, projectId).filter(isLive).length
  return (
    <div className="wb-dbg-topbar" role="group" aria-label="Debug session">
      <button className="wb-topbar-widget" onClick={() => showToolWindow('debug', 'bottom')} title={`${s.name}: ${stateLabel(s)}${n > 1 ? `\n${n} debug sessions` : ''}`}>
        <Bug size={14} />
        <StatusDot tone={stateTone(s)} pulse={s.state === 'starting'} />
        <span className="wb-ellipsis">{s.name}</span>
        <span className="wb-muted wb-small">{stateLabel(s)}</span>
      </button>
      <button className="wb-icon-btn stop" title="Stop (Ctrl+F2)" aria-label={`Stop ${s.name}`} onClick={() => void stopSession(s)}>
        <Square size={14} />
      </button>
    </div>
  )
}
