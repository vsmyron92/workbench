// Pure helpers of the debug feature (unit-tested in logic.test.ts).

import { basename, isAbsolutePath, samePath } from '@/features/files/modelAccess'
import type { BpStatus, CompletionItem, DebugSession, Frame, LaunchConfig, LineBreakpoint, OutputLine, RemoteConfig, SvdField, SvdPeripheralSummary, SvdRegister, Variable } from './types'

export function isLive(s: Pick<DebugSession, 'state'> | undefined | null): boolean {
  return !!s && (s.state === 'starting' || s.state === 'running' || s.state === 'stopped')
}

/** The breakpoints of one file, in the shape `PUT breakpoints/file` takes. */
export function fileBreakpoints(all: LineBreakpoint[], path: string): LineBreakpoint[] {
  return all.filter((b) => b.path === path).sort((a, b) => a.line - b.line)
}

function strip(b: LineBreakpoint): Partial<LineBreakpoint> {
  const { status: _status, ...rest } = b
  return rest
}

/** Toggle a line breakpoint: remove the one on `line`, or add a plain one. */
export function toggleLine(all: LineBreakpoint[], path: string, line: number): Partial<LineBreakpoint>[] {
  const list = fileBreakpoints(all, path)
  if (list.some((b) => b.line === line)) return list.filter((b) => b.line !== line).map(strip)
  return [...list.map(strip), { line, enabled: true }]
}

/** Replace (or add) the breakpoint on `line` with `patch` applied. */
export function patchLine(all: LineBreakpoint[], path: string, line: number, patch: Partial<LineBreakpoint>): Partial<LineBreakpoint>[] {
  const list = fileBreakpoints(all, path)
  const cur = list.find((b) => b.line === line)
  const next = { ...(cur ? strip(cur) : { line, enabled: true }), ...patch }
  for (const k of ['condition', 'hitCondition', 'logMessage'] as const) {
    if (typeof next[k] === 'string' && !next[k]!.trim()) delete next[k]
  }
  return cur ? list.map((b) => (b.line === line ? next : strip(b))) : [...list.map(strip), next]
}

/** Move breakpoints whose lines moved (the editor tracked them through edits). */
export function moveLines(all: LineBreakpoint[], path: string, moved: Map<string, number>): Partial<LineBreakpoint>[] | null {
  const list = fileBreakpoints(all, path)
  let changed = false
  const seen = new Set<number>()
  const out: Partial<LineBreakpoint>[] = []
  for (const b of list) {
    const line = moved.get(b.id) ?? b.line
    if (line !== b.line) changed = true
    if (seen.has(line)) {
      changed = true
      continue // two collapsed onto one line: keep the first
    }
    seen.add(line)
    out.push({ ...strip(b), line })
  }
  return changed ? out : null
}

export type GlyphKind = 'bp' | 'bp-cond' | 'bp-log' | 'bp-disabled' | 'bp-unverified' | 'bp-muted'

/** How a breakpoint looks in the gutter (CLion: red dot, red with ? when conditional,
 *  a diamond-ish log point, hollow while a session could not place it). */
export function glyphKind(b: Pick<LineBreakpoint, 'enabled' | 'condition' | 'hitCondition' | 'logMessage'> & { status?: BpStatus | null }, muted: boolean, live: boolean): GlyphKind {
  if (!b.enabled) return 'bp-disabled'
  if (muted) return 'bp-muted'
  if (live && b.status && !b.status.verified) return 'bp-unverified'
  if (b.logMessage) return 'bp-log'
  if (b.condition || b.hitCondition) return 'bp-cond'
  return 'bp'
}

export function glyphTitle(b: LineBreakpoint, muted: boolean): string {
  const parts = [b.logMessage ? `Log point: ${b.logMessage}` : 'Breakpoint']
  if (b.condition) parts.push(`Condition: ${b.condition}`)
  if (b.hitCondition) parts.push(`Hit count: ${b.hitCondition}`)
  if (!b.enabled) parts.push('Disabled')
  else if (muted) parts.push('Muted')
  if (b.status && !b.status.verified) parts.push(b.status.message ? `Not placed: ${b.status.message}` : 'Not placed yet')
  return parts.join('\n')
}

/** Apply a DAP completion item to `text` with the caret at `caret` (0-based). */
export function applyCompletion(text: string, caret: number, item: CompletionItem): { text: string; caret: number } {
  const insert = item.text ?? item.label
  let from: number
  if (typeof item.start === 'number') from = Math.max(0, item.start - 1)
  else from = Math.max(0, caret - (item.length ?? 0))
  const to = typeof item.start === 'number' ? Math.min(text.length, from + (item.length ?? 0)) : caret
  const next = text.slice(0, from) + insert + text.slice(to)
  return { text: next, caret: from + insert.length }
}

/** The status line of a session (tool window header, top bar). */
export function stateLabel(s: DebugSession): string {
  switch (s.state) {
    case 'starting':
      return s.phase ?? 'Starting…'
    case 'running':
      return 'Running'
    case 'stopped': {
      const r = s.stopped?.reason ?? 'pause'
      if (r === 'exception') return s.stopped?.description ? `Exception: ${s.stopped.description}` : 'Exception'
      const why: Record<string, string> = { breakpoint: 'breakpoint', 'function breakpoint': 'breakpoint', 'data breakpoint': 'watchpoint', 'instruction breakpoint': 'breakpoint', pause: '', entry: 'at the reset vector' }
      const w = why[r] ?? r
      return w ? `Paused (${w})` : 'Paused'
    }
    case 'terminated':
      if (s.stopRequested) return s.request === 'attach' && !s.parentId ? 'Detached' : 'Stopped'
      return s.exitCode != null ? `Exited with code ${s.exitCode}` : 'Terminated'
    case 'failed':
      return 'Failed'
  }
}

/** The configuration a plain "Debug" (Shift+F9) starts without asking: the one picked
 *  in this browser or started last, if it still exists. Otherwise the picker opens
 *  (a repository's configuration can run a pre-launch command). */
export function rememberedConfig(list: LaunchConfig[], picked?: string | null, last?: string | null): string | null {
  for (const n of [picked, last]) if (n && list.some((c) => c.name === n)) return n
  return null
}

export function stateTone(s: DebugSession): 'success' | 'warning' | 'danger' | 'accent' | 'muted' {
  switch (s.state) {
    case 'starting':
      return 'accent'
    case 'running':
      return 'success'
    case 'stopped':
      return 'warning'
    case 'failed':
      return 'danger'
    default:
      return 'muted'
  }
}

/** What starting a remote-target configuration runs and sends, one line each: the
 *  server's command line, what gdb connects to and the commands in the order they run.
 *  Shown in tooltips, so nothing a configuration does happens unseen. */
export function remoteLines(r: RemoteConfig, stopOnEntry: boolean): string[] {
  const out: string[] = []
  if (r.inContainer) out.push('Built in the dev container; the debugger and the debug server run on this computer')
  out.push(r.server ? `Debug server: ${r.commandLine ?? r.serverLabel ?? r.server}` : 'No debug server: gdb connects to a stub that is already running')
  out.push(`gdb connects${r.extended ? ' (target extended-remote)' : ''} to: ${r.connect ?? 'the server on this computer'}`)
  const runsProgram = r.extended && r.attach == null
  if (r.extended) out.push(runsProgram ? 'The stub runs the program' : `Attaches to target ${r.attach}`)
  const steps = [...r.init, ...r.reset]
  if (steps.length) out.push(`Then: ${r.init.length ? r.init.join('; ') : ''}${r.init.length && r.reset.length ? '; ' : ''}${r.reset.join('; ')}`)
  if (r.download) out.push(`Download the program (load)${r.reset.length ? ', then ' + r.reset.join('; ') : ''}`)
  out.push(stopOnEntry ? `Stops at ${runsProgram ? 'main' : r.stopAt === 'reset' ? 'the reset vector' : r.stopAt}` : 'Runs')
  if (r.channels.length) out.push(`Target output: ${r.channels.join(', ')}`)
  if (r.svd) out.push(`Register map: ${r.svd}`)
  return out
}

// ---------------------------------------------------------------- peripherals (SVD)

/** `0x40020000`: an address or an offset, upper case, 8 digits (wider when it needs them). */
export function hexAddress(n: number): string {
  return `0x${n.toString(16).toUpperCase().padStart(8, '0')}`
}

/** `[7:4]`, or `[3]` for one bit. */
export function fieldRange(f: Pick<SvdField, 'bitOffset' | 'bitWidth'>): string {
  return f.bitWidth === 1 ? `[${f.bitOffset}]` : `[${f.bitOffset + f.bitWidth - 1}:${f.bitOffset}]`
}

/** A field's value as the view shows it: `Down (1)`, a plain number, hex for wide fields. */
export function fieldValueText(f: Pick<SvdField, 'bitWidth' | 'value' | 'valueName'>): string {
  if (f.value == null) return ''
  const n = f.bitWidth > 8 ? `0x${f.value.toString(16).toUpperCase()}` : String(f.value)
  return f.valueName ? `${f.valueName} (${n})` : n
}

/** Peripherals matching `query` (words, all of which must appear in name, group or description). */
export function filterPeripherals(list: SvdPeripheralSummary[], query: string): SvdPeripheralSummary[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean)
  if (!words.length) return list
  return list.filter((p) => {
    const hay = `${p.name} ${p.group ?? ''} ${p.description ?? ''}`.toLowerCase()
    return words.every((w) => hay.includes(w))
  })
}

/** Registers matching `query` by name or description; all of them for an empty query. */
export function filterRegisters(list: SvdRegister[], query: string): SvdRegister[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean)
  if (!words.length) return list
  return list.filter((r) => {
    const hay = `${r.name} ${r.description ?? ''}`.toLowerCase()
    return words.every((w) => hay.includes(w))
  })
}

/** What to say where a register has no value: why it was not read, or the debugger's error. */
export function registerNote(r: Pick<SvdRegister, 'value' | 'error' | 'skipped' | 'access'>): string {
  if (r.value) return ''
  if (r.error) return r.error
  if (r.skipped) return r.skipped
  return r.access === 'write-only' ? 'write-only' : ''
}

/** The tooltip of a launch configuration: its program, what runs before, a remote
 *  target's server and commands, and its problems. */
export function configTitle(c: LaunchConfig): string {
  return [
    c.program ?? c.module ?? '',
    c.preLaunch ? `before: ${c.preLaunch}` : '',
    ...(c.remote ? remoteLines(c.remote, c.stopOnEntry) : []),
    ...c.problems.map((p) => `⚠ ${p}`),
  ]
    .filter(Boolean)
    .join('\n')
}

/** The muted text after a configuration's name: its debugger, and for a remote target
 *  the server it goes through. */
export function configSubtitle(c: LaunchConfig): string {
  const via = c.remote ? (c.remote.serverLabel ? `via ${c.remote.serverLabel}` : c.remote.connect ? `at ${c.remote.connect}` : '') : ''
  return [c.adapterLabel ?? '', via, c.preLaunch ?? ''].filter(Boolean).join(' · ')
}

/** "OpenOCD · 127.0.0.1:3333": the debug server of a remote session and where gdb is connected. */
export function remoteChip(s: Pick<DebugSession, 'remote'>): string | null {
  if (!s.remote) return null
  return [s.remote.server, s.remote.target].filter(Boolean).join(' · ') || 'remote target'
}

const ORIGIN_TITLE: Record<LaunchConfig['origin'], string> = {
  config: 'Launch configurations',
  cargo: 'Cargo',
  cmake: 'CMake',
  python: 'Python',
  go: 'Go',
}

/** Configurations grouped for pickers: explicit ones first, then derived by tool. */
export function groupConfigs(list: LaunchConfig[]): { title: string; items: LaunchConfig[] }[] {
  const order: LaunchConfig['origin'][] = ['config', 'cargo', 'cmake', 'python', 'go']
  return order.map((o) => ({ title: ORIGIN_TITLE[o], items: list.filter((c) => c.origin === o) })).filter((g) => g.items.length > 0)
}

/** The configuration a plain "Debug" starts: the last used one, else the first explicit, else the first. */
export function defaultConfig(list: LaunchConfig[], last?: string | null): LaunchConfig | null {
  return list.find((c) => c.name === last) ?? list.find((c) => c.origin === 'config') ?? list[0] ?? null
}

/** Lines a `debug.source` view marks while its session is suspended there: the
 *  execution point (the top frame) and the selected outer frame. */
export function sourceLines(
  view: { path?: string; sourceReference?: number },
  s: Pick<DebugSession, 'state' | 'stopEpoch'> | undefined,
  stack: { epoch: number; frames: Frame[] } | undefined,
  frameIndex: number,
): { line: number; kind: 'exec' | 'frame' }[] {
  if (!s || s.state !== 'stopped' || !stack || stack.epoch !== s.stopEpoch) return []
  const here = (f: Frame | undefined) => {
    const src = f?.source
    if (!src) return false
    if (view.sourceReference) return src.sourceReference === view.sourceReference
    return !src.inProject && !src.sourceReference && (src.path && view.path ? samePath(src.path, view.path) : src.path === view.path)
  }
  const out: { line: number; kind: 'exec' | 'frame' }[] = []
  const top = stack.frames[0]
  if (here(top)) out.push({ line: top.line, kind: 'exec' })
  const sel = stack.frames[frameIndex]
  if (frameIndex > 0 && here(sel) && !out.some((l) => l.line === sel.line)) out.push({ line: sel.line, kind: 'frame' })
  return out
}

/** The session of a `debug.source` view's model (`inmemory://debug-source/<sid>/…`). */
export function sourceViewSession(uri: { scheme: string; authority: string; path: string }): string | null {
  if (uri.scheme !== 'inmemory' || uri.authority !== 'debug-source') return null
  const m = /^\/([^/]+)\//.exec(uri.path)
  return m ? decodeURIComponent(m[1]) : null
}

/** The frame a stop shows. After a step or at a breakpoint, the top one when it has
 *  source (a step into a library shows the library's code); after a pause, a signal
 *  or an exception, the first frame in the project within 15 (not libc's `raise`). */
export function revealFrame(frames: Frame[], reason?: string): number {
  const top = frames[0]?.source
  const exact = ['step', 'breakpoint', 'function breakpoint', 'instruction breakpoint', 'data breakpoint', 'entry', 'goto']
  if (reason && exact.includes(reason) && (top?.path || top?.sourceReference)) return 0
  const at = frames.findIndex((f) => f.source?.path && f.source.inProject)
  return at >= 0 && at <= 15 ? at : 0
}

export function frameLocation(f: Frame): string {
  const src = f.source
  if (!src?.path) return src?.name ?? ''
  return `${basename(src.path)}:${f.line}`
}

/** The `debug.source` panel that shows a frame outside the project (null: no source):
 *  source the debugger holds, or a file at an absolute path (`/…`, or `C:\…` on a
 *  Windows server). */
export function sourcePanel(s: Pick<DebugSession, 'id' | 'projectId'>, f: Frame): { kind: string; id: string; title: string; params: Record<string, unknown> } | null {
  const src = f.source
  // DAP: a `sourceReference` means "ask the debugger", even with a path (debugpy
  // names exec'd code `<generated>`).
  if (src?.sourceReference) {
    const name = src.name ?? src.path ?? `source ${src.sourceReference}`
    return {
      kind: 'debug.source',
      id: `debug.source:${s.projectId}:${s.id}:ref${src.sourceReference}`,
      title: basename(name),
      params: { projectId: s.projectId, sessionId: s.id, sourceReference: src.sourceReference, name },
    }
  }
  if (src?.path && isAbsolutePath(src.path) && !src.inProject) {
    return {
      kind: 'debug.source',
      id: `debug.source:${s.projectId}:${src.path}`,
      title: basename(src.path),
      params: { projectId: s.projectId, sessionId: s.id, path: src.path, name: src.name ?? undefined },
    }
  }
  return null
}

/** A console's entries as lines: an entry without a final newline is ended when the
 *  next one is of another category (gdb's log points print no newline). */
export function consoleText(lines: OutputLine[]): { category: string; text: string; seq: number }[] {
  const out: { category: string; text: string; seq: number }[] = []
  for (let i = 0; i < lines.length; i++) {
    const l = lines[i]
    const next = lines[i + 1]
    const text = !l.text.endsWith('\n') && next && next.category !== l.category ? l.text + '\n' : l.text
    const prev = out[out.length - 1]
    if (prev && prev.category === l.category && !prev.text.endsWith('\n')) prev.text += text
    else out.push({ category: l.category, text, seq: l.seq })
  }
  return out
}

/** The expression under `column` (1-based) of `line`: identifiers joined by `.`,
 *  `->` or `::`, for evaluate-on-hover. */
export function expressionAt(line: string, column: number): { expr: string; start: number; end: number } | null {
  const idx = column - 1
  const isWord = (c: string | undefined) => !!c && /[A-Za-z0-9_$]/.test(c)
  if (!isWord(line[idx])) return null
  let end = idx
  while (isWord(line[end + 1])) end++
  let start = idx
  for (;;) {
    while (isWord(line[start - 1])) start--
    if (line[start - 1] === '.' && isWord(line[start - 2])) start -= 1
    else if (line.slice(start - 2, start) === '->' && isWord(line[start - 3])) start -= 2
    else if (line.slice(start - 2, start) === '::' && isWord(line[start - 3])) start -= 2
    else break
  }
  const expr = line.slice(start, end + 1)
  if (/^[0-9]/.test(expr)) return null
  return { expr, start: start + 1, end: end + 2 }
}

/** What to paste into an agent session for "Ask agent about this stop". */
export function agentPrompt(o: {
  session: DebugSession
  frames: Frame[]
  frameIndex: number
  locals: Variable[]
  console: OutputLine[]
}): string {
  const s = o.session
  const st = s.stopped
  const lines: string[] = []
  const top = o.frames[o.frameIndex] ?? o.frames[0]
  const where = top?.source?.path ? ` at ${top.source.inProject ? '@' : ''}${top.source.path}:${top.line}` : ''
  lines.push(`The debugger (${s.adapterLabel}, session "${s.name}") stopped: ${st?.reason ?? 'paused'}${st?.description ? ` — ${st.description}` : ''}${where}.`)
  if (st?.text) lines.push(`Details: ${st.text.slice(0, 1500)}`)
  if (o.frames.length) {
    lines.push('', 'Call stack:')
    for (const [i, f] of o.frames.slice(0, 15).entries()) {
      const loc = f.source?.path ? `${f.source.path}:${f.line}` : f.source?.name ?? '?'
      lines.push(`${i === o.frameIndex ? '→' : ' '} #${i} ${f.name} at ${loc}`)
    }
  }
  if (o.locals.length) {
    lines.push('', `Variables of frame #${o.frameIndex}:`)
    for (const v of o.locals.slice(0, 40)) {
      const value = v.value.length > 200 ? v.value.slice(0, 200) + '…' : v.value
      lines.push(`  ${v.name}${v.type ? `: ${v.type}` : ''} = ${value || (v.variablesReference ? '{…}' : '')}`)
    }
  }
  const tail = o.console
    // `target`: a microcontroller's own output (UART, SWO), the program's output there.
    .filter((l) => l.category === 'stdout' || l.category === 'stderr' || l.category === 'console' || l.category === 'important' || l.category === 'target')
    .slice(-15)
    .map((l) => l.text)
    .join('')
    .trimEnd()
  if (tail) lines.push('', 'Recent program output:', '```', tail.slice(-3000), '```')
  lines.push('', '(The debug_state tool shows the live session.) ')
  return lines.join('\n')
}

// ---------------------------------------------------------------- live watch

/** Readings kept per watched expression (the sparkline's width in samples). */
export const LIVE_HISTORY = 120

export type Radix = 'dec' | 'hex' | 'bin'

/** The number a reading stands for, for the sparkline: numbers, booleans (0 or 1) and numeric text (64-bit values). */
export function numericValue(v: unknown): number | null {
  if (typeof v === 'number') return Number.isFinite(v) ? v : null
  if (typeof v === 'boolean') return v ? 1 : 0
  if (typeof v === 'string' && /^-?\d+$/.test(v)) return Number(v)
  return null
}

/** `v` appended to `list`, keeping the newest `max`. */
export function pushSample<T>(list: readonly T[] | undefined, v: T, max = LIVE_HISTORY): T[] {
  const next = list ? [...list, v] : [v]
  return next.length > max ? next.slice(next.length - max) : next
}

const pad = (digits: string, width: number) => digits.padStart(width, '0')

/** A watched value as shown. Integers follow `radix` (a negative one in hex or binary is its two's complement
 *  in `size` bytes); a pointer is hex; a float has up to seven digits; bytes are as they came. */
export function formatLiveValue(kind: string, size: number, v: unknown, radix: Radix = 'dec'): string {
  if (v === undefined || v === null) return '—'
  if (typeof v === 'boolean') return v ? 'true' : 'false'
  if (kind === 'bytes') return String(v)
  if (kind === 'float') {
    if (typeof v === 'number') return String(Number(v.toPrecision(7)))
    return String(v)
  }
  if (typeof v !== 'number' && typeof v !== 'string') return String(v)
  let n: bigint
  try {
    n = BigInt(v)
  } catch {
    return String(v)
  }
  const bits = Math.max(1, size) * 8
  const unsigned = BigInt.asUintN(bits, n)
  if (kind === 'ptr' || radix === 'hex') return `0x${pad(unsigned.toString(16), bits / 4)}`
  if (radix === 'bin') return `0b${pad(unsigned.toString(2), bits).replace(/(.{4})(?=.)/g, '$1_')}`
  return n.toString()
}

/** The points of a sparkline's polyline in a `w`×`h` box (`pad` inside the edges): the oldest reading on
 *  the left, the largest value on top. A constant series is a line across the middle; a gap (a failed
 *  reading) splits the line, so the result is one polyline per run. */
export function sparklineRuns(values: readonly (number | null)[], w: number, h: number, pad = 2): string[] {
  const nums = values.filter((x): x is number => x !== null)
  if (!nums.length) return []
  const lo = Math.min(...nums)
  const hi = Math.max(...nums)
  const x = (i: number) => (values.length < 2 ? w / 2 : pad + (i * (w - 2 * pad)) / (values.length - 1))
  const y = (v: number) => (hi === lo ? h / 2 : h - pad - ((v - lo) * (h - 2 * pad)) / (hi - lo))
  const runs: string[] = []
  let cur: string[] = []
  values.forEach((v, i) => {
    if (v === null) {
      if (cur.length) runs.push(cur.join(' '))
      cur = []
    } else {
      cur.push(`${x(i).toFixed(1)},${y(v).toFixed(1)}`)
    }
  })
  if (cur.length) runs.push(cur.join(' '))
  return runs
}
