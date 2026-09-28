// Pure helpers of the lsp UI (covered by vitest).

import type { LspDiagnostic, LspStatus, ServerState, ServerStatus } from './api'

/** Lowercase extension without the dot (`''` for none and for dotfiles). */
export function extOf(path: string): string {
  const name = path.slice(path.lastIndexOf('/') + 1)
  const i = name.lastIndexOf('.')
  return i > 0 ? name.slice(i + 1).toLowerCase() : ''
}

/** The server that would handle a file: the first enabled one by extension, else by language (the server's order). */
export function serverFor(status: Pick<LspStatus, 'servers'>, path: string, language?: string): ServerStatus | null {
  const ext = extOf(path)
  const usable = status.servers.filter((s) => s.enabled && !s.disabledHere)
  const byExt = usable.filter((s) => ext && s.extensions.includes(ext))
  const byLang = usable.filter((s) => language && language !== 'plaintext' && s.languages.includes(language))
  const list = byExt.length ? byExt : byLang
  return list.find((s) => s.available) ?? list[0] ?? null
}

/** Servers worth showing first: running, failing, or relevant to the project. */
export function activeServers(status: LspStatus): ServerStatus[] {
  return status.servers.filter((s) => ['starting', 'indexing', 'ready', 'crashed', 'failed'].includes(s.state))
}

/**
 * The popover's first list: running or failing servers, then for each language of the
 * project the server that serves it (the first installed one; alternatives such as
 * basedpyright next to Pyright stay under "Other servers").
 */
export function serversInUse(status: LspStatus): ServerStatus[] {
  const out: ServerStatus[] = [...activeServers(status)]
  const covered = new Set(out.flatMap((s) => s.languages))
  const candidates = status.servers.filter((s) => s.relevant && s.enabled && !s.disabledHere && !out.includes(s))
  // Installed ones first, keeping the preference order otherwise.
  for (const s of [...candidates.filter((x) => x.available), ...candidates.filter((x) => !x.available)]) {
    if (s.languages.some((l) => covered.has(l))) continue
    out.push(s)
    s.languages.forEach((l) => covered.add(l))
  }
  return status.servers.filter((s) => out.includes(s))
}

export const STATE_LABEL: Record<ServerState, string> = {
  off: 'Not running',
  starting: 'Starting',
  indexing: 'Indexing',
  ready: 'Ready',
  stopped: 'Stopped',
  crashed: 'Crashed',
  failed: 'Failed to start',
  unavailable: 'Not installed',
  disabled: 'Disabled',
}

export function stateTone(s: ServerState): 'success' | 'warning' | 'danger' | 'accent' | 'muted' {
  switch (s) {
    case 'ready':
      return 'success'
    case 'starting':
    case 'indexing':
      return 'accent'
    case 'crashed':
    case 'failed':
      return 'danger'
    case 'unavailable':
      return 'warning'
    default:
      return 'muted'
  }
}

/** One line for a server's progress (`Indexing 42% · crates`). */
export function progressText(s: Pick<ServerStatus, 'state' | 'progress'>): string {
  const p = s.progress
  if (!p) return STATE_LABEL[s.state]
  const pct = p.percentage !== undefined && p.percentage !== null ? ` ${p.percentage}%` : ''
  const msg = p.message ? ` · ${p.message}` : ''
  return `${p.title || STATE_LABEL[s.state]}${pct}${msg}`
}

/** The status bar's summary of the project's servers. */
export function summarize(status: LspStatus): { tone: 'success' | 'accent' | 'danger' | 'muted'; text: string; busy: boolean } {
  if (!status.enabled) return { tone: 'muted', text: 'Code intelligence off', busy: false }
  const active = activeServers(status)
  if (!active.length) return { tone: 'muted', text: 'No language server running', busy: false }
  const bad = active.filter((s) => s.state === 'crashed' || s.state === 'failed')
  const busy = active.filter((s) => s.state === 'starting' || s.state === 'indexing')
  if (bad.length) return { tone: 'danger', text: `${bad.map((s) => s.label).join(', ')} ${bad.length > 1 ? 'failed' : STATE_LABEL[bad[0].state].toLowerCase()}`, busy: false }
  if (busy.length) {
    const s = busy[0]
    const pct = s.progress?.percentage
    return { tone: 'accent', text: `${s.label}: ${s.state === 'starting' ? 'starting' : 'indexing'}${pct !== undefined && pct !== null ? ` ${pct}%` : ''}`, busy: true }
  }
  return { tone: 'success', text: active.map((s) => s.label).join(', '), busy: false }
}

export type Severity = 'error' | 'warning' | 'info' | 'hint'

export function severityOf(d: Pick<LspDiagnostic, 'severity'>): Severity {
  switch (d.severity) {
    case 2:
      return 'warning'
    case 3:
      return 'info'
    case 4:
      return 'hint'
    default:
      return 'error'
  }
}

/** Diagnostics filtered by the Problems window's toggles, most severe first, then by line. */
export function filterDiagnostics<T extends Pick<LspDiagnostic, 'severity' | 'range'>>(list: T[], show: Record<Severity, boolean>): T[] {
  return list
    .filter((d) => show[severityOf(d)])
    .sort((a, b) => (a.severity ?? 1) - (b.severity ?? 1) || a.range.start.line - b.range.start.line || a.range.start.character - b.range.start.character)
}

/** Split a preview line around a highlighted range, trimming long leading whitespace. */
export function previewParts(line: string, start: number, end: number, max = 220): { before: string; match: string; after: string } {
  const lead = line.length - line.trimStart().length
  const cut = Math.min(lead, start)
  let before = line.slice(cut, start)
  const match = line.slice(start, Math.max(start, end))
  let after = line.slice(Math.max(start, end))
  if (before.length > 80) before = '…' + before.slice(before.length - 79)
  if (before.length + match.length + after.length > max) after = after.slice(0, Math.max(0, max - before.length - match.length)) + '…'
  return { before, match, after }
}
