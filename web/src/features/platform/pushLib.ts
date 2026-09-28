// Pure helpers for Web Push on this device (tested in pushLib.test.ts): support
// detection and its explanation, how the browser's subscription and the server's
// record are brought back in line, launch parameters and notification targets.

export type PushSupport =
  | { ok: true }
  | { ok: false; reason: 'insecure' | 'ios-browser' | 'unsupported'; message: string }

export interface SupportEnv {
  secure: boolean
  serviceWorker: boolean
  pushManager: boolean
  notification: boolean
  userAgent: string
  /** Running as an installed app (display-mode: standalone). */
  standalone: boolean
}

export function isIos(ua: string): boolean {
  return /iPhone|iPad|iPod/.test(ua) || (/Macintosh/.test(ua) && /Mobile\//.test(ua))
}

/** Whether this browser can receive push, and if not, what to do about it. */
export function pushSupport(env: SupportEnv): PushSupport {
  if (!env.secure) {
    return {
      ok: false,
      reason: 'insecure',
      message:
        'Push needs HTTPS. Open Workbench through an https address (tailscale serve, a reverse proxy or [server.tls]) and turn push on from there.',
    }
  }
  if (isIos(env.userAgent) && !env.standalone) {
    return {
      ok: false,
      reason: 'ios-browser',
      message: 'On iPhone and iPad, push works in the installed app: in Safari tap Share, then Add to Home Screen, open Workbench from the Home Screen and turn push on there (iOS 16.4 or later).',
    }
  }
  if (!env.serviceWorker || !env.pushManager || !env.notification) {
    return { ok: false, reason: 'unsupported', message: 'This browser does not support push notifications.' }
  }
  return { ok: true }
}

/** What the page found when it compared the browser with the server. */
export interface SyncState {
  permission: NotificationPermission
  /** The browser has a push subscription for this worker. */
  browserSub: boolean
  /** The browser subscription was made for the server's current key. */
  browserKeyMatches: boolean
  /** The server has a subscription for this device session… */
  serverHas: boolean
  /** …and it is the browser's current endpoint. */
  endpointMatches: boolean
  /** This browser turned push on (kept locally). */
  wanted: boolean
  /** The server key this browser subscribed with, when it turned push on. */
  keyAtSubscribe: string | null
  serverKey: string
}

/**
 * - `none`: consistent.
 * - `register`: send the browser's subscription to the server again (it changed, or
 *   the server lost it together with its key).
 * - `resubscribe`: make a new browser subscription for the server's key, then register it.
 * - `forget`: the server's record is useless (notifications blocked): remove it.
 * - `drop`: the server no longer has it (removed from another device, or the push
 *   service expired it): unsubscribe the browser too, so this device shows "off".
 */
export type SyncAction = 'none' | 'register' | 'resubscribe' | 'forget' | 'drop'

export function syncAction(s: SyncState): SyncAction {
  if (s.permission !== 'granted') return s.serverHas ? 'forget' : 'none'
  if (s.serverHas) {
    if (!s.browserSub || !s.browserKeyMatches) return 'resubscribe'
    return s.endpointMatches ? 'none' : 'register'
  }
  // The server made a new key (its data was reset): every device subscribes again.
  if (s.wanted && s.keyAtSubscribe && s.keyAtSubscribe !== s.serverKey) return 'resubscribe'
  return s.browserSub ? 'drop' : 'none'
}

/** base64url → bytes (the `applicationServerKey`). */
export function b64urlBytes(b64: string): Uint8Array<ArrayBuffer> {
  const s = b64.replace(/-/g, '+').replace(/_/g, '/')
  const raw = atob(s + '='.repeat((4 - (s.length % 4)) % 4))
  const out = new Uint8Array(new ArrayBuffer(raw.length))
  for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i)
  return out
}

export function bytesB64url(buf: ArrayBuffer | Uint8Array | null | undefined): string {
  if (!buf) return ''
  const bytes = buf instanceof Uint8Array ? buf : new Uint8Array(buf)
  let s = ''
  for (const b of bytes) s += String.fromCharCode(b)
  return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
}

/** A panel a notification (or `?open=`) asks for. */
export interface OpenTarget {
  kind?: string
  id?: string
  params?: Record<string, unknown>
  title?: string
  projectId?: string
}

/** Panel kinds a notification may open; anything else is ignored. */
const OPENABLE = new Set(['terminal', 'agents.home', 'pipeline', 'gh.run', 'settings'])

/** Validate a target from a notification or the URL (both reachable by other pages). */
export function parseTarget(raw: unknown): OpenTarget | null {
  let v = raw
  if (typeof v === 'string') {
    if (v.length > 2000) return null
    try {
      v = JSON.parse(v)
    } catch {
      return null
    }
  }
  if (!v || typeof v !== 'object' || Array.isArray(v)) return null
  const o = v as Record<string, unknown>
  const out: OpenTarget = {}
  if (typeof o.projectId === 'string' && /^[\w.-]{1,120}$/.test(o.projectId)) out.projectId = o.projectId
  if (typeof o.kind === 'string' && OPENABLE.has(o.kind)) {
    out.kind = o.kind
    if (typeof o.id === 'string' && o.id.length <= 300) out.id = o.id
    if (o.params && typeof o.params === 'object' && !Array.isArray(o.params)) out.params = o.params as Record<string, unknown>
    if (typeof o.title === 'string') out.title = o.title.slice(0, 120)
  }
  return out.kind || out.projectId ? out : null
}

/** Manifest shortcuts (`/?tab=agents`) and notification targets (`/?open=…`). */
export interface LaunchParams {
  tab: 'agents' | 'files' | null
  open: OpenTarget | null
  /** The URL without them (for history.replaceState), or null when unchanged. */
  cleaned: string | null
}

export function launchParams(href: string): LaunchParams {
  const url = new URL(href)
  const t = url.searchParams.get('tab')
  const o = url.searchParams.get('open')
  const tab = t === 'agents' || t === 'files' ? t : null
  const open = o ? parseTarget(o) : null
  if (!url.searchParams.has('tab') && !url.searchParams.has('open')) return { tab, open, cleaned: null }
  url.searchParams.delete('tab')
  url.searchParams.delete('open')
  return { tab, open, cleaned: url.pathname + url.search + url.hash }
}
