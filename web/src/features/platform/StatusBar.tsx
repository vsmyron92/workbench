// Status bar widgets: remote-access indicator and the settings gear.

import { AlertTriangle, MonitorSmartphone, Settings } from 'lucide-react'
import { openPanel } from '@/shell/actions'
import { useRemote } from './api'
import { restartText } from './lib'

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

export function SettingsButton() {
  return (
    <button className="wb-status-item" onClick={() => openSettings()} title="Settings (Ctrl+,)" aria-label="Settings">
      <Settings size={13} />
    </button>
  )
}
