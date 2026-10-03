// Pure helpers for the platform feature (tested in lib.test.ts).

import type { ActivityEvent, McpCall, SecretRef, UpdateStatus } from './types'

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

/**
 * `parts` under the config dir the server reports (`GET /api/settings` `paths.configDir`,
 * `~`-contracted, with the server's separator): `~/.config/workbench/tls/cert.pem` on
 * Linux, `~\AppData\Roaming\workbench\tls\cert.pem` on Windows.
 */
export function inConfigDir(configDir: string | undefined, ...parts: string[]): string {
  const dir = configDir || '~/.config/workbench'
  const sep = dir.includes('\\') ? '\\' : '/'
  return [dir.replace(/[\\/]+$/, ''), ...parts].join(sep)
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

const MB = 1024 * 1024

/** What an update is doing right now, or null while nothing runs. */
export function updatePhaseText(phase: UpdateStatus['phase'], progress: UpdateStatus['progress'], version?: string | null): string | null {
  switch (phase) {
    case 'checking':
      return 'Looking for a newer release…'
    case 'downloading': {
      const what = version ? `Downloading ${version}` : 'Downloading'
      if (!progress || progress.total <= 0) return `${what}…`
      return `${what}: ${(progress.received / MB).toFixed(1)} of ${(progress.total / MB).toFixed(1)} MB`
    }
    case 'verifying':
      return 'Checking the download (SHA-256)…'
    case 'installing':
      return 'Installing…'
    case 'restarting':
      return 'Restarting Workbench…'
    default:
      return null
  }
}

/** How far the download is, 0 to 1; null when there is nothing to measure. */
export function updateFraction(phase: UpdateStatus['phase'], progress: UpdateStatus['progress']): number | null {
  if (phase !== 'downloading' || !progress || progress.total <= 0) return null
  return Math.min(1, Math.max(0, progress.received / progress.total))
}

/**
 * What restarting the server costs, for the confirmation: `terminals` are the running
 * ones (`working`: an agent in the middle of a turn), `restore` is
 * `agents.restore_on_start`. As the server restores them (`terminals::agent::restore`):
 * agent sessions resume, shells start again under their last screen, runs do not.
 */
export function restartImpact(terminals: { kind: 'agent' | 'shell' | 'run' | 'command'; working: boolean }[], restore: boolean): string {
  if (!terminals.length) return 'Nothing is running in its terminals. Workbench is back in a few seconds.'
  const agents = terminals.filter((t) => t.kind === 'agent').length
  const shells = terminals.filter((t) => t.kind === 'shell').length
  const runs = terminals.length - agents - shells
  const working = terminals.filter((t) => t.working).length
  const count = (n: number, one: string) => `${n} ${one}${n === 1 ? '' : 's'}`
  const list = (items: string[]) => (items.length > 1 ? `${items.slice(0, -1).join(', ')} and ${items[items.length - 1]}` : items.join(''))
  const stopping = [agents && count(agents, 'agent session'), shells && count(shells, 'shell'), runs && count(runs, 'run')].filter((s): s is string => !!s)
  const after = [
    agents && (restore ? 'agent sessions resume after the restart' : 'agent sessions are not resumed (agents.restore_on_start is off) but stay in the history'),
    shells && 'shells start again under their last screen, without what ran in them',
    runs && 'runs are not started again',
  ].filter((s): s is string => !!s)
  const now = working ? ` (${working === 1 ? '1 agent is' : `${working} agents are`} working right now)` : ''
  const then = after.join('; ')
  return `${list(stopping)} will stop${now}. ${then[0].toUpperCase()}${then.slice(1)}.`
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

/**
 * The CLIs that can run under more than one login, with the variable that selects it.
 * Most keep a login in a folder of their own; Gemini CLI's variable names the folder that
 * holds its `.gemini`, and Aider has no login at all, only API keys in a `.env` file.
 */
export const ACCOUNT_KINDS = [
  { kind: 'claude', label: 'Claude Code', homeVar: 'CLAUDE_CONFIG_DIR', file: false, defaultHome: '~/.claude', suggest: (n: string) => `~/.claude-${n}` },
  { kind: 'codex', label: 'Codex', homeVar: 'CODEX_HOME', file: false, defaultHome: '~/.codex', suggest: (n: string) => `~/.codex-${n}` },
  { kind: 'kimi', label: 'Kimi Code', homeVar: 'KIMI_CODE_HOME', file: false, defaultHome: '~/.kimi', suggest: (n: string) => `~/.kimi-${n}` },
  { kind: 'gemini', label: 'Gemini CLI', homeVar: 'GEMINI_CLI_HOME', file: false, defaultHome: '~', suggest: (n: string) => `~/.gemini-${n}` },
  { kind: 'aider', label: 'Aider', homeVar: 'AIDER_ENV_FILE', file: true, defaultHome: null, suggest: (n: string) => `~/.aider-${n}.env` },
] as const

export type AccountKind = (typeof ACCOUNT_KINDS)[number]['kind']

/** The names `[agents.providers]` gives to the built-in CLIs: an account cannot take one. */
const PRESET_IDS = ACCOUNT_KINDS.map((k) => k.kind as string)

export const accountKind = (kind: string | null | undefined) => ACCOUNT_KINDS.find((k) => k.kind === kind)

/** What the account's location is called: a folder, or Aider's keys file. */
export const homeNoun = (kind: AccountKind): string => (accountKind(kind)!.file ? 'keys file' : 'folder')

/** `claude` + "Work" → `claude-work`: a provider name (lowercase letters, digits, - and _). */
export function suggestAccountId(kind: AccountKind, label: string): string {
  const slug = label
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
  return (slug ? `${kind}-${slug}` : '').slice(0, 32).replace(/-+$/, '')
}

/** Why `id` cannot name a new account (`null`: it can). Mirrors the server's `valid_provider_id`. */
export function accountIdError(id: string, existing: string[]): string | null {
  if (!id) return 'Enter a name'
  if (!/^[a-z0-9][a-z0-9_-]{0,31}$/.test(id)) return 'Lowercase letters, digits, - and _ (at most 32)'
  if (PRESET_IDS.includes(id)) return `“${id}” is the built-in ${id}: choose another name`
  if (existing.includes(id)) return 'An account with this name exists'
  return null
}

const sameFolder = (a: string, b: string) => a.trim().replace(/[\\/]+$/, '') === b.trim().replace(/[\\/]+$/, '')

/**
 * Why `home` cannot be the folder (Aider: the keys file) of an account of `kind` (`null`: it
 * can). Two accounts in one place are one login, and the default place is the account the CLI
 * uses outside Workbench.
 */
export function accountHomeError(kind: AccountKind, home: string, others: { id: string; home: string }[]): string | null {
  const k = accountKind(kind)!
  const noun = homeNoun(kind)
  const h = home.trim()
  if (!h) return k.file ? 'Enter the .env file that holds this account’s API keys' : 'Enter the folder that holds this account’s login'
  if (!/^(~[\\/]|[\\/]|[A-Za-z]:[\\/])/.test(h)) return 'Use an absolute path, or one starting with ~/'
  if (h.includes('${')) return 'A plain path: no ${…} references'
  if (k.file && /[\\/]$/.test(h)) return 'A file, not a folder'
  if (k.defaultHome !== null && sameFolder(h, k.defaultHome)) return `${k.defaultHome} is the default account: it is already listed as “${k.label}”`
  const clash = others.find((o) => sameFolder(o.home, h))
  return clash ? `Already the ${noun} of “${clash.id}”` : null
}

// ---------------------------------------------------------------- local models

/** The model servers an agent CLI can run on (`[agents.providers.<name>.local]`), by CLI. */
export const LOCAL_SERVERS: Record<string, { label: string; url: string; hint: string }> = {
  ollama: { label: 'Ollama', url: 'http://localhost:11434', hint: 'Ollama 0.14 or newer for Claude Code, 0.13.4 for Codex. It recommends a context of 64k or more.' },
  lmstudio: { label: 'LM Studio', url: 'http://localhost:1234', hint: 'Start its local server (LM Studio 0.4.1 or newer for Claude Code).' },
  openai: { label: 'OpenAI-compatible', url: '', hint: 'llama.cpp’s llama-server, vLLM and the like. Codex needs one that serves /v1/responses.' },
  anthropic: { label: 'Anthropic-compatible', url: '', hint: 'A server or gateway with the Anthropic Messages API (/v1/messages).' },
}

export const LOCAL_BY_KIND: Record<AccountKind, string[]> = {
  claude: ['ollama', 'lmstudio', 'anthropic'],
  codex: ['ollama', 'lmstudio', 'openai'],
  aider: ['ollama', 'lmstudio', 'openai'],
  kimi: [],
  gemini: [],
}

/** Why `url` cannot be a model server's address (`null`: it can; empty is the server's usual one). */
export function localUrlError(server: string, url: string): string | null {
  const u = url.trim()
  if (!u) return LOCAL_SERVERS[server]?.url ? null : 'Enter the address of the server'
  if (!/^https?:\/\/[^\s/?#@]+(?:[/?#]\S*)?$/i.test(u)) return 'http:// or https:// and an address, without a user name or password'
  return null
}

/** `claude` + `ollama` + `qwen3-coder:30b` → `claude-ollama-qwen3-coder`: a provider name (the model without its tag). */
export function suggestLocalId(kind: AccountKind, server: string, model: string): string {
  const slug = (t: string) =>
    t
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '')
  return [kind, server, slug(model.split(':')[0])].filter(Boolean).join('-').slice(0, 32).replace(/-+$/, '')
}

// ---------------------------------------------------------------- usage and fallback

const PRESET_LABEL: Record<string, string> = { claude: 'Claude Code', codex: 'Codex', kimi: 'Kimi Code', gemini: 'Gemini CLI', aider: 'Aider' }

/** What an account is called in lists: `Claude · Work` for an account, the CLI's name for a built-in. */
export function accountName(id: string, providers: Record<string, { kind?: string | null; label?: string | null }>): string {
  const c = providers[id]
  const base = PRESET_LABEL[id]
  const label = c?.label?.trim()
  if (base && !label) return base
  if (!c) return base ?? id
  const kindLabel = accountKind(c.kind)?.label
  const text = label || id
  if (!kindLabel || text.toLowerCase().includes(kindLabel.split(' ')[0].toLowerCase())) return text
  return `${kindLabel.split(' ')[0]} · ${text}`
}

/** The accounts `id` may fall back to: every other configured one and the built-in CLIs. */
export function fallbackCandidates(id: string, providers: Record<string, { kind?: string | null; label?: string | null; enabled?: boolean | null }>): { id: string; label: string }[] {
  const ids = [...new Set([...Object.keys(PRESET_LABEL), ...Object.keys(providers)])]
  return ids.filter((x) => x !== id && providers[x]?.enabled !== false).map((x) => ({ id: x, label: accountName(x, providers) }))
}

/** `list` with the item at `from` moved by `by` places (a no-op at the ends). */
export function moveItem<T>(list: T[], from: number, by: number): T[] {
  const to = from + by
  if (from < 0 || from >= list.length || to < 0 || to >= list.length) return list
  const out = [...list]
  const [x] = out.splice(from, 1)
  out.splice(to, 0, x)
  return out
}

/** "3:45 PM" today, "Mon 12:00 AM" within the week, else "Oct 9, 3:00 PM". */
export function formatUntil(ms: number, now: number = Date.now()): string {
  const d = new Date(ms)
  const time = d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
  const sameDay = d.toDateString() === new Date(now).toDateString()
  if (sameDay) return time
  if (ms - now < 6 * 24 * 3600 * 1000) return `${d.toLocaleDateString([], { weekday: 'short' })} ${time}`
  return `${d.toLocaleDateString([], { month: 'short', day: 'numeric' })}, ${time}`
}

/** The colour of a usage bar. */
export function usageTone(pct: number): 'ok' | 'warn' | 'full' {
  return pct >= 99.5 ? 'full' : pct >= 80 ? 'warn' : 'ok'
}

/** Why `text` cannot be a context window in tokens (`null`: it can; empty is "not set"). Mirrors the server's range. */
export function contextError(text: string): string | null {
  const t = text.trim().replace(/[_,\s]/g, '')
  if (!t) return null
  const m = /^(\d+(?:\.\d+)?)([kKmM]?)$/.exec(t)
  if (!m) return 'A number of tokens, such as 32768 or 32k'
  const n = parseContext(text)
  return n !== null && n >= 2048 && n <= 10_000_000 ? null : 'Between 2048 and 10 000 000 tokens'
}

/** "32k" → 32768, "128000" → 128000, "1m" → 1048576; `null` when empty or not a number. */
export function parseContext(text: string): number | null {
  const t = text.trim().replace(/[_,\s]/g, '')
  const m = /^(\d+(?:\.\d+)?)([kKmM]?)$/.exec(t)
  if (!m) return null
  const unit = m[2].toLowerCase() === 'k' ? 1024 : m[2].toLowerCase() === 'm' ? 1024 * 1024 : 1
  return Math.round(Number(m[1]) * unit)
}
