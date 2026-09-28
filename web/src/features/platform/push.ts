// Web Push on this device: the service worker (/sw.js), this browser's push
// subscription, the values the worker needs in IndexedDB (the device key for the
// Allow / Deny actions, the server's key for re-subscribing), presence reports and
// the panels notifications open. Pure decisions live in pushLib.ts.

import { api, getDeviceKey } from '@/api/client'
import { getDockApi, isMobileShell, openPanel } from '@/shell/actions'
import { panelDefs } from '@/shell/registry'
import { useUi } from '@/state/store'
import { b64urlBytes, bytesB64url, launchParams, parseTarget, pushSupport, syncAction, type OpenTarget, type PushSupport, type SyncAction } from './pushLib'
import type { PushInfo, PushSubscriptionInfo, PushTopics } from './types'

// ---------------------------------------------------------------- IndexedDB (shared with sw.js)

const DB = 'workbench'
const STORE = 'kv'

function withStore<T>(mode: IDBTransactionMode, fn: (s: IDBObjectStore) => IDBRequest<T>): Promise<T | null> {
  return new Promise((resolve) => {
    let open: IDBOpenDBRequest
    try {
      open = indexedDB.open(DB, 1)
    } catch {
      resolve(null)
      return
    }
    open.onupgradeneeded = () => open.result.createObjectStore(STORE)
    open.onerror = () => resolve(null)
    open.onsuccess = () => {
      const db = open.result
      try {
        const req = fn(db.transaction(STORE, mode).objectStore(STORE))
        req.onsuccess = () => {
          resolve(req.result ?? null)
          db.close()
        }
        req.onerror = () => {
          resolve(null)
          db.close()
        }
      } catch {
        resolve(null)
        db.close()
      }
    }
  })
}

const idbSet = (key: string, value: string) => withStore('readwrite', (s) => s.put(value, key))
const idbDel = (key: string) => withStore('readwrite', (s) => s.delete(key))

/** What the worker needs to answer permission requests and to re-subscribe. */
async function storeWorkerKeys(serverKey: string) {
  const key = getDeviceKey()
  if (key) await idbSet('deviceKey', key)
  await idbSet('vapidKey', serverKey)
}

/** Forget them (push off, or signing out of this device). */
export async function clearWorkerKeys() {
  await idbDel('deviceKey')
  await idbDel('vapidKey')
}

// ---------------------------------------------------------------- support and the worker

export function currentSupport(): PushSupport {
  return pushSupport({
    secure: window.isSecureContext,
    serviceWorker: 'serviceWorker' in navigator,
    pushManager: 'PushManager' in window,
    notification: typeof Notification !== 'undefined',
    userAgent: navigator.userAgent,
    standalone: matchMedia('(display-mode: standalone)').matches || (navigator as { standalone?: boolean }).standalone === true,
  })
}

export function notificationPermission(): NotificationPermission {
  return typeof Notification === 'undefined' ? 'denied' : Notification.permission
}

let registration: Promise<ServiceWorkerRegistration | null> | null = null

/** Register /sw.js (scope /) once; null where service workers are unavailable. */
export function registerWorker(): Promise<ServiceWorkerRegistration | null> {
  if (!('serviceWorker' in navigator) || !window.isSecureContext) return Promise.resolve(null)
  registration ??= navigator.serviceWorker.register('/sw.js', { scope: '/', updateViaCache: 'none' }).catch((e: unknown) => {
    console.warn('Workbench: service worker registration failed', e)
    return null
  })
  return registration
}

async function browserSubscription(): Promise<PushSubscription | null> {
  const reg = await registerWorker()
  if (!reg || !('pushManager' in reg)) return null
  try {
    return await reg.pushManager.getSubscription()
  } catch {
    return null
  }
}

const keyOf = (sub: PushSubscription) => bytesB64url(sub.options.applicationServerKey)

async function endpointHash(endpoint: string): Promise<string> {
  const d = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(endpoint))
  return [...new Uint8Array(d)]
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('')
    .slice(0, 16)
}

// "This browser turned push on", with the server key it subscribed for.
const WANTED = 'wb.push.key'

function wantedKey(): string | null {
  try {
    return localStorage.getItem(WANTED)
  } catch {
    return null
  }
}

function setWanted(key: string | null) {
  try {
    if (key) localStorage.setItem(WANTED, key)
    else localStorage.removeItem(WANTED)
  } catch {
    /* storage blocked */
  }
}

// ---------------------------------------------------------------- on / off

async function subscribeBrowser(serverKey: string): Promise<PushSubscription> {
  const reg = await registerWorker()
  if (!reg) throw new Error('The service worker could not be registered.')
  await navigator.serviceWorker.ready
  let sub = await reg.pushManager.getSubscription()
  if (sub && keyOf(sub) !== serverKey) {
    await sub.unsubscribe().catch(() => false)
    sub = null
  }
  return sub ?? (await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: b64urlBytes(serverKey) }))
}

async function register(sub: PushSubscription, serverKey: string, extra?: { topics?: PushTopics; quietWhenActive?: boolean }) {
  await storeWorkerKeys(serverKey)
  const out = await api.post<PushSubscriptionInfo>('/api/push/subscriptions', { ...sub.toJSON(), ...extra })
  setWanted(serverKey)
  return out
}

/** While turning push on or off, the page does not also sync (it would see half the change). */
let busy = 0

async function exclusive<T>(fn: () => Promise<T>): Promise<T> {
  busy++
  try {
    return await fn()
  } finally {
    busy--
  }
}

/** Ask for permission, subscribe this browser and register it with Workbench. */
export function enablePush(info: PushInfo): Promise<PushSubscriptionInfo> {
  return exclusive(async () => {
    const support = currentSupport()
    if (!support.ok) throw new Error(support.message)
    const permission = await Notification.requestPermission()
    if (permission !== 'granted') {
      throw new Error(permission === 'denied' ? 'Notifications are blocked for this site in the browser settings.' : 'Notifications were not allowed.')
    }
    const sub = await subscribeBrowser(info.publicKey)
    return register(sub, info.publicKey)
  })
}

/** Remove this device's subscription (server and browser). */
export function disablePush(info: PushInfo | undefined): Promise<void> {
  return exclusive(async () => {
    const mine = info?.subscriptions.find((s) => s.current)
    if (mine) await api.del(`/api/push/subscriptions/${encodeURIComponent(mine.id)}`)
    const sub = await browserSubscription()
    await sub?.unsubscribe().catch(() => false)
    setWanted(null)
    await clearWorkerKeys()
  })
}

/** Bring the browser and the server back in line (see `syncAction`); returns what it did. */
export async function syncPush(info: PushInfo): Promise<SyncAction> {
  if (busy || !currentSupport().ok) return 'none'
  return exclusive(async () => {
    const sub = await browserSubscription()
    const mine = info.subscriptions.find((s) => s.current)
    const keep = mine && { topics: mine.topics, quietWhenActive: mine.quietWhenActive }
    const action = syncAction({
      permission: notificationPermission(),
      browserSub: !!sub,
      browserKeyMatches: !!sub && keyOf(sub) === info.publicKey,
      serverHas: !!mine,
      endpointMatches: !!sub && !!mine && (await endpointHash(sub.endpoint)) === mine.endpointHash,
      wanted: wantedKey() !== null,
      keyAtSubscribe: wantedKey(),
      serverKey: info.publicKey,
    })
    try {
      switch (action) {
        case 'none':
          // Keep the worker's copy of the device key current (it changes on a new sign-in).
          if (mine) await storeWorkerKeys(info.publicKey)
          break
        case 'register':
          if (sub) await register(sub, info.publicKey, keep)
          break
        case 'resubscribe':
          await register(await subscribeBrowser(info.publicKey), info.publicKey, keep)
          break
        case 'forget':
          if (mine) await api.del(`/api/push/subscriptions/${encodeURIComponent(mine.id)}`)
          setWanted(null)
          break
        case 'drop':
          await sub?.unsubscribe().catch(() => false)
          setWanted(null)
          await clearWorkerKeys()
          break
      }
    } catch (e) {
      console.warn('Workbench: push sync failed', e)
    }
    return action
  })
}

// ---------------------------------------------------------------- presence

const ACTIVE_MS = 120_000
const PRESENCE_EVERY_MS = 30_000

/** This page's own id for presence reports: every tab of a browser shares its device session. */
function newTabId(): string {
  const bytes = new Uint8Array(12)
  crypto.getRandomValues(bytes)
  return bytesB64url(bytes)
}

/**
 * Report whether this page is on screen (and used recently), so a device looking at
 * Workbench gets no push and, when a device asks for it, is left alone while another
 * one is in active use. Each page reports with its own tab id, so one tab going hidden
 * does not hide another that is on screen. Returns the cleanup.
 */
export function startPresence(): () => void {
  const tab = newTabId()
  let lastInput = Date.now()
  const onInput = () => {
    lastInput = Date.now()
  }
  const report = (visible = document.visibilityState === 'visible') => {
    const key = getDeviceKey()
    const headers: Record<string, string> = { 'Content-Type': 'application/json' }
    if (key) headers['X-Workbench-Key'] = key
    void fetch('/api/push/presence', {
      method: 'POST',
      credentials: 'same-origin',
      keepalive: true,
      headers,
      body: JSON.stringify({ visible, active: visible && Date.now() - lastInput < ACTIVE_MS, tab }),
    }).catch(() => {})
  }
  const onVisibility = () => report()
  const onHide = () => report(false)
  for (const t of ['pointerdown', 'keydown', 'wheel', 'touchstart'] as const) window.addEventListener(t, onInput, { capture: true, passive: true })
  document.addEventListener('visibilitychange', onVisibility)
  window.addEventListener('pagehide', onHide)
  const timer = window.setInterval(() => {
    if (document.visibilityState === 'visible') report()
  }, PRESENCE_EVERY_MS)
  report()
  return () => {
    for (const t of ['pointerdown', 'keydown', 'wheel', 'touchstart'] as const) window.removeEventListener(t, onInput, { capture: true })
    document.removeEventListener('visibilitychange', onVisibility)
    window.removeEventListener('pagehide', onHide)
    window.clearInterval(timer)
    report(false)
  }
}

// ---------------------------------------------------------------- opening what a notification points at

/** The project ids, or null while they are not loaded yet. */
export type ProjectIds = () => string[] | null

/** Run `fn` once a shell (dock or phone tabs) can take panels and the projects are known. */
function whenReady(projects: ProjectIds, fn: () => void, tries = 100) {
  if (((getDockApi() || isMobileShell()) && projects() !== null) || tries <= 0) fn()
  else window.setTimeout(() => whenReady(projects, fn, tries - 1), 100)
}

/** Switch to the target's project and open its panel (only known projects and panel kinds). */
export function openTarget(t: OpenTarget, projects: ProjectIds) {
  whenReady(projects, () => {
    if (t.projectId && projects()?.includes(t.projectId)) useUi.getState().setProject(t.projectId)
    if (t.kind && panelDefs[t.kind]) openPanel({ kind: t.kind, id: t.id, params: t.params ?? {}, title: t.title })
  })
}

/** `workbench:open` messages from the worker (a notification tapped while Workbench was open). */
export function listenToWorker(hasProject: ProjectIds): () => void {
  if (!('serviceWorker' in navigator)) return () => {}
  const onMessage = (e: MessageEvent) => {
    const d = e.data as { type?: string; target?: unknown } | null
    if (d?.type !== 'workbench:open') return
    const t = parseTarget(d.target)
    if (t) openTarget(t, hasProject)
  }
  navigator.serviceWorker.addEventListener('message', onMessage)
  return () => navigator.serviceWorker.removeEventListener('message', onMessage)
}

// Launch parameters, taken once when the app loads: manifest shortcuts
// (`/?tab=agents`, `/?tab=files`) and notifications opening a new window
// (`/?open={…}`). The phone shell reads its saved tab when it mounts.
const launch = typeof location === 'undefined' ? null : launchParams(location.href)
if (launch?.cleaned) history.replaceState(history.state, '', launch.cleaned)
if (launch?.tab) {
  try {
    sessionStorage.setItem('wb.mobile.tab', launch.tab)
  } catch {
    /* private mode */
  }
}

/** The launch target and tool window, once (desktop shows the tab's tool window). */
export function takeLaunch(): { tab: 'agents' | 'files' | null; open: OpenTarget | null } {
  const out = { tab: launch?.tab ?? null, open: launch?.open ?? null }
  if (launch) {
    launch.tab = null
    launch.open = null
  }
  return out
}
