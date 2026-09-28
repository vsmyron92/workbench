// One WebSocket per mounted terminal view, with reconnects. Each (re)connection starts
// with a snapshot the view must apply after `term.reset()`.

import { wsUrl } from '@/api/client'
import { backoff, FrameRouter, validSize, type FrameAction } from './protocol'

export type SocketStatus = 'connecting' | 'open' | 'closed'

export interface TermSocketHandlers {
  onAction(a: FrameAction): void
  onStatus(s: SocketStatus): void
}

export class TermSocket {
  private ws: WebSocket | null = null
  private router = new FrameRouter()
  private attempt = 0
  private timer: number | undefined
  private disposed = false
  private lastSize: string | null = null
  private readonly id: string
  private readonly h: TermSocketHandlers

  constructor(id: string, handlers: TermSocketHandlers) {
    this.id = id
    this.h = handlers
    this.connect()
  }

  private connect() {
    if (this.disposed) return
    this.h.onStatus('connecting')
    this.router.reset()
    this.lastSize = null
    const ws = new WebSocket(wsUrl(`/api/terminals/${encodeURIComponent(this.id)}/ws`))
    ws.binaryType = 'arraybuffer'
    this.ws = ws
    ws.onopen = () => {
      this.attempt = 0
      this.h.onStatus('open')
    }
    ws.onmessage = (m) => {
      const a = typeof m.data === 'string' ? this.router.text(m.data) : this.router.binary(new Uint8Array(m.data as ArrayBuffer))
      if (a.kind !== 'none') this.h.onAction(a)
    }
    ws.onclose = () => {
      if (this.ws !== ws) return
      this.ws = null
      this.h.onStatus('closed')
      if (this.disposed) return
      this.timer = window.setTimeout(() => this.connect(), backoff(this.attempt++))
    }
  }

  get open(): boolean {
    return this.ws?.readyState === WebSocket.OPEN
  }

  send(data: string | Uint8Array<ArrayBuffer>) {
    if (!this.open) return
    this.ws!.send(typeof data === 'string' ? new TextEncoder().encode(data) : data)
  }

  /** Report the view size; duplicates are skipped. `force` re-sends (visibility change). */
  resize(cols: number, rows: number, force = false) {
    if (!this.open || !validSize(cols, rows)) return
    const key = `${cols}x${rows}`
    if (!force && key === this.lastSize) return
    this.lastSize = key
    this.ws!.send(JSON.stringify({ t: 'resize', cols, rows }))
  }

  /** Reconnect now (e.g. the page came back from the background). */
  kick() {
    if (this.ws || this.disposed) return
    window.clearTimeout(this.timer)
    this.attempt = 0
    this.connect()
  }

  dispose() {
    this.disposed = true
    window.clearTimeout(this.timer)
    const ws = this.ws
    this.ws = null
    if (ws && ws.readyState <= WebSocket.OPEN) ws.close()
  }
}
