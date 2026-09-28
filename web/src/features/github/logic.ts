// Pure helpers for the GitHub feature (unit-tested in logic.test.ts): states
// and labels, job log steps/groups and folding, diff-line mapping for review
// comments, the changed-files tree, matrix job groups, merge box state, the
// rate-limit line and polling intervals, agent prompts, and keyboard access
// for rows.

import type { KeyboardEvent } from 'react'
import type { Annotation, Job, PrFile, Pull, RateInfo, Run, Step, StepMark, Thread } from './types'

// ---------------------------------------------------------------- keyboard

/**
 * Make a clickable row or card focusable and open it with Enter or Space. Keys
 * pressed on a control inside it are left to that control.
 */
export function rowKeys(open: () => void) {
  return {
    tabIndex: 0,
    role: 'button' as const,
    onKeyDown: (e: Pick<KeyboardEvent, 'key' | 'target' | 'currentTarget' | 'preventDefault'>) => {
      if (e.target !== e.currentTarget) return
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault()
        open()
      }
    },
  }
}

// ---------------------------------------------------------------- states

export type Tone = 'success' | 'danger' | 'accent' | 'warning' | 'muted'

/** The run, job or check can still change. */
export function isActive(state: string | null | undefined): boolean {
  return state === 'running' || state === 'pending'
}

export function stateTone(state: string | null | undefined): Tone {
  switch (state) {
    case 'success':
      return 'success'
    case 'failed':
      return 'danger'
    case 'running':
      return 'accent'
    case 'pending':
    case 'manual':
      return 'warning'
    default:
      return 'muted'
  }
}

const STATE_LABELS: Record<string, string> = {
  success: 'passed',
  failed: 'failed',
  running: 'running',
  pending: 'queued',
  canceled: 'cancelled',
  skipped: 'skipped',
  manual: 'waiting',
}

/** A shared-vocabulary state in words (top bar, combined checks). */
export function stateLabel(state: string | null | undefined): string {
  if (!state) return 'no checks'
  return STATE_LABELS[state] ?? state.replace(/_/g, ' ')
}

/** GitHub's own words for a run, job, step or check. */
export function ghLabel(status: string | null | undefined, conclusion: string | null | undefined): string {
  if (status === 'completed') return (conclusion || 'completed').replace(/_/g, ' ')
  return (status || 'unknown').replace(/_/g, ' ')
}

/** Combine states the way the server does (a failure wins, then activity). */
export function aggregateState(states: readonly (string | null | undefined)[]): string | null {
  const order = ['failed', 'running', 'pending', 'manual', 'canceled', 'success', 'skipped']
  let best = -1
  for (const s of states) {
    const i = s ? order.indexOf(s) : -1
    if (i >= 0 && (best < 0 || i < best)) best = i
  }
  return best < 0 ? null : order[best]
}

export function shortSha(sha: string | null | undefined): string {
  return sha ? sha.slice(0, 7) : ''
}

/** Seconds a run or job has taken (live while it runs). */
export function elapsedSeconds(
  p: { duration: number | null; state: string; startedAt?: string | null; runStartedAt?: string | null },
  now: number = Date.now(),
): number | null {
  const start = p.runStartedAt ?? p.startedAt ?? null
  if (p.state === 'running' && start) {
    const t = Date.parse(start)
    if (Number.isFinite(t)) return Math.max(0, (now - t) / 1000)
  }
  return p.duration ?? null
}

/** Pretty event names. */
export function eventLabel(event: string): string {
  return event.replace(/_/g, ' ')
}

/** Runs newest first, deduplicated by id (merged pages and live updates). */
export function mergeRuns(pages: readonly (readonly Run[])[]): Run[] {
  const seen = new Set<number>()
  const out: Run[] = []
  for (const page of pages) {
    for (const r of page) {
      if (seen.has(r.id)) continue
      seen.add(r.id)
      out.push(r)
    }
  }
  return out
}

/** The state of `pull`, for icons: open, draft, merged or closed. */
export function prState(p: Pick<Pull, 'state' | 'draft' | 'merged' | 'mergedAt'>): 'open' | 'draft' | 'merged' | 'closed' {
  if (p.merged || p.mergedAt) return 'merged'
  if (p.state === 'closed') return 'closed'
  return p.draft ? 'draft' : 'open'
}

// ---------------------------------------------------------------- matrix groups

export interface JobGroup {
  name: string
  jobs: Job[]
  state: string
}

/** Group matrix jobs (`build (ubuntu, stable)`) under their base name, in order. */
export function groupJobs(jobs: readonly Job[]): JobGroup[] {
  const groups: JobGroup[] = []
  for (const j of jobs) {
    const m = /^(.*?) \((.+)\)$/.exec(j.name)
    const base = m ? m[1] : j.name
    let g = groups.find((x) => x.name === base)
    if (!g) groups.push((g = { name: base, jobs: [], state: '' }))
    g.jobs.push(j)
  }
  for (const g of groups) g.state = aggregateState(g.jobs.map((j) => j.state)) ?? 'skipped'
  return groups
}

// ---------------------------------------------------------------- merge box

export function mergeBox(p: Pull): { tone: Tone; label: string } {
  if (p.merged) return { tone: 'accent', label: 'Merged' }
  if (p.state === 'closed') return { tone: 'danger', label: 'Closed' }
  if (p.draft) return { tone: 'muted', label: 'Draft — mark it ready for review first' }
  switch (p.mergeableState) {
    case 'clean':
    case 'has_hooks':
      return { tone: 'success', label: 'Ready to merge' }
    case 'unstable':
      return { tone: 'warning', label: 'Some checks did not pass — merging is still allowed' }
    case 'blocked':
      return { tone: 'warning', label: 'Blocked: required reviews or checks are missing' }
    case 'behind':
      return { tone: 'warning', label: 'Behind the base branch — update it first' }
    case 'dirty':
      return { tone: 'danger', label: 'Merge conflicts' }
    default:
      return { tone: 'muted', label: 'Checking mergeability…' }
  }
}

/** The merge box can offer a merge now. */
export function canMergeNow(p: Pull): boolean {
  return !p.merged && p.state === 'open' && !p.draft && ['clean', 'unstable', 'has_hooks'].includes(p.mergeableState ?? '')
}

// ---------------------------------------------------------------- rate limit

export function rateText(r: RateInfo | null | undefined, nowSeconds: number = Date.now() / 1000): string | null {
  if (!r || r.remaining === null || r.limit === null) return null
  const mins = r.reset ? Math.max(1, Math.ceil((r.reset - nowSeconds) / 60)) : null
  return `${r.remaining}/${r.limit} requests left${mins !== null && r.reset! > nowSeconds ? ` · resets in ${mins} min` : ''}`
}

export function rateLow(r: RateInfo | null | undefined): boolean {
  return !!r && r.remaining !== null && r.limit !== null && r.remaining <= Math.max(5, r.limit * 0.1)
}

// ---------------------------------------------------------------- polling
//
// Without a token GitHub allows 60 requests an hour per IP, and a 304 costs one
// too. The server keeps anonymous answers for 5 minutes and polls viewed
// projects' runs itself (emitting `github.run`), so anonymous views refresh
// from events and the Refresh buttons, and the summary's timer is only a
// fallback slower than the server's cache.

/** Summary fallback refresh (events keep it fresh otherwise). */
export const SUMMARY_POLL_MS = { idle: 120_000, active: 20_000, anonymous: 600_000 }

/** An error a timer cannot fix: the repository is missing, or it needs a token. */
export function isSetupError(e: unknown): boolean {
  if (!e || typeof e !== 'object') return false
  const { status, code } = e as { status?: unknown; code?: unknown }
  return code === 'not_configured' || status === 404 || status === 412
}

/** The summary's refetch interval. */
export function summaryPoll(
  s: { auth: { authenticated: boolean }; branchRun?: { state: string } | null; headStatus?: { status: string } | null } | undefined,
  error: unknown,
): number | false {
  if (!s && isSetupError(error)) return false
  if (s && !s.auth.authenticated) return SUMMARY_POLL_MS.anonymous
  return s && (isActive(s.branchRun?.state) || isActive(s.headStatus?.status)) ? SUMMARY_POLL_MS.active : SUMMARY_POLL_MS.idle
}

/** Detail views poll while something moves, and only with a token. */
export function livePoll(anonymous: boolean, active: boolean, ms: number): number | false {
  return active && !anonymous ? ms : false
}

// ---------------------------------------------------------------- job logs

// eslint-disable-next-line no-control-regex
const ANSI = /\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]/g

export function stripAnsi(s: string): string {
  return s.replace(ANSI, '').replace(/\r/g, '')
}

export interface LogSection {
  /** Order of appearance. */
  id: number
  kind: 'step' | 'group'
  title: string
  depth: number
  collapsedByDefault: boolean
  /** Index of the header line in `ParsedLog.lines`. */
  line: number
  /** A group that only repeats its step's title (`Run x` inside step `Run x`). */
  echo?: boolean
  /** Steps: number, state and duration. */
  step?: number
  state?: string
  duration?: number | null
}

interface ParsedLine {
  text: string
  /** Section this line opens (a header), if any. */
  opens: number | null
  /** `##[endgroup]`: nothing visible. */
  markerOnly: boolean
  /** Sections enclosing this line (for a header: its parents). */
  parents: number[]
}

export interface ParsedLog {
  lines: ParsedLine[]
  sections: LogSection[]
}

function stepSeconds(s: Step): number | null {
  if (!s.startedAt || !s.completedAt) return null
  const a = Date.parse(s.startedAt)
  const b = Date.parse(s.completedAt)
  return Number.isFinite(a) && Number.isFinite(b) ? Math.max(0, (b - a) / 1000) : null
}

/**
 * Split a job log into lines, steps (from the server's step marks) and
 * `##[group]` sections inside them. Steps that failed start unfolded; other
 * steps and all groups start folded, like GitHub's own view.
 */
export function parseGhLog(text: string, steps: readonly Step[] = [], marks: readonly StepMark[] = []): ParsedLog {
  const sections: LogSection[] = []
  const lines: ParsedLine[] = []
  const markAt = new Map<number, number>()
  for (const m of marks) markAt.set(m.line, m.number)
  const hasSteps = marks.length > 0
  let step: number | null = null
  const groups: number[] = []
  const raw = text.split('\n')
  for (let i = 0; i < raw.length; i++) {
    const n = markAt.get(i)
    if (n !== undefined) {
      groups.length = 0
      const s = steps.find((x) => x.number === n)
      const id = sections.length
      sections.push({
        id,
        kind: 'step',
        title: s?.name ?? `Step ${n}`,
        depth: 0,
        collapsedByDefault: s?.state !== 'failed',
        line: lines.length,
        step: n,
        state: s?.state,
        duration: s ? stepSeconds(s) : null,
      })
      lines.push({ text: '', opens: id, markerOnly: false, parents: [] })
      step = id
    }
    const line = raw[i]
    const parents = [...(step !== null ? [step] : []), ...groups]
    if (line.startsWith('##[group]')) {
      const id = sections.length
      const title = stripAnsi(line.slice(9)).trim() || 'group'
      sections.push({
        id,
        kind: 'group',
        title,
        depth: (hasSteps && step !== null ? 1 : 0) + groups.length,
        collapsedByDefault: true,
        line: lines.length,
        echo: step !== null && groups.length === 0 && sections[step].title === title,
      })
      lines.push({ text: line.slice(9), opens: id, markerOnly: false, parents })
      groups.push(id)
    } else if (line.startsWith('##[endgroup]')) {
      groups.pop()
      lines.push({ text: '', opens: null, markerOnly: true, parents })
    } else {
      lines.push({ text: line, opens: null, markerOnly: false, parents })
    }
  }
  return { lines, sections }
}

export function isCollapsed(s: LogSection, overrides: ReadonlyMap<number, boolean>): boolean {
  return overrides.get(s.id) ?? s.collapsedByDefault
}

const STEP_MARK: Record<string, string> = { success: '✓', failed: '✗', canceled: '⊘', skipped: '⤼', running: '●', pending: '○', manual: '◐' }
const SGR_FOR_STATE: Record<string, string> = { success: '32', failed: '31', running: '34', pending: '33', manual: '33' }

/** A header line as plain text (what the viewer shows and search finds). */
export function headerText(s: LogSection, folded: boolean): string {
  const chev = folded ? '▸' : '▾'
  return s.kind === 'step' ? `${chev} ${STEP_MARK[s.state ?? ''] ?? '·'} ${s.title}` : `${chev} ${s.title}`
}

function formatSecs(s: number | null | undefined): string {
  if (s === null || s === undefined) return ''
  const r = Math.round(s)
  if (r < 60) return `${r}s`
  const m = Math.floor(r / 60)
  return m < 60 ? `${m}m ${r % 60}s` : `${Math.floor(m / 60)}h ${m % 60}m`
}

/** One log line with GitHub's message markers turned into coloured words. */
export function renderLine(text: string): string {
  if (text.startsWith('##[error]')) return `\x1b[31mError: ${text.slice(9)}\x1b[39m`
  if (text.startsWith('##[warning]')) return `\x1b[33mWarning: ${text.slice(11)}\x1b[39m`
  if (text.startsWith('##[notice]')) return `\x1b[36mNotice: ${text.slice(10)}\x1b[39m`
  if (text.startsWith('##[command]')) return `\x1b[34m${text.slice(11)}\x1b[39m`
  if (text.startsWith('##[debug]')) return `\x1b[2m${text.slice(9)}\x1b[22m`
  if (text.startsWith('##[section]')) return `\x1b[1m${text.slice(11)}\x1b[22m`
  return text
}

/** The text to show: headers with fold chevrons, folded bodies left out. */
export function renderGhLog(parsed: ParsedLog, overrides: ReadonlyMap<number, boolean>): string {
  const folded = new Set(parsed.sections.filter((s) => isCollapsed(s, overrides)).map((s) => s.id))
  const out: string[] = []
  for (const l of parsed.lines) {
    if (l.markerOnly) continue
    if (l.parents.some((p) => folded.has(p))) continue
    if (l.opens !== null) {
      const s = parsed.sections[l.opens]
      const plain = headerText(s, folded.has(s.id))
      if (s.kind === 'step') {
        const sgr = SGR_FOR_STATE[s.state ?? ''] ?? '2'
        const [chev, mark, ...rest] = plain.split(' ')
        const dur = formatSecs(s.duration)
        out.push(`\x1b[2m${chev}\x1b[22m \x1b[${sgr}m${mark}\x1b[39m \x1b[1m${rest.join(' ')}\x1b[22m${dur ? `  \x1b[2m${dur}\x1b[22m` : ''}`)
      } else {
        out.push(`\x1b[2m${plain.slice(0, 1)}\x1b[22m${plain.slice(1)}`)
      }
    } else {
      out.push(renderLine(l.text))
    }
  }
  return out.join('\n')
}

/** The last `n` lines of text. */
export function lastLines(text: string, n: number): string {
  const lines = text.split('\n')
  return lines.slice(Math.max(0, lines.length - n)).join('\n')
}

// ---------------------------------------------------------------- diff comments

export interface DiffLines {
  /** Old-side line numbers in the diff (removed and context lines). */
  left: Set<number>
  /** New-side line numbers in the diff (added and context lines). */
  right: Set<number>
}

/** Lines GitHub accepts review comments on: those inside the patch's hunks. */
export function diffLines(patch: string | null | undefined): DiffLines {
  const left = new Set<number>()
  const right = new Set<number>()
  let o = 0
  let n = 0
  let inHunk = false
  for (const line of (patch ?? '').split('\n')) {
    const h = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line)
    if (h) {
      o = Number(h[1])
      n = Number(h[2])
      inHunk = true
      continue
    }
    if (!inHunk || line === '' || line.startsWith('\\')) continue
    if (line.startsWith('+')) right.add(n++)
    else if (line.startsWith('-')) left.add(o++)
    else {
      left.add(o++)
      right.add(n++)
    }
  }
  return { left, right }
}

/** Where a comment on a clicked line goes, or null when GitHub would refuse it. */
export function commentTarget(lines: DiffLines, side: 'original' | 'modified', line: number): { line: number; side: 'LEFT' | 'RIGHT' } | null {
  if (side === 'modified') return lines.right.has(line) ? { line, side: 'RIGHT' } : null
  return lines.left.has(line) ? { line, side: 'LEFT' } : null
}

/** Threads of one file that can be placed on a line (not outdated). */
export function placedThreads(threads: readonly Thread[], path: string): { thread: Thread; side: 'LEFT' | 'RIGHT'; line: number }[] {
  const out: { thread: Thread; side: 'LEFT' | 'RIGHT'; line: number }[] = []
  for (const t of threads) {
    if (t.path !== path || t.outdated || t.line === null) continue
    out.push({ thread: t, side: t.side === 'LEFT' ? 'LEFT' : 'RIGHT', line: t.line })
  }
  return out
}

// ---------------------------------------------------------------- changed files tree

export interface FileRow {
  kind: 'dir' | 'file'
  path: string
  name: string
  depth: number
  file?: PrFile
}

interface DirNode {
  dirs: Map<string, DirNode>
  files: PrFile[]
}

const baseName = (f: PrFile) => f.filename.slice(f.filename.lastIndexOf('/') + 1)

/** CLion-style changes tree: directories first (single-child chains compacted), then files. */
export function fileRows(files: readonly PrFile[], collapsed: ReadonlySet<string> = new Set()): FileRow[] {
  const root: DirNode = { dirs: new Map(), files: [] }
  for (const f of files) {
    let node = root
    for (const dir of f.filename.split('/').slice(0, -1)) {
      let next = node.dirs.get(dir)
      if (!next) node.dirs.set(dir, (next = { dirs: new Map(), files: [] }))
      node = next
    }
    node.files.push(f)
  }
  const rows: FileRow[] = []
  const walk = (node: DirNode, prefix: string, depth: number) => {
    for (const name of [...node.dirs.keys()].sort()) {
      let child = node.dirs.get(name)!
      let label = name
      let path = prefix ? `${prefix}/${name}` : name
      while (child.files.length === 0 && child.dirs.size === 1) {
        const [only, next] = [...child.dirs.entries()][0]
        label += `/${only}`
        path += `/${only}`
        child = next
      }
      rows.push({ kind: 'dir', path, name: label, depth })
      if (!collapsed.has(path)) walk(child, path, depth + 1)
    }
    for (const f of [...node.files].sort((a, b) => baseName(a).localeCompare(baseName(b))))
      rows.push({ kind: 'file', path: f.filename, name: baseName(f), depth, file: f })
  }
  walk(root, '', 0)
  return rows
}

/** CLion change letter: A(dded), D(eleted), R(enamed) or M(odified). */
export function changeKind(f: Pick<PrFile, 'status'>): 'A' | 'D' | 'R' | 'M' {
  if (f.status === 'added') return 'A'
  if (f.status === 'removed') return 'D'
  if (f.status === 'renamed') return 'R'
  return 'M'
}

// ---------------------------------------------------------------- prompts

export function jobFixPrompt(o: {
  repo: string
  job: Pick<Job, 'id' | 'name' | 'runId' | 'headSha' | 'headBranch' | 'steps'>
  runName?: string | null
  log: string | null
  shownLines: number
  totalLines: number
  annotations?: readonly Annotation[]
}): string {
  const failedSteps = o.job.steps.filter((s) => s.state === 'failed').map((s) => s.name)
  const where = `${o.runName ? `workflow "${o.runName}", ` : ''}run ${o.job.runId}${o.job.headBranch ? ` on ${o.job.headBranch}` : ''} @ ${shortSha(o.job.headSha)}`
  const parts = [
    `The GitHub Actions job "${o.job.name}" (job id ${o.job.id}) of ${o.repo} failed (${where}).${failedSteps.length ? ` Failed step: ${failedSteps.join(', ')}.` : ''}`,
    '',
  ]
  if (o.log !== null) {
    parts.push(`Last ${o.shownLines} of ${o.totalLines} log lines:`, '```', o.log.replace(/```/g, "'''"), '```', '')
  }
  if (o.annotations?.length) {
    parts.push('Annotations:')
    for (const a of o.annotations.slice(0, 30))
      parts.push(`- ${a.annotationLevel} ${a.path}${a.startLine ? `:${a.startLine}` : ''}: ${a.message.split('\n')[0]}`)
    parts.push('')
  }
  parts.push(
    `Find the root cause and fix it in the code. For more of the log use the github_job_log tool (jobId ${o.job.id}, tailLines up to 2000); github_run_jobs (runId ${o.job.runId}) lists the run's other jobs.`,
  )
  return parts.join('\n')
}

export function prReviewPrompt(o: { repo: string; number: number; title: string; head: string | null; base: string | null; sha: string | null }): string {
  return [
    `Review GitHub pull request #${o.number} "${o.title}" in ${o.repo} (${o.head ?? '?'} → ${o.base ?? '?'}${o.sha ? `, head ${shortSha(o.sha)}` : ''}).`,
    `Use the github_pr tool (number ${o.number}) for the description, checks and review threads, and github_pr_diff for the changes (pass path to read one file).`,
    'Report correctness problems, risky changes and missing tests, citing file:line. Do not post anything on GitHub unless I ask; if I do, use github_pr_comment.',
  ].join('\n')
}

/** Initials for an avatar badge. */
export function initials(u: { name?: string | null; login?: string } | null | undefined): string {
  const n = (u?.name || u?.login || '?').trim()
  const parts = n.split(/\s+/).filter(Boolean)
  const s = parts.length > 1 ? parts[0][0] + parts[parts.length - 1][0] : n.slice(0, 2)
  return s.toUpperCase()
}

/** "feature/add-thing_now" → "Add thing now" */
export function titleFromBranch(branch: string): string {
  const last = branch.split('/').pop() ?? branch
  const words = last.replace(/[-_]+/g, ' ').trim()
  return words ? words[0].toUpperCase() + words.slice(1) : branch
}

/** When an artifact expires, from now: "in 90 days", "in 5 hours", "within an hour". */
export function expiresIn(iso: string, now = Date.now()): string {
  const ms = Date.parse(iso) - now
  if (Number.isNaN(ms)) return ''
  if (ms <= 0) return 'expired'
  const hours = ms / 3_600_000
  if (hours < 1) return 'within an hour'
  if (hours < 48) return `in ${Math.floor(hours)} hours`
  return `in ${Math.floor(hours / 24)} days`
}
