// Terminal WebSocket protocol (see server/src/terminals/routes.rs):
//   server → client: text {"t":"snapshot"|"resync", cols, rows} then a binary snapshot;
//                    live output as binary; {"t":"exit",code,signal}; {"t":"running"};
//                    {"t":"size",cols,rows} when the PTY size changes.
//   client → server: binary input; text {"t":"resize",cols,rows}.
// The very first binary frame is always a snapshot, even without a marker.
// Several views can show one terminal (a desktop panel and a phone): the most recently
// active one decides the PTY size, and the others render at that size until the user
// acts in them.

export type Control =
  | { t: 'snapshot' | 'resync'; cols?: number; rows?: number }
  | { t: 'exit'; code: number | null; signal: string | null }
  | { t: 'running' }
  | { t: 'size'; cols: number; rows: number }
  | { t: 'pong' }

export type FrameAction =
  | { kind: 'snapshot'; data: Uint8Array; cols: number | null; rows: number | null }
  | { kind: 'data'; data: Uint8Array }
  | { kind: 'exit'; code: number | null; signal: string | null }
  | { kind: 'running' }
  | { kind: 'size'; cols: number; rows: number }
  | { kind: 'none' }

/** Per-connection routing state: whether the next binary frame is a snapshot. */
export class FrameRouter {
  private expectSnapshot = true
  private size: { cols: number; rows: number } | null = null

  /** Call when (re)connecting: the server starts every connection with a snapshot. */
  reset() {
    this.expectSnapshot = true
    this.size = null
  }

  text(raw: string): FrameAction {
    let msg: Control
    try {
      msg = JSON.parse(raw) as Control
    } catch {
      return { kind: 'none' }
    }
    switch (msg.t) {
      case 'snapshot':
      case 'resync':
        this.expectSnapshot = true
        this.size = msg.cols && msg.rows ? { cols: msg.cols, rows: msg.rows } : null
        return { kind: 'none' }
      case 'exit':
        return { kind: 'exit', code: msg.code ?? null, signal: msg.signal ?? null }
      case 'running':
        return { kind: 'running' }
      case 'size':
        return validSize(msg.cols, msg.rows) ? { kind: 'size', cols: msg.cols, rows: msg.rows } : { kind: 'none' }
      default:
        return { kind: 'none' }
    }
  }

  binary(data: Uint8Array): FrameAction {
    if (this.expectSnapshot) {
      this.expectSnapshot = false
      const s = this.size
      this.size = null
      return { kind: 'snapshot', data, cols: s?.cols ?? null, rows: s?.rows ?? null }
    }
    return { kind: 'data', data }
  }
}

// ---------------------------------------------------------------- clipboard (OSC 52)

/** Programs may put at most this much on the clipboard. */
export const OSC52_MAX = 1024 * 1024

/**
 * Whether a program's OSC 52 request may write the system clipboard. Reads (queries)
 * always get an empty clipboard: any output — an ssh session, a log, a tool printing
 * untrusted text — could otherwise read it. Writes need the user to be in this terminal
 * (it has focus), so a program cannot replace the clipboard behind their back.
 */
export function osc52WriteAllowed(selection: string, text: string, terminalFocused: boolean): boolean {
  return selection === 'c' && terminalFocused && text.length > 0 && text.length <= OSC52_MAX
}

export const MIN_COLS = 20
export const MAX_COLS = 500
export const MIN_ROWS = 5
export const MAX_ROWS = 200

export function validSize(cols: number, rows: number): boolean {
  return Number.isInteger(cols) && Number.isInteger(rows) && cols >= MIN_COLS && cols <= MAX_COLS && rows >= MIN_ROWS && rows <= MAX_ROWS
}

/** Reconnect delay: 0.5 s doubling to 5 s. */
export function backoff(attempt: number): number {
  return Math.min(500 * 2 ** Math.max(0, attempt), 5000)
}

// ---------------------------------------------------------------- wheel policy

export type WheelDecision = 'local' | 'default'

/**
 * Normal buffer with scrollback → scroll our own history even when the program enabled
 * mouse reporting (Claude on the main screen); alternate buffer → let xterm pass the
 * wheel to the program (full-screen TUIs).
 */
export function wheelDecision(bufferType: 'normal' | 'alternate', baseY: number, mouseTracking: string): WheelDecision {
  if (bufferType === 'normal' && baseY > 0 && mouseTracking !== 'none') return 'local'
  return 'default'
}

/** Wheel delta in lines (deltaMode: 0 pixels, 1 lines, 2 pages). */
export function wheelLines(deltaY: number, deltaMode: number, lineHeight: number, rows: number): number {
  if (deltaMode === 1) return deltaY
  if (deltaMode === 2) return deltaY * rows
  return deltaY / Math.max(1, lineHeight)
}

// ---------------------------------------------------------------- keys

/** The phone extra-keys bar. */
export const EXTRA_KEYS: { label: string; title: string; seq: string }[] = [
  { label: 'Esc', title: 'Escape', seq: '\x1b' },
  { label: 'Tab', title: 'Tab', seq: '\t' },
  { label: '⇧Tab', title: 'Shift+Tab', seq: '\x1b[Z' },
  { label: '^C', title: 'Ctrl+C', seq: '\x03' },
  { label: '↑', title: 'Up', seq: '\x1b[A' },
  { label: '↓', title: 'Down', seq: '\x1b[B' },
  { label: '←', title: 'Left', seq: '\x1b[D' },
  { label: '→', title: 'Right', seq: '\x1b[C' },
  { label: '⏎', title: 'Enter', seq: '\r' },
]
