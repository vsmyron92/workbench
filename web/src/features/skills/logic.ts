// Pure helpers of the skills views: filtering, grouping, the summary and the words. No React.

import type { AgentReport, CatalogEntry, Cli, InstallState, Issue, SkillInfo, SkillsReport } from './types'

export const CLI_LABEL: Record<Cli, string> = { claude: 'Claude Code', codex: 'Codex', gemini: 'Gemini CLI', kimi: 'Kimi Code' }

/** 1234 → "1.2k", 87 → "87". */
export function compact(n: number): string {
  if (n < 1000) return String(n)
  const k = n / 1000
  return `${k < 10 ? k.toFixed(1) : Math.round(k)}k`
}

/** "≈ 1.2k tokens": an estimate, four characters to a token. */
export function tokensText(n: number): string {
  return `≈ ${compact(n)} token${n === 1 ? '' : 's'}`
}

export function bytesText(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KB`
  return `${(n / 1024 / 1024).toFixed(1)} MB`
}

// ---------------------------------------------------------------- filtering

export interface Filters {
  query: string
  /** Show only skills one of these CLIs reads (empty: all). */
  clis: Cli[]
  /** Only skills with something to fix or look at. */
  attention: boolean
}

export const NO_FILTERS: Filters = { query: '', clis: [], attention: false }

/**
 * The filters as they apply. "To review" does nothing while no skill needs a review: its switch is not shown
 * then, so a filter left on could neither be seen nor turned off, and would hide every skill.
 */
export function effectiveFilters(f: Filters, toReview: number): Filters {
  return f.attention && toReview === 0 ? { ...f, attention: false } : f
}

export function isFiltering(f: Filters): boolean {
  return f.query.trim() !== '' || f.attention || f.clis.length > 0
}

export function needsAttention(s: SkillInfo): boolean {
  return s.issues.some((i) => i.level === 'error' || i.level === 'warn')
}

export function worstIssue(s: SkillInfo): Issue | null {
  return s.issues.find((i) => i.level === 'error') ?? s.issues.find((i) => i.level === 'warn') ?? s.issues[0] ?? null
}

/** Whether `s` passes the filters: every word of the query is in its names, description or place. */
export function matches(s: SkillInfo, f: Filters): boolean {
  if (f.clis.length && !s.agents.some((a) => f.clis.includes(a))) return false
  if (f.attention && !needsAttention(s)) return false
  const words = f.query.toLowerCase().split(/\s+/).filter(Boolean)
  if (!words.length) return true
  const hay = `${s.qualified} ${s.name} ${s.description} ${s.whenToUse ?? ''} ${s.path} ${s.scope} ${s.scopeLabel ?? ''} ${s.kind}`.toLowerCase()
  return words.every((w) => hay.includes(w))
}

// ---------------------------------------------------------------- grouping

export interface Group {
  key: string
  title: string
  /** Muted text after the title (a folder). */
  hint?: string
  /** Among groups of one kind: the order of the account it belongs to. */
  order: number
  skills: SkillInfo[]
}

const sep = /[\\/]/

/** The account whose folder holds `path` (both are written with `~`). */
export function accountOf(path: string, agents: AgentReport[]): AgentReport | undefined {
  return agents.find((a) => path.startsWith(a.folder) && (path.length === a.folder.length || sep.test(path[a.folder.length])))
}

function dirname(path: string): string {
  const i = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  return i > 0 ? path.slice(0, i) : path
}

/** Skills that are in no group of the list: Workbench's own are shown with its catalog. */
export function listed(skills: SkillInfo[]): SkillInfo[] {
  return skills.filter((s) => !s.workbench)
}

const ORDER = ['project', 'personal', 'plugin', 'synced', 'managed', 'system', 'extra', 'builtin']

/** The groups of the list, in reading order: the project's, the user's, plugins', claude.ai's, and the rest. */
export function groupSkills(skills: SkillInfo[], agents: AgentReport[]): Group[] {
  const shown = agents.filter((a) => a.installed || a.skills > 0)
  const many = shown.length > 1
  const groups = new Map<string, Group>()
  const put = (key: string, title: string, s: SkillInfo, hint?: string, order = 0) => {
    let g = groups.get(key)
    if (!g) groups.set(key, (g = { key, title, hint, order, skills: [] }))
    g.skills.push(s)
  }
  for (const s of skills) {
    switch (s.scope) {
      case 'project':
      case 'nested':
        put('project', 'Project', s)
        break
      case 'user': {
        const a = accountOf(s.path, agents)
        if (a) put(`personal:${a.id}`, many ? `Personal · ${a.label}` : 'Personal', s, a.folder, agents.indexOf(a))
        else put(`personal:${dirname(s.path)}`, 'Personal · shared by agents', s, dirname(s.path), agents.length)
        break
      }
      case 'plugin':
        put(`plugin:${s.scopeLabel ?? ''}`, `Plugin · ${s.scopeLabel ?? 'unknown'}`, s)
        break
      case 'synced':
        put('synced', 'claude.ai', s, 'synced skills')
        break
      case 'managed':
        put('managed', 'Organization', s)
        break
      case 'system':
        put('system', 'System', s, 'bundled with the CLI')
        break
      case 'extra':
        put('extra', 'Extra folders', s)
        break
      case 'builtin':
        put('builtin', 'Built in', s, 'Claude Code, in no folder')
        break
    }
  }
  const rank = (g: Group) => {
    const i = ORDER.indexOf(g.key.split(':')[0])
    return i < 0 ? ORDER.length : i
  }
  for (const g of groups.values()) g.skills.sort((a, b) => (a.kind === 'command' ? 1 : 0) - (b.kind === 'command' ? 1 : 0) || a.qualified.localeCompare(b.qualified))
  return [...groups.values()].sort((a, b) => rank(a) - rank(b) || a.order - b.order || a.title.localeCompare(b.title))
}

// ---------------------------------------------------------------- the summary

export interface AccountSummary {
  account: AgentReport
  /** Skills it can load. */
  count: number
  /** The estimated cost of its listing, or what its last session really received. */
  tokens: number
  /** `tokens` is what a session received, not an estimate. */
  measured: boolean
}

export interface Summary {
  /** Skills an agent can use. */
  usable: number
  /** Present but off, or hidden by another of the name. */
  inactive: number
  attention: number
  /** Skills that can act without asking: pre-approved tools, hooks, shell commands. */
  powerful: number
  rows: AccountSummary[]
}

export function isUsable(s: SkillInfo): boolean {
  return s.status === 'active' || s.status === 'name-only'
}

export function canActAlone(s: SkillInfo): boolean {
  return s.flags.preapproves.length > 0 || s.flags.hooks || s.flags.shell
}

export function summarize(r: SkillsReport): Summary {
  const own = listed(r.skills)
  const rows = r.agents
    .filter((a) => a.installed || a.skills > 0)
    .map((account) => ({
      account,
      count: account.skills,
      tokens: account.session?.tokens ?? account.listingTokens,
      measured: !!account.session,
    }))
  return {
    usable: r.skills.filter(isUsable).length,
    inactive: r.skills.filter((s) => !isUsable(s)).length,
    attention: own.filter(needsAttention).length,
    powerful: r.skills.filter((s) => isUsable(s) && canActAlone(s)).length,
    rows,
  }
}

// ---------------------------------------------------------------- words

export function scopeTitle(s: SkillInfo): string {
  const what = s.kind === 'command' ? 'command' : 'skill'
  switch (s.scope) {
    case 'project':
      return `Project ${what}`
    case 'nested':
      return `Nested ${what}${s.scopeLabel ? ` · ${s.scopeLabel}` : ''}`
    case 'user':
      return s.workbench ? 'Workbench skill, installed' : `Personal ${what}`
    case 'plugin':
      return `Plugin ${what}${s.scopeLabel ? ` · ${s.scopeLabel}` : ''}`
    case 'synced':
      return `claude.ai ${what} (synced)`
    case 'managed':
      return `Organization's ${what}`
    case 'system':
      return `System ${what}`
    case 'extra':
      return `Extra-folder ${what}`
    case 'builtin':
      return 'Built into Claude Code'
  }
}

/** A short word for what keeps a skill from being used as it is, or how it is used. */
export function statusWord(s: SkillInfo): { text: string; tone?: 'warning' | 'accent' } | null {
  switch (s.status) {
    case 'disabled':
      return { text: 'off', tone: 'warning' }
    case 'shadowed':
      return { text: 'hidden', tone: 'warning' }
    case 'name-only':
      return { text: 'name only' }
    default:
      return s.flags.manualOnly ? { text: 'manual' } : null
  }
}

export function invokeText(s: SkillInfo): string | null {
  return s.invoke.find((i) => i.agent === 'claude')?.text ?? s.invoke[0]?.text ?? null
}

/** A description on one line: whitespace collapsed, cut at a word when it is longer than `max` (CSS cuts what does not fit). */
export function oneLine(text: string, max = 160): string {
  const t = text.replace(/\s+/g, ' ').trim()
  if (t.length <= max) return t
  return `${t.slice(0, max).replace(/\s+\S*$/, '')}…`
}

// ---------------------------------------------------------------- Workbench's own skills

export interface InstallChoice {
  label: string
  /** The call it makes. */
  run: 'install' | 'remove'
  /** Replaces the user's edits. */
  force: boolean
  danger: boolean
}

/** What the button of a catalog entry's state does. `foreign`: nothing (the folder is someone else's). */
export function installChoices(state: InstallState): InstallChoice[] {
  switch (state) {
    case 'available':
      return [{ label: 'Install', run: 'install', force: false, danger: false }]
    case 'update':
      return [
        { label: 'Update', run: 'install', force: false, danger: false },
        { label: 'Remove', run: 'remove', force: false, danger: true },
      ]
    case 'installed':
      return [{ label: 'Remove', run: 'remove', force: false, danger: true }]
    case 'modified':
      return [
        { label: 'Restore', run: 'install', force: true, danger: true },
        { label: 'Remove', run: 'remove', force: true, danger: true },
      ]
    case 'foreign':
      return []
  }
}

export const INSTALL_WORD: Record<InstallState, { text: string; tone?: 'success' | 'warning' | 'accent' }> = {
  available: { text: 'not installed' },
  installed: { text: 'installed', tone: 'success' },
  update: { text: 'update', tone: 'accent' },
  modified: { text: 'edited', tone: 'warning' },
  foreign: { text: 'name taken', tone: 'warning' },
}

/** How many of the accounts offered have the skill (any state but not installed). */
export function installedCount(e: CatalogEntry): number {
  return e.installs.filter((i) => i.state !== 'available' && i.state !== 'foreign').length
}

/** What a catalog row says about the accounts it can be installed for, in one word. */
export function catalogState(e: CatalogEntry): { text: string; tone?: 'success' | 'warning' | 'accent' } | null {
  const n = e.installs.length
  if (n === 0) return { text: 'no agent here' }
  const states = e.installs.map((i) => i.state)
  if (states.includes('modified')) return INSTALL_WORD.modified
  if (states.includes('update')) return INSTALL_WORD.update
  const have = installedCount(e)
  if (have === 0) return states.includes('foreign') && n === 1 ? INSTALL_WORD.foreign : null
  return have === n ? INSTALL_WORD.installed : { text: `installed ${have} of ${n}`, tone: 'accent' }
}

/** Whether a catalog entry passes the filters: Workbench's skills are for the CLIs that get its tools, and have no problems. */
export function matchesCatalog(e: CatalogEntry, f: Filters): boolean {
  if (f.attention) return false
  if (f.clis.length && !f.clis.some((c) => c === 'claude' || c === 'codex')) return false
  const words = f.query.toLowerCase().split(/\s+/).filter(Boolean)
  const hay = `${e.name} ${e.description} workbench optional`.toLowerCase()
  return words.every((w) => hay.includes(w))
}

/**
 * A link in a skill's text that points at another file of the skill: its path within the skill, or
 * null for anything else (a URL, an anchor, an absolute path, a file the skill does not have, a
 * malformed escape). The text is untrusted, so nothing here throws.
 */
export function skillLink(from: string, href: string, known: Set<string>): string | null {
  if (!href || href.startsWith('#') || /^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith('/') || href.startsWith('\\')) return null
  let path: string
  try {
    path = decodeURI(href.split('#')[0].split('?')[0])
  } catch {
    return null
  }
  const out = from.includes('/') ? from.slice(0, from.lastIndexOf('/')).split('/') : []
  for (const part of path.split('/')) {
    if (part === '..') {
      // Climbing out of the skill's folder leads to nothing of the skill.
      if (!out.length) return null
      out.pop()
    } else if (part && part !== '.') out.push(part)
  }
  const joined = out.join('/')
  return known.has(joined) ? joined : null
}
