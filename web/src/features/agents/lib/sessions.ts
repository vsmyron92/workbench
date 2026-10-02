// Pure helpers over TerminalInfo: agent state presentation, attention ordering, cache
// updates. No React, no DOM — covered by sessions.test.ts.

import type { AgentInfo, TerminalInfo } from '@/api/types'

/** Visual tone of a terminal's state chip / dot. */
export type Tone = 'working' | 'attention' | 'error' | 'unread' | 'idle' | 'starting' | 'exited'

export function isRunning(t: TerminalInfo): boolean {
  return t.status !== 'exited'
}

/** Background processes an exited terminal left running (Kill ends them). */
export function lingering(t: TerminalInfo): number {
  return isRunning(t) ? 0 : (t.lingering ?? 0)
}

/**
 * Why the phone's compose box must not send now (mirrors `input_refusal` on the server):
 * text + Enter typed into an agent's dialog would answer it — Enter picks the highlighted
 * option, "Yes" on a permission prompt. Dialogs are answered with the keys bar (or, for a
 * Claude permission request, with Allow / Deny).
 */
export function composeBlocked(t: TerminalInfo): string | null {
  const a = t.agent
  if (!a || !isRunning(t)) return null
  switch (a.state) {
    case 'needs_permission':
      return a.pendingPermission
        ? 'The session is asking for permission. Allow or deny it above, or answer with the keys (↑ ↓ ⏎), then send.'
        : 'The session is asking for permission. Answer with the keys above (↑ ↓ ⏎), then send.'
    case 'needs_input':
      return 'The session is showing a question. Answer with the keys above (↑ ↓ ⏎), then send.'
    case 'starting':
      return 'The session is starting…'
    default:
      return null
  }
}

/** Something to kill: the process, or what it left running. */
export function canKill(t: TerminalInfo): boolean {
  return isRunning(t) || lingering(t) > 0
}

/**
 * How a terminal starts again (mirrors `restart_refusal` in server/src/terminals/mod.rs):
 * - `self`: POST /api/terminals/{id}/restart (agents, shells, log follows, Remote
 *   Control servers, commands whose spawner allows it);
 * - `run`: through the run configuration (apps tracks it), which makes a new terminal;
 * - `owner`: only where it came from — deploys and env commands re-check their gates and
 *   confirmation there.
 */
export type RestartMode = 'self' | 'run' | 'owner'

export function restartMode(t: TerminalInfo): RestartMode {
  if (t.kind === 'agent' || t.kind === 'shell') return 'self'
  const m = t.meta ?? {}
  if (t.kind === 'run') return typeof m.run === 'string' && t.projectId ? 'run' : 'owner'
  if (m.remoteControlServer === true || m.restartable === true || m.action === 'logs') return 'self'
  return 'owner'
}

export function tone(t: TerminalInfo): Tone {
  const a = t.agent
  if (!isRunning(t)) return 'exited'
  if (!a) return 'idle'
  switch (a.state) {
    case 'working':
      return 'working'
    case 'needs_permission':
    case 'needs_input':
      return 'attention'
    case 'error':
      return 'error'
    case 'starting':
      return 'starting'
    case 'exited':
      return 'exited'
    default:
      return a.unread ? 'unread' : 'idle'
  }
}

export function stateLabel(t: TerminalInfo): string {
  const a = t.agent
  if (!isRunning(t)) {
    const code = t.exit?.code
    return code === null || code === undefined || code === 0 || code === 129 ? 'Exited' : `Exited (${code})`
  }
  if (!a) return 'Running'
  switch (a.state) {
    case 'needs_permission':
      return 'Needs permission'
    case 'needs_input':
      return 'Needs input'
    case 'working':
      return 'Working'
    case 'error':
      return 'Error'
    case 'starting':
      return 'Starting'
    case 'exited':
      return 'Exited'
    default:
      return a.unread ? 'Done' : 'Idle'
  }
}

/** A running agent waiting on the user: a prompt, an error, or an unread answer. */
export function needsAttention(t: TerminalInfo): boolean {
  const a = t.agent
  if (!a || !isRunning(t)) return false
  return a.state === 'needs_permission' || a.state === 'needs_input' || a.state === 'error' || (a.state === 'idle' && a.unread)
}

/** Blocked on the user right now (not merely an unread answer). */
export function isBlocked(t: TerminalInfo): boolean {
  const a = t.agent
  return !!a && isRunning(t) && (a.state === 'needs_permission' || a.state === 'needs_input')
}

const RANK: Record<Tone, number> = { attention: 0, error: 1, unread: 2, working: 3, starting: 4, idle: 5, exited: 6 }

export function lastActivity(t: TerminalInfo): number {
  return Math.max(t.agent?.lastEventAt ?? 0, t.lastOutputAt, t.createdAt)
}

/** Attention first, then working, then idle; newest activity first within a group. */
export function sortSessions(list: TerminalInfo[]): TerminalInfo[] {
  return [...list].sort((a, b) => RANK[tone(a)] - RANK[tone(b)] || Number(b.pinned) - Number(a.pinned) || lastActivity(b) - lastActivity(a))
}

export function agentSessions(list: TerminalInfo[] | undefined, projectId: string | null, allProjects = false): TerminalInfo[] {
  return (list ?? []).filter((t) => t.kind === 'agent' && (allProjects || !projectId || t.projectId === projectId))
}

/**
 * Where a terminal opened on request is shown: the column of the project it belongs to.
 * Another project's terminal goes to that project's column (the UI switches to it), so
 * the columns never mix; one whose project is not listed any more, or that belongs to
 * none, is shown in the current column. `extra`: it is not that project's own open tab
 * (a closed session's saved screen, a project-less terminal), so it is added to the
 * column's extras. A terminal not in the list yet (just created, its event on the way)
 * is an extra of the current column until it arrives.
 */
export function tabPlace(t: TerminalInfo | undefined, currentProject: string | null, projectIds: readonly string[]): { project: string | null; extra: boolean } {
  if (!t) return { project: currentProject, extra: true }
  const home = t.projectId && projectIds.includes(t.projectId) ? t.projectId : currentProject
  return { project: home, extra: !(t.open && t.projectId === home) }
}

/**
 * The tabs of the agents column for a project: its open terminals (agent sessions,
 * shells, runs, commands) in a stable order, pinned first, then the `extras` opened on
 * request (a closed one's saved screen, a terminal of a project that is gone) in the
 * order they were opened. Terminals that no longer exist are left out, and so are the
 * `exclude`d ones (the Terminal tool window's shells).
 */
export function columnTabs(list: TerminalInfo[] | undefined, projectId: string | null, extras: string[] = [], exclude: string[] = []): TerminalInfo[] {
  const all = list ?? []
  const own = all
    .filter((t) => t.open && t.projectId === projectId && !exclude.includes(t.id))
    .sort((a, b) => Number(b.pinned) - Number(a.pinned) || a.order - b.order || a.createdAt - b.createdAt || a.id.localeCompare(b.id))
  const seen = new Set(own.map((t) => t.id))
  const more: TerminalInfo[] = []
  for (const id of extras) {
    const t = seen.has(id) ? undefined : all.find((x) => x.id === id)
    if (!t) continue
    seen.add(id)
    more.push(t)
  }
  return [...own, ...more]
}

/**
 * The tabs of the Terminal tool window for a project: the project's open terminals among
 * `ids` (the shells started there), in the order they were started. Ones that are gone
 * or closed are left out.
 */
export function bottomTabs(list: TerminalInfo[] | undefined, projectId: string | null, ids: string[]): TerminalInfo[] {
  const all = list ?? []
  const out: TerminalInfo[] = []
  for (const id of ids) {
    const t = all.find((x) => x.id === id)
    if (t && t.open && t.projectId === projectId) out.push(t)
  }
  return out
}

/** The tab to show once `closing` goes away: its right neighbour, else its left one, else the home tab (null). */
export function tabAfterClose(tabs: TerminalInfo[], closing: string): string | null {
  const i = tabs.findIndex((t) => t.id === closing)
  if (i === -1) return null
  return (tabs[i + 1] ?? tabs[i - 1])?.id ?? null
}

export interface Counts {
  attention: number
  working: number
  running: number
}

export function counts(list: TerminalInfo[] | undefined): Counts {
  const c: Counts = { attention: 0, working: 0, running: 0 }
  for (const t of list ?? []) {
    if (t.kind !== 'agent' || !isRunning(t)) continue
    c.running++
    if (needsAttention(t)) c.attention++
    if (t.agent?.state === 'working') c.working++
  }
  return c
}

/** The next session needing attention after `currentId` (wraps around). */
export function nextAttention(list: TerminalInfo[] | undefined, currentId: string | null): TerminalInfo | null {
  const waiting = sortSessions((list ?? []).filter(needsAttention))
  if (!waiting.length) return null
  const i = waiting.findIndex((t) => t.id === currentId)
  return waiting[(i + 1) % waiting.length]
}

// ---------------------------------------------------------------- cache updates

export function upsertTerminal(list: TerminalInfo[] | undefined, t: TerminalInfo): TerminalInfo[] {
  if (!list) return [t]
  const i = list.findIndex((x) => x.id === t.id)
  if (i === -1) return [...list, t]
  const next = list.slice()
  next[i] = t
  return next
}

export function removeTerminal(list: TerminalInfo[] | undefined, id: string): TerminalInfo[] {
  return (list ?? []).filter((x) => x.id !== id)
}

// ---------------------------------------------------------------- formatting

export function formatCost(usd: number | null | undefined): string | null {
  if (usd === null || usd === undefined || !Number.isFinite(usd)) return null
  if (usd > 0 && usd < 0.01) return '<$0.01'
  return `$${usd.toFixed(2)}`
}

export function formatContext(pct: number | null | undefined): string | null {
  if (pct === null || pct === undefined || !Number.isFinite(pct)) return null
  return `${Math.round(pct)}% ctx`
}

/** "Haiku 4.5 · low · 18% ctx · $0.03" pieces. */
export function agentMeta(a: AgentInfo | null | undefined): string[] {
  if (!a) return []
  return [a.model, a.effort, formatContext(a.contextPct), formatCost(a.costUsd)].filter((x): x is string => !!x)
}

export function terminalPanelId(id: string): string {
  return `terminal:${id}`
}

/** Tab colours: token names stored on the server, rendered through theme variables. */
export const TAB_COLORS: { id: string; label: string; css: string }[] = [
  { id: 'accent', label: 'Blue', css: 'var(--accent)' },
  { id: 'success', label: 'Green', css: 'var(--success)' },
  { id: 'warning', label: 'Yellow', css: 'var(--warning)' },
  { id: 'danger', label: 'Red', css: 'var(--danger)' },
  { id: 'renamed', label: 'Cyan', css: 'var(--vcs-renamed)' },
  { id: 'ignored', label: 'Olive', css: 'var(--vcs-ignored)' },
  { id: 'muted', label: 'Gray', css: 'var(--fg-subtle)' },
]

export function colorCss(color: string | null | undefined): string | undefined {
  if (!color) return undefined
  if (/^#[0-9a-f]{6}$/i.test(color)) return color
  return TAB_COLORS.find((c) => c.id === color)?.css
}

/** Quote a path for insertion at a prompt (shell-style single quotes when needed). */
export function quotePath(p: string): string {
  if (p && /^[A-Za-z0-9/._\-+,:@%]+$/.test(p)) return p
  return `'${p.replace(/'/g, `'\\''`)}'`
}
