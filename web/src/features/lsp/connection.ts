// One socket per project to `/api/projects/{pid}/lsp/ws` (see server/src/lsp/ws.rs):
// document sync (full text, debounced, flushed before every request on the
// document), requests with cancellation, diagnostics, and the server's own requests.
// Reconnects with backoff and opens its documents again; stops for good when code
// intelligence is disabled (`disabled`) or the device signs out (4401).

import { CLOSE_SESSION_ENDED, notifyUnauthorized, wsUrl } from '@/api/client'
import type { LspDiagnostic } from './api'

export type ConnState = 'connecting' | 'open' | 'closed' | 'disabled'

/** How long a document's edits wait before they are sent (a request sends them at once). */
const CHANGE_DEBOUNCE_MS = 300
/** A browser request gives up after this (the server has its own, shorter limits). */
const REQUEST_TIMEOUT_MS = 130_000
/** Documents larger than this are not synchronized (the server refuses them too). */
export const MAX_DOC_CHARS = 8 * 1024 * 1024

export interface DocSource {
  uri: string
  languageId: string
  /** Current text. */
  text: () => string
  /** Changes whenever the text does. */
  version: () => number
}

interface TrackedDoc {
  src: DocSource
  /** Version last sent (-1: not sent since the socket opened). */
  sent: number
  timer?: ReturnType<typeof setTimeout>
  /** Server id from `opened` (null: no server handles it; undefined: not answered yet). */
  server?: string | null
  error?: string
}

export class LspError extends Error {
  code: number
  constructor(code: number, message: string) {
    super(message)
    this.code = code
  }
}

export interface ConnectionHandlers {
  onState?: (s: ConnState) => void
  onOpened?: (uri: string, server: string | null, error?: string) => void
  onDiagnostics?: (uri: string, server: string, diagnostics: LspDiagnostic[]) => void
  onCaps?: (server: string, caps: Record<string, unknown>) => void
  onDown?: (server: string) => void
  onMessage?: (server: string, level: number, message: string) => void
  onRefresh?: (server: string, what: string) => void
  /** A server request the editor answers (`workspace/applyEdit`, `window/showMessageRequest`). */
  onRequest?: (server: string, method: string, params: unknown) => Promise<unknown>
  /** The socket could not be opened (maybe code intelligence is off): the owner checks. */
  onRefused?: () => void
}

interface Pending {
  resolve: (v: { result: unknown; server?: string }) => void
  reject: (e: unknown) => void
  timer: ReturnType<typeof setTimeout>
}

export class LspConnection {
  readonly projectId: string
  state: ConnState = 'connecting'
  /** Ready servers and their capabilities. */
  readonly servers = new Map<string, Record<string, unknown>>()
  private ws: WebSocket | null = null
  private handlers: ConnectionHandlers
  private docs = new Map<string, TrackedDoc>()
  private pending = new Map<number, Pending>()
  private nextId = 1
  private retry = 0
  private retryTimer?: ReturnType<typeof setTimeout>
  private disposed = false
  private everOpened = false
  private openWaiters: (() => void)[] = []

  constructor(projectId: string, handlers: ConnectionHandlers) {
    this.projectId = projectId
    this.handlers = handlers
    this.connect()
  }

  // ------------------------------------------------------------ socket

  private setState(s: ConnState) {
    if (this.state === s) return
    this.state = s
    this.handlers.onState?.(s)
  }

  private connect() {
    if (this.disposed) return
    this.setState('connecting')
    let ws: WebSocket
    try {
      ws = new WebSocket(wsUrl(`/api/projects/${encodeURIComponent(this.projectId)}/lsp/ws`))
    } catch {
      this.scheduleReconnect()
      return
    }
    this.ws = ws
    let opened = false
    let ping: ReturnType<typeof setInterval> | undefined
    ws.onopen = () => {
      opened = true
      this.everOpened = true
      this.retry = 0
      ping = setInterval(() => this.sendRaw({ t: 'ping' }), 25_000)
    }
    ws.onmessage = (m) => {
      let msg: Record<string, unknown>
      try {
        msg = JSON.parse(m.data as string)
      } catch {
        return
      }
      this.dispatch(msg)
    }
    ws.onclose = (ev) => {
      clearInterval(ping)
      if (this.ws === ws) this.ws = null
      this.servers.clear()
      this.failPending(new LspError(-32098, 'the code intelligence connection closed'))
      for (const d of this.docs.values()) {
        clearTimeout(d.timer)
        d.timer = undefined
        d.sent = -1
        d.server = undefined
      }
      if (this.disposed || this.state === 'disabled') return
      if (ev.code === CLOSE_SESSION_ENDED) {
        this.setState('closed')
        notifyUnauthorized()
        return
      }
      this.setState('closed')
      if (!opened) this.handlers.onRefused?.()
      this.scheduleReconnect()
    }
  }

  private scheduleReconnect() {
    if (this.disposed || this.state === 'disabled') return
    clearTimeout(this.retryTimer)
    this.retry = Math.min(this.retry + 1, 6)
    const delay = this.everOpened && this.retry === 1 ? 300 : Math.min(500 * 2 ** this.retry, 15_000)
    this.retryTimer = setTimeout(() => this.connect(), delay)
  }

  private sendRaw(msg: unknown): boolean {
    const ws = this.ws
    if (!ws || ws.readyState !== WebSocket.OPEN) return false
    ws.send(JSON.stringify(msg))
    return true
  }

  private dispatch(msg: Record<string, unknown>) {
    switch (msg.t) {
      case 'hello': {
        const servers = (msg.servers ?? {}) as Record<string, { capabilities: Record<string, unknown> }>
        this.servers.clear()
        for (const [id, s] of Object.entries(servers)) {
          this.servers.set(id, s.capabilities ?? {})
          this.handlers.onCaps?.(id, s.capabilities ?? {})
        }
        this.setState('open')
        this.openWaiters.splice(0).forEach((w) => w())
        for (const d of this.docs.values()) this.sendOpen(d)
        break
      }
      case 'opened': {
        const d = this.docs.get(msg.uri as string)
        if (d) {
          d.server = (msg.server as string | null) ?? null
          d.error = msg.error as string | undefined
        }
        this.handlers.onOpened?.(msg.uri as string, (msg.server as string | null) ?? null, msg.error as string | undefined)
        break
      }
      case 'caps':
        this.servers.set(msg.server as string, (msg.capabilities ?? {}) as Record<string, unknown>)
        this.handlers.onCaps?.(msg.server as string, (msg.capabilities ?? {}) as Record<string, unknown>)
        break
      case 'down':
        this.servers.delete(msg.server as string)
        this.handlers.onDown?.(msg.server as string)
        break
      case 'diagnostics':
        this.handlers.onDiagnostics?.(msg.uri as string, msg.server as string, (msg.diagnostics ?? []) as LspDiagnostic[])
        break
      case 'res': {
        const p = this.pending.get(msg.id as number)
        if (!p) return
        this.pending.delete(msg.id as number)
        clearTimeout(p.timer)
        const err = msg.error as { code: number; message: string } | undefined
        if (err) p.reject(new LspError(err.code, err.message))
        else p.resolve({ result: msg.result, server: msg.server as string | undefined })
        break
      }
      case 'message':
        this.handlers.onMessage?.(msg.server as string, msg.level as number, msg.message as string)
        break
      case 'refresh':
        this.handlers.onRefresh?.(msg.server as string, msg.what as string)
        break
      case 'request': {
        const id = msg.id
        const run = this.handlers.onRequest?.(msg.server as string, msg.method as string, msg.params) ?? Promise.resolve(null)
        run.then(
          (result) => this.sendRaw({ t: 'reply', id, result: result ?? null }),
          () => this.sendRaw({ t: 'reply', id, result: null }),
        )
        break
      }
      case 'disabled':
        this.setState('disabled')
        this.ws?.close()
        break
      default:
        break
    }
  }

  private failPending(e: Error) {
    for (const [, p] of this.pending) {
      clearTimeout(p.timer)
      p.reject(e)
    }
    this.pending.clear()
  }

  /** Resolves once the socket is open (or rejects after `ms`). */
  whenOpen(ms = 8000): Promise<void> {
    if (this.state === 'open') return Promise.resolve()
    if (this.state === 'disabled' || this.disposed) return Promise.reject(new LspError(-32002, 'code intelligence is off'))
    return new Promise((resolve, reject) => {
      const t = setTimeout(() => reject(new LspError(-32002, 'code intelligence is not connected')), ms)
      this.openWaiters.push(() => {
        clearTimeout(t)
        resolve()
      })
    })
  }

  dispose() {
    this.disposed = true
    clearTimeout(this.retryTimer)
    for (const d of this.docs.values()) clearTimeout(d.timer)
    this.docs.clear()
    this.failPending(new LspError(-32098, 'code intelligence was turned off'))
    this.ws?.close()
    this.ws = null
  }

  // ------------------------------------------------------------ documents

  private sendOpen(d: TrackedDoc) {
    const text = d.src.text()
    if (text.length > MAX_DOC_CHARS) return
    const v = d.src.version()
    if (this.sendRaw({ t: 'open', uri: d.src.uri, languageId: d.src.languageId, text })) d.sent = v
  }

  track(src: DocSource) {
    if (this.docs.has(src.uri)) return
    const d: TrackedDoc = { src, sent: -1 }
    this.docs.set(src.uri, d)
    if (this.state === 'open') this.sendOpen(d)
  }

  untrack(uri: string) {
    const d = this.docs.get(uri)
    if (!d) return
    clearTimeout(d.timer)
    this.docs.delete(uri)
    this.sendRaw({ t: 'close', uri })
  }

  tracked(uri: string): boolean {
    return this.docs.has(uri)
  }

  trackedUris(): string[] {
    return [...this.docs.keys()]
  }

  /** The text changed: send it after a pause (or with the next request). */
  changed(uri: string) {
    const d = this.docs.get(uri)
    if (!d || d.server === null) return
    clearTimeout(d.timer)
    d.timer = setTimeout(() => this.flush(uri), CHANGE_DEBOUNCE_MS)
  }

  /** Send pending edits of a document now. */
  flush(uri: string) {
    const d = this.docs.get(uri)
    if (!d) return
    clearTimeout(d.timer)
    d.timer = undefined
    if (this.state !== 'open' || d.sent < 0 || d.server === null) return
    const v = d.src.version()
    if (v === d.sent) return
    const text = d.src.text()
    if (text.length > MAX_DOC_CHARS) return
    if (this.sendRaw({ t: 'change', uri, text })) d.sent = v
  }

  saved(uri: string) {
    if (!this.docs.has(uri)) return
    this.flush(uri)
    this.sendRaw({ t: 'save', uri })
  }

  /** The server synchronizing a document (undefined: not known yet; null: none). */
  serverOf(uri: string): string | null | undefined {
    return this.docs.get(uri)?.server
  }

  docError(uri: string): string | undefined {
    return this.docs.get(uri)?.error
  }

  // ------------------------------------------------------------ requests

  /**
   * Ask the server of `params.textDocument.uri` (or `server`; `workspace/symbol`
   * without either asks every ready server). Pending edits of the document go first.
   */
  request<T = unknown>(method: string, params: unknown, opts: { server?: string; signal?: AbortSignal } = {}): Promise<{ result: T; server?: string }> {
    const uri = (params as { textDocument?: { uri?: string } } | null)?.textDocument?.uri
    if (uri) this.flush(uri)
    if (this.state !== 'open') return Promise.reject(new LspError(-32002, 'code intelligence is not connected'))
    if (opts.signal?.aborted) return Promise.reject(new DOMException('cancelled', 'AbortError'))
    const id = this.nextId++
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        if (this.pending.delete(id)) {
          this.sendRaw({ t: 'cancel', id })
          reject(new LspError(-32099, `${method} timed out`))
        }
      }, REQUEST_TIMEOUT_MS)
      this.pending.set(id, { resolve: resolve as Pending['resolve'], reject, timer })
      opts.signal?.addEventListener(
        'abort',
        () => {
          const p = this.pending.get(id)
          if (!p) return
          this.pending.delete(id)
          clearTimeout(p.timer)
          this.sendRaw({ t: 'cancel', id })
          reject(new DOMException('cancelled', 'AbortError'))
        },
        { once: true },
      )
      const msg: Record<string, unknown> = { t: 'req', id, method, params }
      if (opts.server) msg.server = opts.server
      if (!this.sendRaw(msg)) {
        this.pending.delete(id)
        clearTimeout(timer)
        reject(new LspError(-32002, 'code intelligence is not connected'))
      }
    })
  }
}
