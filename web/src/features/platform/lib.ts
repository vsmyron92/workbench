// Pure helpers for the platform feature (tested in lib.test.ts).

import type { ActivityEvent, McpCall, SecretRef } from './types'

type Params = Record<string, unknown>

const str = (v: unknown) => (typeof v === 'string' || typeof v === 'number' ? String(v) : '')

/**
 * The stable panel id for a kind and its params, following the conventions in
 * docs/ARCHITECTURE.md#panels, so an agent's `ui.open` focuses an already open tab.
 */
export function panelIdFor(kind: string, params: Params): string {
  const pid = params.projectId === null || params.projectId === undefined ? '' : str(params.projectId)
  switch (kind) {
    case 'agents.home':
    case 'settings':
      return kind
    case 'terminal':
      return `terminal:${str(params.terminalId)}`
    case 'editor':
    case 'markdown':
      return `${kind}:${pid}:${str(params.path)}`
    case 'search':
    case 'gitlog':
      return `${kind}:${pid}`
    case 'diff': {
      const mode = str(params.mode) || 'working'
      const ref = params.sha ? str(params.sha) : params.base || params.head ? `${str(params.base)}..${str(params.head)}` : ''
      return `diff:${pid}:${mode}:${ref}:${str(params.path)}`
    }
    case 'commit':
      return `commit:${pid}:${str(params.sha)}`
    case 'conflict':
      return `conflict:${pid}:${str(params.path)}`
    case 'mr':
      return `mr:${pid}:${str(params.iid)}`
    case 'pipeline':
      return `pipeline:${pid}:${str(params.pipelineId)}`
    case 'job':
      return `job:${pid}:${str(params.jobId)}`
    case 'gitlab.issue':
      return `gitlab.issue:${pid}:${str(params.iid)}`
    case 'pr':
    case 'gh.issue':
      return `${kind}:${pid}:${str(params.number)}`
    case 'gh.run':
      return `gh.run:${pid}:${str(params.runId)}`
    case 'gh.job':
      return `gh.job:${pid}:${str(params.jobId)}`
    case 'workspace.home':
      return kind
    case 'card':
      return `card:${str(params.scope)}:${str(params.cardId)}`
    case 'confluence':
      return `confluence:${str(params.pageId)}`
    case 'jira':
      return `jira:${str(params.key)}`
    case 'app':
      return `app:${pid}:${str(params.env) || str(params.run) || str(params.url)}`
    default: {
      const keys = Object.keys(params).sort()
      return keys.length ? `${kind}:${JSON.stringify(params, keys)}` : kind
    }
  }
}

/** `m:ss` for a countdown; `0:00` once expired. */
export function formatCountdown(msLeft: number): string {
  const s = Math.max(0, Math.ceil(msLeft / 1000))
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`
}

/** `null` when `h` is a valid allowed-hosts entry, else the problem. */
export function hostEntryError(h: string): string | null {
  const v = h.trim()
  if (!v) return 'Enter a host name'
  if (v.includes('://')) return 'Host only — no http:// or https://'
  if (v.includes('/')) return 'Host only — no path'
  if (/[\s@]/.test(v)) return 'Host names cannot contain spaces or @'
  return null
}

/**
 * `null` when `p` can be a path in Settings › Projects, else the problem: absolute or under
 * `~`, and on a Windows server also `C:\…`, `C:/…` and `\\server\share` (which the server
 * then refuses with its own reason), as `util::os::path::is_absolute_str` reads them.
 */
export function projectPathError(p: string, os: string | null | undefined): string | null {
  if (p.startsWith('/') || p.startsWith('~')) return null
  if (os !== 'windows') return 'Use an absolute path or ~/…'
  return /^([A-Za-z]:)?[\\/]/.test(p) ? null : 'Use an absolute path (C:\\…) or ~\\…'
}

/** `null` when `u` can be the public URL, else the problem. */
export function publicUrlError(u: string): string | null {
  const v = u.trim()
  if (!v) return null
  let url: URL
  try {
    url = new URL(v)
  } catch {
    return 'Not a URL'
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') return 'Use http:// or https://'
  if (url.username || url.password) return 'No credentials in the URL'
  return null
}

/** Host (with port) of a URL, or '' when it cannot be parsed. */
export function hostOf(url: string): string {
  try {
    return new URL(url).host
  } catch {
    return ''
  }
}

/** Prepend `item` (newest first), replacing an entry with the same id, keep at most `cap`. */
export function prependCapped<T extends { id: number }>(list: T[], item: T, cap = 500): T[] {
  return [item, ...list.filter((x) => x.id !== item.id)].slice(0, cap)
}

/** Host name of a URL without port or IPv6 brackets ('' when unparsable). */
export function hostNameOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^\[(.*)\]$/, '$1')
  } catch {
    return ''
  }
}

export type TimelineItem = { type: 'call'; at: number; call: McpCall } | { type: 'event'; at: number; event: ActivityEvent }

export interface TimelineFilter {
  show: 'all' | 'calls' | 'events'
  writesOnly: boolean
  errorsOnly: boolean
  /** Hide everything at or before this time (client-side "clear"). */
  since: number
}

/** Tool calls and events merged newest first, filtered. */
export function timeline(calls: McpCall[], events: ActivityEvent[], f: TimelineFilter): TimelineItem[] {
  const items: TimelineItem[] = []
  if (f.show !== 'events') {
    for (const c of calls) {
      if (c.at <= f.since) continue
      if (f.writesOnly && !c.mutating) continue
      if (f.errorsOnly && c.ok) continue
      items.push({ type: 'call', at: c.at, call: c })
    }
  }
  if (f.show !== 'calls' && !f.writesOnly) {
    for (const e of events) {
      if (e.at <= f.since) continue
      if (f.errorsOnly && e.level !== 'error') continue
      items.push({ type: 'event', at: e.at, event: e })
    }
  }
  return items.sort((a, b) => b.at - a.at)
}

export type SecretSource = 'file' | 'env' | 'keyring' | 'dotenv' | 'command'

/** Split a reference into the form fields of the secret editor. */
export function secretRefFields(r: SecretRef): { source: SecretSource; location: string; key: string } {
  if ('file' in r) return { source: 'file', location: r.file, key: '' }
  if ('env' in r) return { source: 'env', location: r.env, key: '' }
  if ('keyring' in r) return { source: 'keyring', location: r.keyring, key: '' }
  if ('dotenv' in r) return { source: 'dotenv', location: r.dotenv.path, key: r.dotenv.key }
  return { source: 'command', location: r.command.join(' '), key: '' }
}

/**
 * Build a reference from the editor's fields. Commands are split on spaces
 * (quote-free argv: the value is never passed through a shell).
 */
export function makeSecretRef(source: SecretSource, location: string, key = ''): SecretRef | string {
  const loc = location.trim()
  if (!loc) return 'Enter where the secret lives'
  switch (source) {
    case 'file':
      return { file: loc }
    case 'env':
      return /^[A-Za-z_][A-Za-z0-9_]*$/.test(loc) ? { env: loc } : 'Not a valid environment variable name'
    case 'keyring':
      return loc.includes('/') ? { keyring: loc } : 'Use service/account'
    case 'dotenv':
      return key.trim() ? { dotenv: { path: loc, key: key.trim() } } : 'Enter the variable name in the .env file'
    case 'command':
      return { command: loc.split(/\s+/).filter(Boolean) }
  }
}

export const SECRET_NAME_RE = /^[A-Za-z0-9_.-]+$/

/** Human text for config keys that need a restart. */
export function restartText(keys: string[]): string {
  const names: Record<string, string> = { 'server.bind': 'bind address', 'server.tls': 'TLS certificate' }
  return keys.map((k) => names[k] ?? k).join(' and ')
}

/** `HH:MM:SS` (24 h) in local time. */
export function clock(ms: number): string {
  const d = new Date(ms)
  return [d.getHours(), d.getMinutes(), d.getSeconds()].map((n) => String(n).padStart(2, '0')).join(':')
}

/** 1234 → "1.2 s", 87 → "87 ms". */
export function formatMs(ms: number): string {
  if (ms < 1000) return `${ms} ms`
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)} s`
  return `${Math.floor(ms / 60_000)} m ${Math.round((ms % 60_000) / 1000)} s`
}

export function isLoopbackHost(hostname: string): boolean {
  return hostname === 'localhost' || hostname === '::1' || hostname === '[::1]' || hostname.startsWith('127.')
}

/**
 * Whether the server's desktop notifications reach whoever looks at this page: on its
 * own computer (a loopback host), unless the server's OS has none (`desktopUnsupported`,
 * the health report's reason: Windows). Then the browser notifies instead.
 */
export function desktopNotifiesHere(hostname: string, desktopUnsupported: string | null): boolean {
  return isLoopbackHost(hostname) && !desktopUnsupported
}
