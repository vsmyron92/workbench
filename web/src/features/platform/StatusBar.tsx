// Status bar widgets: the remote-access indicator and the update notice.

import { AlertTriangle, CircleArrowUp, MonitorSmartphone, RotateCw } from 'lucide-react'
import { openPanel } from '@/shell/actions'
import { useRemote, useUpdate } from './api'
import { restartText, updatePhaseText } from './lib'

export function openSettings(section?: string) {
  openPanel({ kind: 'settings', id: 'settings', title: 'Settings', params: section ? { section } : {} })
}

/** Shown when Workbench is reachable from other devices, or a restart is pending. */
export function RemoteIndicator() {
  const { data } = useRemote()
  if (!data) return null
  const restart = data.restartRequired
  return (
    <>
      {restart.length > 0 && (
        <button className="wb-status-item wb-warning" onClick={() => openSettings('remote')} title={`Restart to apply the new ${restartText(restart)}`}>
          <AlertTriangle size={13} /> Restart required
        </button>
      )}
      {data.exposed && (
        <button
          className="wb-status-item"
          onClick={() => openSettings('remote')}
          title={`Remote access on ${data.publicUrl ?? data.bind} · ${data.remoteDevices} paired device${data.remoteDevices === 1 ? '' : 's'}`}
        >
          <MonitorSmartphone size={13} /> Remote · {data.remoteDevices}
        </button>
      )}
    </>
  )
}

/** Shown while a newer release waits, an update runs, or an installed one waits for the restart. */
export function UpdateIndicator() {
  const { data } = useUpdate()
  if (!data) return null
  const open = () => openSettings('updates')
  const running = data.phase === 'checking' ? null : updatePhaseText(data.phase, data.progress, data.latest?.version)
  if (running) {
    return (
      <button className="wb-status-item" onClick={open} title={running}>
        <CircleArrowUp size={13} /> Updating…
      </button>
    )
  }
  if (data.restartPending && data.canRestart) {
    return (
      <button className="wb-status-item wb-warning" onClick={open} title={`Restart Workbench to use ${data.installed ?? 'the installed version'}`}>
        <RotateCw size={13} /> Restart to update
      </button>
    )
  }
  if (data.available && data.latest) {
    return (
      <button className="wb-status-item wb-status-update" onClick={open} title={`Workbench ${data.latest.version} is available (this is ${data.current})`}>
        <CircleArrowUp size={13} /> Update {data.latest.version}
      </button>
    )
  }
  return null
}
