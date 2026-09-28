// Pure helpers for the GitLab feature (unit-tested in logic.test.ts): status
// classification, job log sections and folding, diff-line mapping for review
// comments, the changed-files tree, agent prompts, and keyboard access for rows.

import type { KeyboardEvent } from 'react'
import type { Job, MrDiffFile, Pipeline } from './types'

// ---------------------------------------------------------------- keyboard

/**
 * Make a clickable row or card focusable and open it with Enter or Space. Keys
 * pressed on a control inside it (a job's Retry button, a link) are left to
 * that control.
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

// ---------------------------------------------------------------- statuses

export type Tone = 'success' | 'danger' | 'accent' | 'warning' | 'muted'

const ACTIVE = new Set(['created', 'waiting_for_resource', 'preparing', 'pending', 'running', 'scheduled', 'waiting_for_callback'])
const FINISHED = new Set(['success', 'failed', 'canceled', 'skipped', 'manual'])

/** The pipeline or job can still change. */
export function isActive(status: string | null | undefined): boolean {
  return !!status && ACTIVE.has(status)
}

/** A job's log no longer grows. */
export function isFinished(status: string | null | undefined): boolean {
  return !!status && FINISHED.has(status)
}

export function statusTone(status: string | null | undefined): Tone {
  switch (status) {
    case 'success':
      return 'success'
    case 'failed':
      return 'danger'
    case 'running':
      return 'accent'
    case 'pending':
    case 'waiting_for_resource':
    case 'preparing':
    case 'success_with_warnings':
    case 'scheduled':
      return 'warning'
    default:
      return 'muted'
  }
}

const LABELS: Record<string, string> = {
  success: 'passed',
  success_with_warnings: 'passed with warnings',
  failed: 'failed',
  running: 'running',
  pending: 'pending',
  created: 'created',
  waiting_for_resource: 'waiting for resource',
  preparing: 'preparing',
  canceled: 'canceled',
  canceling: 'canceling',
  skipped: 'skipped',
  manual: 'manual',
  scheduled: 'scheduled',
}

export function statusLabel(status: string | null | undefined): string {
  if (!status) return 'no pipeline'
  return LABELS[status] ?? status.replace(/_/g, ' ')
}

const MERGE_STATUS: Record<string, string> = {
  mergeable: 'Ready to merge',
  not_open: 'Not open',
  ci_must_pass: 'Pipeline must succeed',
  ci_still_running: 'Pipeline still running',
  draft_status: 'Draft — mark as ready first',
  discussions_not_resolved: 'Unresolved threads',
  not_approved: 'Approval required',
  need_rebase: 'Needs a rebase',
  conflict: 'Merge conflicts',
  checking: 'Checking mergeability…',
  unchecked: 'Checking mergeability…',
  preparing: 'Preparing…',
  approvals_syncing: 'Syncing approvals…',
  blocked_status: 'Blocked by another merge request',
  external_status_checks: 'External status checks pending',
  jira_association_missing: 'Needs a Jira issue key',
  requested_changes: 'Changes requested',
  security_policy_violations: 'Security policy violations',
  commits_status: 'Source branch missing or has no commits',
  merge_request_blocked: 'Blocked',
  merge_time: 'Scheduled merge time not reached',
  locked_paths: 'Paths are locked',
  locked_lfs_files: 'LFS files are locked',
  title_regex: 'Title does not match the required pattern',
}

export function mergeStatusLabel(s: string | null | undefined): string {
  if (!s) return 'Unknown'
  return MERGE_STATUS[s] ?? s.replace(/_/g, ' ')
}

export function shortSha(sha: string | null | undefined): string {
  return sha ? sha.slice(0, 8) : ''
}

/** Seconds a pipeline or job has taken (live while it runs). */
export function elapsedSeconds(
  p: { duration: number | null; startedAt: string | null; status: string },
  now: number = Date.now(),
): number | null {
  if (p.duration !== null && p.duration !== undefined && !isActive(p.status)) return p.duration
  if (p.startedAt && isActive(p.status)) {
    const t = Date.parse(p.startedAt)
    if (Number.isFinite(t)) return Math.max(0, (now - t) / 1000)
  }
  return p.duration ?? null
}

// ---------------------------------------------------------------- job log sections

// section_start:1790419369:prepare_executor[collapsed=true]\r\x1b[0K
// The pattern lives in a function because a /g regex keeps lastIndex state.
function markerRe() {
  // eslint-disable-next-line no-control-regex
  return /section_(start|end):(\d+):([A-Za-z0-9_.-]+)(?:\[([^\]\r\n]*)\])?\r?(?:\x1b\[0K)?/g
}

// eslint-disable-next-line no-control-regex
const ANSI = /\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]/g

export function stripAnsi(s: string): string {
  return s.replace(ANSI, '').replace(/\r/g, '')
}

export interface LogSection {
  /** Order of appearance; stable while the log grows. */
  id: number
  name: string
  /** Header text without escapes (falls back to the name). */
  title: string
  start: number
  end: number | null
  depth: number
  collapsedByDefault: boolean
  /** Index of the header line in `ParsedLog.lines`. */
  line: number
}

interface ParsedLine {
  text: string
  /** Section this line opens (header line), if any. */
  opens: number | null
  /** Only markers, nothing visible: not rendered. */
  markerOnly: boolean
  /** Sections enclosing this line (for a header: its parents). */
  parents: number[]
}

export interface ParsedLog {
  lines: ParsedLine[]
  sections: LogSection[]
}

/** Split a (prefix-stripped) job log into lines and nested sections. */
export function parseLog(text: string): ParsedLog {
  const sections: LogSection[] = []
  const lines: ParsedLine[] = []
  const stack: number[] = []
  const raw = text.split('\n')
  for (let i = 0; i < raw.length; i++) {
    const line = raw[i]
    if (!line.includes('section_')) {
      lines.push({ text: line, opens: null, markerOnly: false, parents: stack.slice() })
      continue
    }
    let opens: number | null = null
    let parents = stack.slice()
    let found = false
    for (const m of line.matchAll(markerRe())) {
      found = true
      const [, kind, ts, name, opts] = m
      if (kind === 'start') {
        parents = stack.slice()
        const id = sections.length
        sections.push({
          id,
          name,
          title: name,
          start: Number(ts),
          end: null,
          depth: stack.length,
          collapsedByDefault: /collapsed=true/.test(opts ?? ''),
          line: lines.length,
        })
        stack.push(id)
        opens = id
      } else {
        const at = stack.map((id) => sections[id].name).lastIndexOf(name)
        if (at >= 0) {
          sections[stack[at]].end = Number(ts)
          stack.splice(at)
        }
      }
    }
    const visible = found ? line.replace(markerRe(), '') : line
    if (opens !== null) {
      const title = stripAnsi(visible).trim()
      if (title) sections[opens].title = title
    }
    lines.push({
      text: visible,
      opens,
      markerOnly: found && opens === null && stripAnsi(visible).trim() === '',
      parents: opens !== null ? parents : stack.slice(),
    })
  }
  return { lines, sections }
}

/** Whether a section is folded, given the user's overrides. */
export function isCollapsed(s: LogSection, overrides: ReadonlyMap<number, boolean>): boolean {
  return overrides.get(s.id) ?? s.collapsedByDefault
}

/**
 * The text to show: section markers replaced by a fold chevron on the header
 * line, and the bodies of folded sections left out. With unchanged folds the
 * output for a longer log starts with the output for a shorter one, so the
 * viewer can append instead of redrawing.
 */
export function renderLog(parsed: ParsedLog, overrides: ReadonlyMap<number, boolean>): string {
  const folded = new Set(parsed.sections.filter((s) => isCollapsed(s, overrides)).map((s) => s.id))
  const out: string[] = []
  for (const l of parsed.lines) {
    if (l.markerOnly) continue
    if (l.parents.some((p) => folded.has(p))) continue
    if (l.opens !== null) {
      const chevron = folded.has(l.opens) ? '▸' : '▾'
      out.push(`\x1b[2m${chevron}\x1b[22m ${l.text}`)
    } else {
      out.push(l.text)
    }
  }
  return out.join('\n')
}

/** Header line as it appears in the rendered log (plain text), for search-to-jump. */
export function sectionSearchText(s: LogSection, overrides: ReadonlyMap<number, boolean>): string {
  return `${isCollapsed(s, overrides) ? '▸' : '▾'} ${s.title}`
}

/** The last `n` lines of text. */
export function lastLines(text: string, n: number): string {
  const lines = text.split('\n')
  return lines.slice(Math.max(0, lines.length - n)).join('\n')
}

// ---------------------------------------------------------------- diff comments

/** Monaco's ILineChange (an end line of 0 means "no lines on this side"). */
export interface LineChange {
  originalStartLineNumber: number
  originalEndLineNumber: number
  modifiedStartLineNumber: number
  modifiedEndLineNumber: number
}

/**
 * GitLab position lines for a line clicked in the diff: added lines have only
 * `newLine`, removed lines only `oldLine`, unchanged lines both.
 */
export function positionForLine(
  changes: readonly LineChange[],
  side: 'original' | 'modified',
  line: number,
): { oldLine?: number; newLine?: number } {
  const len = (s: number, e: number) => (e === 0 ? 0 : e - s + 1)
  let delta = 0
  for (const c of changes) {
    const o = len(c.originalStartLineNumber, c.originalEndLineNumber)
    const m = len(c.modifiedStartLineNumber, c.modifiedEndLineNumber)
    if (side === 'modified') {
      if (m > 0 && line >= c.modifiedStartLineNumber && line <= c.modifiedEndLineNumber) return { newLine: line }
      const anchor = m === 0 ? c.modifiedStartLineNumber : c.modifiedEndLineNumber
      if (anchor < line) delta += m - o
    } else {
      if (o > 0 && line >= c.originalStartLineNumber && line <= c.originalEndLineNumber) return { oldLine: line }
      const anchor = o === 0 ? c.originalStartLineNumber : c.originalEndLineNumber
      if (anchor < line) delta += o - m
    }
  }
  return side === 'modified' ? { oldLine: line - delta, newLine: line } : { oldLine: line, newLine: line - delta }
}

// ---------------------------------------------------------------- changed files tree

export interface FileRow {
  kind: 'dir' | 'file'
  /** Directory path (dirs) or the file's new path (files). */
  path: string
  /** Display name; compacted directory chains read `a/b/c`. */
  name: string
  depth: number
  file?: MrDiffFile
}

interface DirNode {
  dirs: Map<string, DirNode>
  files: MrDiffFile[]
}

/**
 * Rows of a CLion-style changes tree: directories first (single-child chains
 * compacted), then files, each level sorted by name.
 */
export function fileRows(files: readonly MrDiffFile[], collapsed: ReadonlySet<string> = new Set()): FileRow[] {
  const root: DirNode = { dirs: new Map(), files: [] }
  for (const f of files) {
    const parts = (f.deletedFile ? f.oldPath : f.newPath).split('/')
    let node = root
    for (const dir of parts.slice(0, -1)) {
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
    const sorted = [...node.files].sort((a, b) => baseName(a).localeCompare(baseName(b)))
    for (const f of sorted) rows.push({ kind: 'file', path: f.newPath, name: baseName(f), depth, file: f })
  }
  walk(root, '', 0)
  return rows
}

function baseName(f: MrDiffFile): string {
  const p = f.deletedFile ? f.oldPath : f.newPath
  return p.slice(p.lastIndexOf('/') + 1)
}

/** CLion change letter for a file: A(dded), D(eleted), R(enamed) or M(odified). */
export function changeKind(f: MrDiffFile): 'A' | 'D' | 'R' | 'M' {
  if (f.newFile) return 'A'
  if (f.deletedFile) return 'D'
  if (f.renamedFile) return 'R'
  return 'M'
}

// ---------------------------------------------------------------- prompts

export function jobFixPrompt(o: {
  projectPath: string
  job: Pick<Job, 'id' | 'name' | 'stage' | 'failureReason' | 'status'>
  pipeline: { id: number; iid: number | null; ref: string; sha: string } | null
  log: string
  shownLines: number
  totalLines: number
}): string {
  const where = o.pipeline
    ? ` in pipeline #${o.pipeline.iid ?? o.pipeline.id} (id ${o.pipeline.id}) on ${o.pipeline.ref} @ ${shortSha(o.pipeline.sha)}`
    : ''
  const reason = o.job.failureReason ? ` Failure reason: ${o.job.failureReason.replace(/_/g, ' ')}.` : ''
  return [
    `The GitLab CI job "${o.job.name}" (stage ${o.job.stage}, job id ${o.job.id}) of ${o.projectPath} ${o.job.status}${where}.${reason}`,
    '',
    `Last ${o.shownLines} of ${o.totalLines} log lines:`,
    '```',
    o.log.replace(/```/g, "'''"),
    '```',
    '',
    `Find the root cause and fix it in the code. For more of the log use the gitlab_job_log tool (jobId ${o.job.id}, tailLines up to 2000);` +
      (o.pipeline ? ` gitlab_pipeline_jobs (pipelineId ${o.pipeline.id}) lists the pipeline's other jobs.` : ''),
  ].join('\n')
}

/** The prompt of a test's "Ask agent to fix". */
export function testFixPrompt(o: {
  projectPath: string
  pipeline: { id: number; iid: number | null; ref: string; sha: string }
  suite: string
  test: { status: string; name: string; classname: string; file: string | null; output: string; outputTruncated: boolean }
}): string {
  const t = o.test
  const who = t.classname ? `${t.classname} › ${t.name}` : t.name
  return [
    `The test "${who}" (suite ${o.suite}) ${t.status === 'error' ? 'errored' : 'fails'} in GitLab CI pipeline #${o.pipeline.iid ?? o.pipeline.id} (id ${o.pipeline.id}) of ${o.projectPath} on ${o.pipeline.ref} @ ${shortSha(o.pipeline.sha)}.`,
    ...(t.file ? [`Test file: ${t.file}`] : []),
    '',
    t.output ? `Output${t.outputTruncated ? ' (cut)' : ''}:` : 'The report has no output for it.',
    ...(t.output ? ['```', t.output.replace(/```/g, "'''"), '```'] : []),
    '',
    `Find the root cause and fix it; run the test locally to confirm. gitlab_test_failures (pipelineId ${o.pipeline.id}) lists the pipeline's other failures.`,
  ].join('\n')
}

export function mrReviewPrompt(o: {
  projectPath: string
  iid: number
  title: string
  sourceBranch: string
  targetBranch: string
  sha: string | null
}): string {
  return [
    `Review GitLab merge request !${o.iid} "${o.title}" in ${o.projectPath} (${o.sourceBranch} → ${o.targetBranch}${o.sha ? `, head ${shortSha(o.sha)}` : ''}).`,
    `Use the gitlab_mr tool (iid ${o.iid}) for the description and existing discussions, and gitlab_mr_diff for the changes (pass path to read one file).`,
    'Report correctness problems, risky changes and missing tests, citing file:line. Do not post anything on GitLab unless I ask; if I do, use gitlab_mr_comment.',
  ].join('\n')
}

/** Initials for an avatar badge. */
export function initials(u: { name?: string; username?: string } | null | undefined): string {
  const n = (u?.name || u?.username || '?').trim()
  const parts = n.split(/\s+/).filter(Boolean)
  const s = parts.length > 1 ? parts[0][0] + parts[parts.length - 1][0] : n.slice(0, 2)
  return s.toUpperCase()
}

/** Pipelines newest first, deduplicated by id (for merged pages and live updates). */
export function mergePipelines(pages: readonly (readonly Pipeline[])[]): Pipeline[] {
  const seen = new Set<number>()
  const out: Pipeline[] = []
  for (const page of pages) {
    for (const p of page) {
      if (seen.has(p.id)) continue
      seen.add(p.id)
      out.push(p)
    }
  }
  return out
}
