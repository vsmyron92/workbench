// Updates: looking for a newer release, installing it and restarting the server
// (server/src/platform/update), and noticing that the server this page talks to is
// another version than the one it was loaded from.

import type { QueryClient } from '@tanstack/react-query'
import { api } from '@/api/client'
import { onEventsConnection } from '@/api/events'
import { getHealth, refreshHealth } from '@/api/health'
import { qk } from '@/api/queries'
import type { TerminalInfo } from '@/api/types'
import { confirmDialog, toast, toastError } from '@/shell/actions'
import { pk } from './api'
import { restartImpact } from './lib'
import type { SettingsInfo, UpdateStatus } from './types'

/** This tab asked for the restart: it reloads by itself once the new version answers. */
let expectingRestart = false

/** Look for a newer release now. A look that fails comes back with `error` set. */
export async function checkForUpdates(qc: QueryClient): Promise<UpdateStatus | null> {
  try {
    const s = await api.post<UpdateStatus>('/api/platform/update/check')
    qc.setQueryData(pk.update, s)
    return s
  } catch (e) {
    toastError(e, 'Could not look for updates')
    return null
  }
}

/** What a restart stops, from the terminals and settings this page already has. */
function impact(qc: QueryClient): string {
  const terminals = qc.getQueryData<TerminalInfo[]>(qk.terminals) ?? []
  const running = terminals.filter((t) => t.status !== 'exited').map((t) => ({ kind: t.kind, working: t.agent?.state === 'working' }))
  const restore = qc.getQueryData<SettingsInfo>(pk.settings)?.config.agents.restore_on_start ?? true
  return restartImpact(running, restore)
}

/** Install `version` over the running Workbench and restart into it, after a confirmation. */
export async function installUpdate(qc: QueryClient, version: string) {
  const ok = await confirmDialog({
    title: `Update Workbench to ${version}?`,
    message: `Workbench downloads the release, checks it and restarts.\n\n${impact(qc)}\n\nThis page reloads when the new version is up.`,
    confirmLabel: 'Update and restart',
  })
  if (!ok) return
  try {
    expectingRestart = true
    qc.setQueryData(pk.update, await api.post<UpdateStatus>('/api/platform/update/install', { version, restart: true }))
  } catch (e) {
    expectingRestart = false
    toastError(e, 'Could not start the update')
  }
}

/** Restart the server (into the version installed on disk), after a confirmation. */
export async function restartWorkbench(qc: QueryClient, version?: string | null) {
  const ok = await confirmDialog({
    title: version ? `Restart Workbench into ${version}?` : 'Restart Workbench?',
    message: impact(qc),
    confirmLabel: 'Restart',
  })
  if (!ok) return
  try {
    expectingRestart = true
    await api.post('/api/platform/restart')
  } catch (e) {
    expectingRestart = false
    toastError(e, 'Could not restart Workbench')
  }
}

/**
 * Each time the event socket comes back, ask the server its version. When it is not the
 * one this page was loaded from, the page's code is the old version's (and its lazy
 * chunks are gone): the tab that asked for the restart reloads, the others offer to.
 * Returns a function that stops watching.
 */
export function watchServerVersion(reload: () => void = () => location.reload()): () => void {
  let loadedFrom = getHealth()?.version ?? null
  let startedAt = getHealth()?.startedAt ?? null
  let offered: string | null = null
  return onEventsConnection((up) => {
    if (!up) return
    void refreshHealth().then((h) => {
      if (!h) return
      // The same process again (the socket dropped, a download still runs): nothing happened yet.
      const restarted = startedAt !== null && h.startedAt !== startedAt
      startedAt = h.startedAt
      if (!loadedFrom) {
        loadedFrom = h.version
        return
      }
      if (h.version === loadedFrom) {
        if (restarted) expectingRestart = false
        return
      }
      if (expectingRestart) {
        reload()
      } else if (offered !== h.version) {
        offered = h.version
        toast('info', `Workbench was updated to ${h.version}`, {
          detail: 'Reload this page to use the new version.',
          action: { label: 'Reload', run: reload },
          timeout: 0,
        })
      }
    })
  })
}
