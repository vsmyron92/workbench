// Thin fetch wrapper. Same origin, cookie auth; errors become ApiError with the
// server's `{error:{code,message}}` payload. A 401 flips the app to the login screen.
//
// Device key: browsers send the session cookie to every port on 127.0.0.1, so the
// cookie alone only lets a request read. Writes and WebSockets also carry this
// browser's device key, which lives in this origin's storage (other ports cannot
// read it). It arrives once at sign-in, in the URL fragment (`/#wbk=…`) or in the
// login response.

export class ApiError extends Error {
  status: number
  code: string
  constructor(status: number, code: string, message: string) {
    super(message)
    this.status = status
    this.code = code
  }
  /** The integration is not set up (HTTP 412). */
  get notConfigured() {
    return this.code === 'not_configured'
  }
}

type Query = Record<string, string | number | boolean | null | undefined>

let onUnauthorized: (() => void) | null = null
export function setUnauthorizedHandler(fn: () => void) {
  onUnauthorized = fn
}

/** The session ended (revoked, signed out, expired): show the login screen. */
export function notifyUnauthorized() {
  onUnauthorized?.()
}

const KEY_STORAGE = 'wb.deviceKey'
const KEY_HEADER = 'X-Workbench-Key'
let deviceKey: string | null = null

function stores(): Storage[] {
  const out: Storage[] = []
  for (const get of [() => localStorage, () => sessionStorage]) {
    try {
      const s = get()
      if (s) out.push(s)
    } catch {
      /* storage blocked */
    }
  }
  return out
}

/** This browser's device key (platform push hands it to the service worker). */
export function getDeviceKey(): string | null {
  return deviceKey
}

/** Remember this browser's device key (after signing in). */
export function setDeviceKey(key: string | null) {
  deviceKey = key
  for (const s of stores()) {
    try {
      if (key) s.setItem(KEY_STORAGE, key)
      else s.removeItem(KEY_STORAGE)
      if (key) break
    } catch {
      /* try the next one */
    }
  }
}

/**
 * A key handed over in the URL fragment (`/#wbk=…`) while another key is stored. Any
 * page can open `/#wbk=junk`, so it replaces the stored key only once that key is
 * known not to sign this browser in (`settleDeviceKey`).
 */
let handedOver: string | null = null

/** Take the stored key, and a key handed over in the URL fragment. */
function loadDeviceKey() {
  if (typeof location === 'undefined') return
  const m = /^#wbk=([A-Za-z0-9_-]+)$/.exec(location.hash)
  if (m) {
    // Out of the address bar and history.
    history.replaceState(null, '', location.pathname + location.search)
  }
  for (const s of stores()) {
    try {
      const k = s.getItem(KEY_STORAGE)
      if (k) {
        deviceKey = k
        break
      }
    } catch {
      /* try the next one */
    }
  }
  if (m && !deviceKey) setDeviceKey(m[1])
  else if (m && m[1] !== deviceKey) handedOver = m[1]
}
loadDeviceKey()

/**
 * The key to keep when sign-in handed over `handed` while `stored` is stored: the
 * stored one while it still signs this browser in (a page that opened `/#wbk=junk`
 * changes nothing), else the handed-over one (a fresh sign-in after the old session
 * ended).
 */
export async function pickDeviceKey(stored: string | null, handed: string | null, storedWorks: () => Promise<boolean>): Promise<string | null> {
  if (!handed || handed === stored) return stored
  if (!stored) return handed
  return (await storedWorks().catch(() => false)) ? stored : handed
}

/** Resolve a handed-over key (see `handedOver`) before the first authenticated request. */
export async function settleDeviceKey(): Promise<void> {
  const handed = handedOver
  handedOver = null
  if (!handed) return
  const works = async () => (await request<{ authenticated: boolean }>('GET', '/api/auth/status')).authenticated
  const key = await pickDeviceKey(deviceKey, handed, works)
  if (key !== deviceKey) setDeviceKey(key)
}

function withQuery(path: string, query?: Query): string {
  if (!query) return path
  const qs = new URLSearchParams()
  for (const [k, v] of Object.entries(query)) {
    if (v !== undefined && v !== null && v !== '') qs.set(k, String(v))
  }
  const s = qs.toString()
  return s ? `${path}${path.includes('?') ? '&' : '?'}${s}` : path
}

async function request<T>(method: string, path: string, body?: unknown, query?: Query, signal?: AbortSignal): Promise<T> {
  const init: RequestInit = { method, credentials: 'same-origin', signal, headers: deviceKey ? { [KEY_HEADER]: deviceKey } : {} }
  if (body !== undefined) {
    if (body instanceof Blob || body instanceof FormData || typeof body === 'string') {
      init.body = body as BodyInit
    } else {
      ;(init.headers as Record<string, string>)['Content-Type'] = 'application/json'
      init.body = JSON.stringify(body)
    }
  }
  const res = await fetch(withQuery(path, query), init)
  if (res.status === 401 && !path.startsWith('/api/auth/')) onUnauthorized?.()
  const text = await res.text()
  let data: unknown = undefined
  if (text) {
    try {
      data = JSON.parse(text)
    } catch {
      data = text
    }
  }
  if (!res.ok) {
    const err = (data as { error?: { code?: string; message?: string } } | undefined)?.error
    throw new ApiError(res.status, err?.code ?? 'http_' + res.status, err?.message ?? (typeof data === 'string' ? data : res.statusText))
  }
  return data as T
}

/**
 * POST a file as the raw request body, reporting progress (fetch cannot report upload
 * progress). Same auth, device key and error handling as `request`.
 */
function upload<T>(path: string, body: Blob, query?: Query, onProgress?: (sent: number, total: number) => void, signal?: AbortSignal): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    if (signal?.aborted) return reject(new DOMException('Upload cancelled', 'AbortError'))
    const xhr = new XMLHttpRequest()
    xhr.open('POST', withQuery(path, query))
    if (deviceKey) xhr.setRequestHeader(KEY_HEADER, deviceKey)
    xhr.upload.onprogress = (e) => onProgress?.(e.loaded, e.lengthComputable ? e.total : body.size)
    xhr.onload = () => {
      if (xhr.status === 401) onUnauthorized?.()
      let data: unknown = undefined
      if (xhr.responseText) {
        try {
          data = JSON.parse(xhr.responseText)
        } catch {
          data = xhr.responseText
        }
      }
      if (xhr.status >= 200 && xhr.status < 300) return resolve(data as T)
      const err = (data as { error?: { code?: string; message?: string } } | undefined)?.error
      reject(new ApiError(xhr.status, err?.code ?? 'http_' + xhr.status, err?.message ?? (typeof data === 'string' && data ? data : xhr.statusText)))
    }
    xhr.onerror = () => reject(new ApiError(0, 'network', 'The upload failed: the connection was lost'))
    xhr.onabort = () => reject(new DOMException('Upload cancelled', 'AbortError'))
    signal?.addEventListener('abort', () => xhr.abort(), { once: true })
    xhr.send(body)
  })
}

export const api = {
  get: <T>(path: string, query?: Query, signal?: AbortSignal) => request<T>('GET', path, undefined, query, signal),
  /** Raw-body file upload with progress (see `upload`). */
  upload,
  post: <T>(path: string, body?: unknown, query?: Query) => request<T>('POST', path, body ?? {}, query),
  put: <T>(path: string, body?: unknown, query?: Query) => request<T>('PUT', path, body ?? {}, query),
  patch: <T>(path: string, body?: unknown, query?: Query) => request<T>('PATCH', path, body ?? {}, query),
  del: <T>(path: string, query?: Query) => request<T>('DELETE', path, undefined, query),
  /** URL for binary content (images, downloads) — use in <img src>. */
  url: (path: string, query?: Query) => withQuery(path, query),
}

/** `ws(s)://host/path` for the current page, with the device key (browsers cannot set headers on WebSockets). */
export function wsUrl(path: string): string {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
  const key = deviceKey ? `${path.includes('?') ? '&' : '?'}wbk=${encodeURIComponent(deviceKey)}` : ''
  return `${proto}//${location.host}${path}${key}`
}

/** WebSocket close code: the device session behind the socket ended. */
export const CLOSE_SESSION_ENDED = 4401

/** Encode a project-relative path for use in a query string (kept readable). */
export function encPath(p: string): string {
  return p.split('/').map(encodeURIComponent).join('/')
}
