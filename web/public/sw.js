// Workbench service worker (scope /): Web Push notifications, their Allow / Deny
// actions for agent permission requests, and opening Workbench from a notification.
//
// It caches nothing and has no fetch handler: every request goes to the network
// exactly as without a worker, so an update never serves a stale page, and /api
// and /view responses are never stored.
//
// The page keeps two values in IndexedDB (`workbench` / `kv`) for this worker:
// `deviceKey` (the device key writes need, see api/client.ts) and `vapidKey` (the
// server's application server key, to re-subscribe on `pushsubscriptionchange`).

const DB = 'workbench'
const STORE = 'kv'
const ICON = '/icons/icon-192.png'
const BADGE = '/icons/badge-96.png'
/** How long "Allowed" / "Denied" stays before it clears itself. */
const CONFIRM_MS = 4000

self.addEventListener('install', () => {
  self.skipWaiting()
})

self.addEventListener('activate', (event) => {
  event.waitUntil(self.clients.claim())
})

function idbGet(key) {
  return new Promise((resolve) => {
    let req
    try {
      req = indexedDB.open(DB, 1)
    } catch {
      resolve(null)
      return
    }
    req.onupgradeneeded = () => req.result.createObjectStore(STORE)
    req.onerror = () => resolve(null)
    req.onsuccess = () => {
      const db = req.result
      try {
        const get = db.transaction(STORE, 'readonly').objectStore(STORE).get(key)
        get.onsuccess = () => {
          resolve(get.result ?? null)
          db.close()
        }
        get.onerror = () => {
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

/** Safari revokes a subscription whose pushes show nothing, even with the app open. */
function isWebKit() {
  const ua = self.navigator.userAgent
  return /Safari\//.test(ua) && !/Chrome\/|Chromium\/|Edg\//.test(ua)
}

async function windows() {
  return self.clients.matchAll({ type: 'window', includeUncontrolled: true })
}

self.addEventListener('push', (event) => {
  let data
  try {
    data = event.data ? event.data.json() : {}
  } catch {
    data = { title: 'Workbench', body: event.data ? event.data.text() : '' }
  }
  if (!data || typeof data !== 'object') data = {}
  event.waitUntil(show(data))
})

function options(data) {
  const permission = !!(data.permissionId && data.terminalId)
  const o = {
    body: typeof data.body === 'string' ? data.body : '',
    icon: ICON,
    badge: BADGE,
    timestamp: typeof data.ts === 'number' ? data.ts : Date.now(),
    requireInteraction: permission,
    data,
  }
  if (typeof data.tag === 'string' && data.tag) {
    o.tag = data.tag
    o.renotify = data.renotify !== false
  }
  if (permission) {
    // Allow only for a request the notification shows whole (nothing cut or masked,
    // short enough: the server's `allow`, the attention toast's rule); otherwise
    // Review (open the session to read it) and Deny.
    o.actions =
      data.allow === true
        ? [
            { action: 'allow', title: 'Allow' },
            { action: 'deny', title: 'Deny' },
          ]
        : [
            { action: 'review', title: 'Review' },
            { action: 'deny', title: 'Deny' },
          ]
  }
  return o
}

async function show(data) {
  // Workbench is on screen: it shows the news itself (the same rule as the server's
  // presence check; Chrome needs no notification for a push while the site is
  // visible, Safari does).
  if (!isWebKit()) {
    const open = await windows()
    if (open.some((c) => c.visibilityState === 'visible')) return
  }
  await self.registration.showNotification(typeof data.title === 'string' && data.title ? data.title : 'Workbench', options(data))
}

self.addEventListener('notificationclick', (event) => {
  const n = event.notification
  const data = n.data || {}
  n.close()
  const done = event.action === 'allow' || event.action === 'deny' ? answer(data, event.action) : focusOrOpen(data)
  try {
    event.waitUntil(done)
  } catch {
    // A synthetic event (tests) cannot be extended; the work still runs.
  }
})

/** Answer an agent's permission request from the notification. */
async function answer(data, decision) {
  if (!data.terminalId || !data.permissionId) return focusOrOpen(data)
  // Never allow what the notification could not show whole.
  if (decision === 'allow' && data.allow !== true) return focusOrOpen(data)
  const key = await idbGet('deviceKey')
  const headers = { 'Content-Type': 'application/json' }
  if (key) headers['X-Workbench-Key'] = key
  let res
  try {
    res = await fetch(`/api/agents/${encodeURIComponent(data.terminalId)}/permission`, {
      method: 'POST',
      credentials: 'same-origin',
      headers,
      body: JSON.stringify({ id: data.permissionId, decision }),
    })
  } catch {
    return retryLater(data, 'Could not reach Workbench.')
  }
  if (res.ok) return confirm(data, decision === 'allow' ? 'Allowed' : 'Denied')
  // Signed out on this device (or the key is gone): open Workbench to sign in and answer there.
  if (res.status === 401 || res.status === 403) return focusOrOpen(data)
  // Workbench can no longer answer it: it was answered in the terminal, the session
  // moved on, or Workbench's wait ran out while the terminal still asks. Say so and
  // stay until tapped, which opens the session: it may still wait at its prompt.
  if (res.status === 409) {
    return settled(data, 'No longer pending here', 'It was answered in the terminal, or only the terminal can answer it now. Tap to check the session.')
  }
  if (res.status === 404) {
    return settled(data, 'This session is gone', 'Tap to open Workbench.', { kind: 'agents.home', id: 'agents.home', params: {} })
  }
  // Worth another try: Workbench restarting, overloaded, or unreachable.
  if (res.status === 429 || res.status >= 500) return retryLater(data, `Workbench answered ${res.status}.`)
  return settled(data, 'Not answered', `Workbench refused the answer (HTTP ${res.status}). Tap to answer in Workbench.`)
}

/**
 * The request cannot be answered from the notification (any more): a notification
 * without actions that stays until dismissed; a tap opens `open` (default: the session).
 */
async function settled(data, what, detail, open) {
  const title = typeof data.title === 'string' && data.title ? data.title : 'Workbench'
  await self.registration.showNotification(title, {
    body: `${data.tool ? `${what} · ${data.tool}` : what}\n${detail}`,
    icon: ICON,
    badge: BADGE,
    tag: data.tag || undefined,
    renotify: false,
    data: { open: open || data.open, projectId: data.projectId, tag: data.tag },
  })
}

/** A quiet notification in place of the request that clears itself. */
async function confirm(data, text) {
  const mark = `${Date.now()}-${Math.random()}`
  const title = typeof data.title === 'string' && data.title ? data.title : 'Workbench'
  const body = data.tool ? `${text} · ${data.tool}` : text
  await self.registration.showNotification(title, {
    body,
    icon: ICON,
    badge: BADGE,
    tag: data.tag || undefined,
    silent: true,
    data: { open: data.open, projectId: data.projectId, tag: data.tag, confirmation: mark },
  })
  await new Promise((r) => setTimeout(r, CONFIRM_MS))
  const shown = await self.registration.getNotifications(data.tag ? { tag: data.tag } : undefined)
  for (const n of shown) if (n.data && n.data.confirmation === mark) n.close()
}

/** Show the request again, with its actions, and why answering failed. */
async function retryLater(data, why) {
  const o = options(data)
  o.body = `${why} Try again, or open Workbench.\n${o.body}`
  o.renotify = false
  await self.registration.showNotification(typeof data.title === 'string' && data.title ? data.title : 'Workbench', o)
}

/** Focus a Workbench window and open the notification's target there, or open one. */
async function focusOrOpen(data) {
  const target = data.open && typeof data.open === 'object' ? { ...data.open, projectId: data.projectId } : data.projectId ? { projectId: data.projectId } : null
  const list = await windows()
  const client = list.find((c) => c.focused) || list.find((c) => c.visibilityState === 'visible') || list[0]
  if (client) {
    try {
      await client.focus()
    } catch {
      /* not allowed outside a click */
    }
    if (target) client.postMessage({ type: 'workbench:open', target })
    return
  }
  const url = target ? `/?open=${encodeURIComponent(JSON.stringify(target))}` : '/'
  await self.clients.openWindow(url)
}

function keyBytes(b64) {
  const s = b64.replace(/-/g, '+').replace(/_/g, '/')
  const raw = atob(s + '==='.slice((s.length + 3) % 4))
  return Uint8Array.from(raw, (c) => c.charCodeAt(0))
}

// The browser replaced the subscription (expired or rotated): subscribe again and
// tell Workbench, which keeps this device's topics.
self.addEventListener('pushsubscriptionchange', (event) => {
  event.waitUntil(
    (async () => {
      const old = event.oldSubscription
      let sub = event.newSubscription
      if (!sub) {
        const vapid = await idbGet('vapidKey')
        const appKey = (old && old.options && old.options.applicationServerKey) || (vapid ? keyBytes(vapid) : null)
        if (!appKey) return
        sub = await self.registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: appKey })
      }
      const key = await idbGet('deviceKey')
      const headers = { 'Content-Type': 'application/json' }
      if (key) headers['X-Workbench-Key'] = key
      await fetch('/api/push/subscriptions', { method: 'POST', credentials: 'same-origin', headers, body: JSON.stringify(sub.toJSON()) })
    })().catch(() => {}),
  )
})
