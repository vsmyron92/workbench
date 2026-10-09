// Global platform provider: server toasts (`ui.notify`), agent-driven panel
// opening (`ui.open`), settings refreshes, the reload after an update, browser notifications on remote
// devices, the pairing dialog, and the phone-app side of push: the service
// worker, keeping this device's push subscription in line with the server,
// presence reports, panels a notification or a manifest shortcut opens, and the
// window's theme colour.

import { useEffect, useRef, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useEvent } from '@/api/events'
import { FEATURES, unsupportedReason } from '@/api/health'
import { qk } from '@/api/queries'
import { knownRepos } from '@/api/repos'
import type { ProjectSummary } from '@/api/types'
import { getDockApi, openPanel, showToolWindow, toast, type ToastLevel } from '@/shell/actions'
import { panelDefs } from '@/shell/registry'
import { useUi } from '@/state/store'
import { pk, usePushInfo } from './api'
import { desktopNotifiesHere, panelIdFor, scopeUiOpen } from './lib'
import { PairDialogHost } from './PairDialog'
import { listenToWorker, openTarget, registerWorker, startPresence, syncPush, takeLaunch, type ProjectIds } from './push'
import { watchServerVersion } from './update'

const LEVELS = new Set<ToastLevel>(['info', 'success', 'warning', 'error'])

interface UiOpen {
  panel?: string
  params?: Record<string, unknown> | null
  title?: string | null
  id?: string
}

/** Open a panel an agent asked for. Only the visible tab acts; unknown kinds are ignored. */
export function handleUiOpen(d: UiOpen) {
  if (document.visibilityState !== 'visible') return
  const kind = d.panel
  if (!kind || !panelDefs[kind]) return
  const raw = d.params && typeof d.params === 'object' && !Array.isArray(d.params) ? d.params : {}
  // A git or CI panel of another repository of the project carries that repository in its scope.
  const { params, scoped } = scopeUiOpen(kind, raw, typeof raw.projectId === 'string' ? knownRepos(raw.projectId) : [])
  const title = d.title ?? undefined
  if (!getDockApi()) {
    // Phone layout: no panels. Offer web pages in a new browser tab instead.
    const url = typeof params.url === 'string' && /^https?:\/\//.test(params.url) ? params.url : null
    if (url) toast('info', `An agent opened ${title ?? url}`, { action: { label: 'Open', run: () => window.open(url, '_blank', 'noopener') } })
    return
  }
  openPanel({ kind, id: (!scoped && d.id) || panelIdFor(kind, params), title, params })
}

/** This device receives Web Push: the service worker notifies, not the page. */
let pushOnHere = false

/**
 * On a remote device (phone, another computer) the server's desktop
 * notifications are not visible, so use the browser's while the tab is hidden.
 * The same on the server's own computer when its OS has none (Windows).
 */
function browserNotify(title: string, body: string, tag?: string) {
  if (!document.hidden || pushOnHere || typeof Notification === 'undefined' || Notification.permission !== 'granted') return
  if (!useUi.getState().prefs.notifications || desktopNotifiesHere(location.hostname, unsupportedReason(FEATURES.desktopNotifications))) return
  try {
    const n = new Notification(title, { body, tag, icon: '/icons/icon-192.png' })
    n.onclick = () => {
      window.focus()
      n.close()
    }
  } catch {
    /* some mobile browsers only allow notifications from a service worker */
  }
}

/** The status bar colour of the installed app and the browser follows the theme (`--bg-panel`). */
function syncThemeColor() {
  const meta = document.querySelector<HTMLMetaElement>('meta[name="theme-color"]')
  const color = getComputedStyle(document.documentElement).getPropertyValue('--bg-panel').trim()
  if (meta && color) meta.content = color
}

function PushBridge() {
  const qc = useQueryClient()
  const push = usePushInfo()
  const info = push.data
  const projects = useRef<ProjectIds>(() => qc.getQueryData<ProjectSummary[]>(qk.projects)?.map((p) => p.id) ?? null)

  // The worker, what notifications open, and what the app was launched for.
  useEffect(() => {
    void registerWorker()
    const stop = listenToWorker(projects.current)
    const { tab, open } = takeLaunch()
    if (tab) {
      window.setTimeout(() => {
        if (!getDockApi()) return
        if (tab === 'agents') openPanel({ kind: 'agents.home', id: 'agents.home', title: 'Agents' })
        else showToolWindow('files')
      }, 300)
    }
    if (open) openTarget(open, projects.current)
    return stop
  }, [])

  // Keep this browser's subscription and the server's record in line.
  useEffect(() => {
    pushOnHere = !!info?.subscriptions.some((s) => s.current)
    if (!info) return
    void syncPush(info).then((action) => {
      if (action !== 'none') void qc.invalidateQueries({ queryKey: pk.push })
    })
  }, [info, qc])

  // Presence matters only once some device receives push.
  const anyPush = !!info?.subscriptions.length
  useEffect(() => (anyPush ? startPresence() : undefined), [anyPush])

  useEffect(() => {
    syncThemeColor()
    return useUi.subscribe((s, prev) => {
      if (s.prefs.theme !== prev.prefs.theme) syncThemeColor()
    })
  }, [])
  return null
}

export function PlatformProvider({ children }: { children?: ReactNode }) {
  const qc = useQueryClient()

  useEvent<{ level?: string; message?: string }>('ui.notify', (ev) => {
    const message = ev.data?.message
    if (!message) return
    const level = LEVELS.has(ev.data.level as ToastLevel) ? (ev.data.level as ToastLevel) : 'info'
    toast(level, message)
    browserNotify('Workbench', message)
    if (message.startsWith('Paired a new device')) void qc.invalidateQueries({ queryKey: pk.remote })
  })

  useEvent<UiOpen>('ui.open', (ev) => handleUiOpen(ev.data ?? {}))

  // A server that came back as another version (an update): reload, or offer to.
  useEffect(() => watchServerVersion(), [])

  useEvent('settings.changed', () => void qc.invalidateQueries({ queryKey: pk.all }))

  useEvent<{ terminalId?: string; title?: string; message?: string }>('agent.attention', (ev) => {
    browserNotify(ev.data?.title || 'Agent', ev.data?.message || 'needs your attention', ev.data?.terminalId)
  })

  return (
    <>
      {children}
      <PushBridge />
      <PairDialogHost />
    </>
  )
}
