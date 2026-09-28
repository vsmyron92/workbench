import { describe, expect, it } from 'vitest'
import {
  aggregateState,
  expiresIn,
  canMergeNow,
  changeKind,
  commentTarget,
  diffLines,
  elapsedSeconds,
  fileRows,
  ghLabel,
  groupJobs,
  headerText,
  isCollapsed,
  isSetupError,
  jobFixPrompt,
  livePoll,
  mergeBox,
  mergeRuns,
  parseGhLog,
  placedThreads,
  prState,
  rateLow,
  rateText,
  renderGhLog,
  rowKeys,
  stateLabel,
  stateTone,
  stripAnsi,
  SUMMARY_POLL_MS,
  summaryPoll,
  titleFromBranch,
} from './logic'
import { mobileViewFor } from './mobile'
import type { Job, PrFile, Pull, Run, Step, Thread } from './types'

const step = (number: number, name: string, state: string): Step => ({
  number,
  name,
  state,
  status: 'completed',
  conclusion: state === 'failed' ? 'failure' : 'success',
  startedAt: '2026-09-26T10:00:00Z',
  completedAt: '2026-09-26T10:00:12Z',
})

const job = (id: number, name: string, state: string): Job => ({
  id,
  runId: 1,
  runAttempt: 1,
  name,
  workflowName: 'CI',
  status: 'completed',
  conclusion: null,
  createdAt: null,
  startedAt: null,
  completedAt: null,
  htmlUrl: '',
  headSha: 'abcdef0123456789',
  headBranch: 'main',
  labels: [],
  runnerName: null,
  steps: [],
  state,
  duration: 10,
})

const pull = (o: Partial<Pull>): Pull =>
  ({ state: 'open', draft: false, merged: false, mergedAt: null, mergeableState: 'clean', ...o }) as Pull

describe('states', () => {
  it('maps tones and labels', () => {
    expect(stateTone('success')).toBe('success')
    expect(stateTone('failed')).toBe('danger')
    expect(stateTone('running')).toBe('accent')
    expect(stateTone('manual')).toBe('warning')
    expect(stateTone(null)).toBe('muted')
    expect(stateLabel('pending')).toBe('queued')
    expect(stateLabel(null)).toBe('no checks')
    expect(ghLabel('completed', 'timed_out')).toBe('timed out')
    expect(ghLabel('in_progress', null)).toBe('in progress')
  })

  it('aggregates like the server', () => {
    expect(aggregateState(['success', 'failed', 'running'])).toBe('failed')
    expect(aggregateState(['success', 'pending'])).toBe('pending')
    expect(aggregateState(['skipped', 'success'])).toBe('success')
    expect(aggregateState([])).toBeNull()
  })

  it('times running things live', () => {
    const now = Date.parse('2026-09-26T10:01:00Z')
    expect(elapsedSeconds({ duration: null, state: 'running', runStartedAt: '2026-09-26T10:00:00Z' }, now)).toBe(60)
    expect(elapsedSeconds({ duration: 42, state: 'success', startedAt: '2026-09-26T10:00:00Z' }, now)).toBe(42)
  })

  it('dedupes runs across pages', () => {
    const r = (id: number) => ({ id }) as Run
    expect(mergeRuns([[r(3), r(2)], [r(2), r(1)]]).map((x) => x.id)).toEqual([3, 2, 1])
  })

  it('names pull request states', () => {
    expect(prState(pull({}))).toBe('open')
    expect(prState(pull({ draft: true }))).toBe('draft')
    expect(prState(pull({ state: 'closed', mergedAt: '2026-01-01' }))).toBe('merged')
    expect(prState(pull({ state: 'closed' }))).toBe('closed')
  })
})

describe('merge box', () => {
  it('explains mergeability', () => {
    expect(mergeBox(pull({})).tone).toBe('success')
    expect(mergeBox(pull({ mergeableState: 'dirty' })).label).toBe('Merge conflicts')
    expect(mergeBox(pull({ mergeableState: 'blocked' })).tone).toBe('warning')
    expect(mergeBox(pull({ mergeableState: null })).label).toContain('Checking')
    expect(mergeBox(pull({ draft: true })).label).toContain('Draft')
    expect(canMergeNow(pull({ mergeableState: 'unstable' }))).toBe(true)
    expect(canMergeNow(pull({ mergeableState: 'blocked' }))).toBe(false)
    expect(canMergeNow(pull({ merged: true }))).toBe(false)
  })
})

describe('rate limit line', () => {
  it('says what is left and when it resets', () => {
    expect(rateText({ limit: 60, remaining: 43, reset: 1000 + 12 * 60, used: 17 }, 1000)).toBe('43/60 requests left · resets in 12 min')
    expect(rateText(null)).toBeNull()
    expect(rateLow({ limit: 60, remaining: 4, reset: null, used: 56 })).toBe(true)
    expect(rateLow({ limit: 5000, remaining: 4000, reset: null, used: 1000 })).toBe(false)
  })
})

describe('polling', () => {
  const summary = (authenticated: boolean, running = false) => ({
    auth: { authenticated },
    branchRun: { state: running ? 'running' : 'success' },
    headStatus: null,
  })
  it('keeps anonymous refreshes slower than the server keeps its answers (5 min)', () => {
    expect(summaryPoll(summary(false), null)).toBe(SUMMARY_POLL_MS.anonymous)
    expect(summaryPoll(summary(false, true), null)).toBe(SUMMARY_POLL_MS.anonymous)
    expect(SUMMARY_POLL_MS.anonymous).toBeGreaterThanOrEqual(300_000)
    expect(summaryPoll(summary(true, true), null)).toBe(SUMMARY_POLL_MS.active)
    expect(summaryPoll(summary(true), null)).toBe(SUMMARY_POLL_MS.idle)
  })
  it('does not poll detail views without a token', () => {
    expect(livePoll(true, true, 5_000)).toBe(false)
    expect(livePoll(false, true, 5_000)).toBe(5_000)
    expect(livePoll(false, false, 5_000)).toBe(false)
  })
  it('stops retrying what a timer cannot fix', () => {
    const err = (status: number, code: string) => Object.assign(new Error('x'), { status, code })
    expect(summaryPoll(undefined, err(412, 'not_configured'))).toBe(false)
    expect(summaryPoll(undefined, err(404, 'not_found'))).toBe(false)
    expect(summaryPoll(undefined, err(502, 'upstream'))).toBe(SUMMARY_POLL_MS.idle)
    expect(isSetupError(err(429, 'rate_limited'))).toBe(false)
    expect(isSetupError(null)).toBe(false)
    // A summary that loaded once keeps refreshing (a later error is transient).
    expect(summaryPoll(summary(false), err(404, 'not_found'))).toBe(SUMMARY_POLL_MS.anonymous)
  })
})

describe('phone tab', () => {
  it('takes runs, jobs, pull requests and issues, for their own project', () => {
    expect(mobileViewFor('gh.run', { projectId: 'p', runId: 3 }, 'q')).toEqual({ kind: 'run', pid: 'p', id: 3 })
    expect(mobileViewFor('gh.job', { projectId: 'p', jobId: 9 }, 'q')).toEqual({ kind: 'job', pid: 'p', id: 9, runId: null })
    expect(mobileViewFor('pr', { projectId: 'p', number: 7 }, 'q')).toEqual({ kind: 'pr', pid: 'p', number: 7 })
    expect(mobileViewFor('gh.issue', { number: 2 }, 'q')).toEqual({ kind: 'issue', pid: 'q', number: 2 })
    expect(mobileViewFor('mr', { projectId: 'p', iid: 1 }, 'q')).toBeNull()
    expect(mobileViewFor('pr', { projectId: 'p' }, 'q')).toBeNull()
    expect(mobileViewFor('pr', { number: 1 }, null)).toBeNull()
  })
})

describe('job logs', () => {
  const text = [
    'Current runner version',
    '##[group]Operating System',
    'Ubuntu',
    '##[endgroup]',
    '##[group]Run cargo test',
    'cargo test',
    '##[endgroup]',
    '\x1b[31merror\x1b[0m: boom',
    '##[error]Process completed with exit code 101.',
  ].join('\n')
  const steps = [step(1, 'Set up job', 'success'), step(2, 'Run tests', 'failed')]
  const marks = [
    { number: 1, line: 0 },
    { number: 2, line: 4 },
  ]

  it('builds steps with groups inside them', () => {
    const p = parseGhLog(text, steps, marks)
    expect(p.sections.map((s) => [s.kind, s.title, s.depth])).toEqual([
      ['step', 'Set up job', 0],
      ['group', 'Operating System', 1],
      ['step', 'Run tests', 0],
      ['group', 'Run cargo test', 1],
    ])
    // "Run cargo test" inside step "Run tests" is not an echo; with the same title it would be.
    expect(p.sections.map((s) => !!s.echo)).toEqual([false, false, false, false])
    expect(parseGhLog(text, [step(1, 'Set up job', 'success'), step(2, 'Run cargo test', 'failed')], marks).sections[3].echo).toBe(true)
    // Failed steps start open, others and all groups folded.
    expect(p.sections.map((s) => s.collapsedByDefault)).toEqual([true, true, false, true])
    expect(p.sections[0].duration).toBe(12)
  })

  it('renders folds, headers and message markers', () => {
    const p = parseGhLog(text, steps, marks)
    const plain = stripAnsi(renderGhLog(p, new Map()))
    expect(plain.split('\n')).toEqual([
      '▸ ✓ Set up job  12s',
      '▾ ✗ Run tests  12s',
      '▸ Run cargo test',
      'error: boom',
      'Error: Process completed with exit code 101.',
    ])
    const open = new Map([[1, false], [0, false]])
    const all = stripAnsi(renderGhLog(p, open)).split('\n')
    expect(all.slice(0, 5)).toEqual(['▾ ✓ Set up job  12s', 'Current runner version', '▾ Operating System', 'Ubuntu', '▾ ✗ Run tests  12s'])
    expect(headerText(p.sections[2], isCollapsed(p.sections[2], new Map()))).toBe('▾ ✗ Run tests')
  })

  it('works without step marks (a truncated log)', () => {
    const p = parseGhLog(text)
    expect(p.sections.map((s) => [s.kind, s.depth])).toEqual([
      ['group', 0],
      ['group', 0],
    ])
    expect(stripAnsi(renderGhLog(p, new Map())).split('\n')[0]).toBe('Current runner version')
  })
})

describe('review comments', () => {
  const patch = '@@ -1,3 +1,4 @@\n a\n-b\n+c\n+d\n e\n@@ -10,2 +11,2 @@\n x\n-y\n+z\n\\ No newline at end of file'

  it('knows which lines are in the diff', () => {
    const l = diffLines(patch)
    expect([...l.right].sort((a, b) => a - b)).toEqual([1, 2, 3, 4, 11, 12])
    expect([...l.left].sort((a, b) => a - b)).toEqual([1, 2, 3, 10, 11])
    expect(commentTarget(l, 'modified', 2)).toEqual({ line: 2, side: 'RIGHT' })
    expect(commentTarget(l, 'original', 2)).toEqual({ line: 2, side: 'LEFT' })
    expect(commentTarget(l, 'modified', 7)).toBeNull()
    expect(diffLines(null).right.size).toBe(0)
  })

  it('places threads by side, skipping outdated ones', () => {
    const t = (id: string, side: string, line: number | null, outdated = false) =>
      ({ id, path: 'a.rs', side, line, outdated, comments: [] }) as unknown as Thread
    const placed = placedThreads([t('1', 'RIGHT', 4), t('2', 'LEFT', 2), t('3', 'RIGHT', null, true)], 'a.rs')
    expect(placed.map((p) => [p.thread.id, p.side, p.line])).toEqual([
      ['1', 'RIGHT', 4],
      ['2', 'LEFT', 2],
    ])
  })
})

describe('files tree', () => {
  const f = (filename: string, status = 'modified') => ({ filename, status }) as PrFile
  it('compacts directories and sorts', () => {
    const rows = fileRows([f('src/a/b/x.rs'), f('src/a/b/a.rs', 'added'), f('README.md')])
    expect(rows.map((r) => `${r.kind}:${r.depth}:${r.name}`)).toEqual(['dir:0:src/a/b', 'file:1:a.rs', 'file:1:x.rs', 'file:0:README.md'])
    expect(fileRows([f('src/a/b/x.rs')], new Set(['src/a/b'])).length).toBe(1)
    expect(changeKind(f('x', 'removed'))).toBe('D')
    expect(changeKind(f('x', 'renamed'))).toBe('R')
  })
})

describe('jobs and prompts', () => {
  it('groups matrix jobs', () => {
    const g = groupJobs([job(1, 'build (ubuntu, stable)', 'success'), job(2, 'lint', 'success'), job(3, 'build (macos, stable)', 'failed')])
    expect(g.map((x) => [x.name, x.jobs.length, x.state])).toEqual([
      ['build', 2, 'failed'],
      ['lint', 1, 'success'],
    ])
  })

  it('writes a fix prompt with the log or the annotations', () => {
    const j = { ...job(9, 'test', 'failed'), steps: [step(2, 'Run tests', 'failed')] }
    const withLog = jobFixPrompt({ repo: 'o/r', job: j, runName: 'CI', log: 'boom ```x```', shownLines: 1, totalLines: 90 })
    expect(withLog).toContain('Failed step: Run tests')
    expect(withLog).toContain("boom '''x'''")
    expect(withLog).toContain('github_job_log tool (jobId 9')
    const noLog = jobFixPrompt({
      repo: 'o/r',
      job: j,
      log: null,
      shownLines: 0,
      totalLines: 0,
      annotations: [{ path: 'src/a.rs', startLine: 3, endLine: 3, annotationLevel: 'failure', title: null, message: 'mismatched types' }],
    })
    expect(noLog).toContain('- failure src/a.rs:3: mismatched types')
    expect(noLog).not.toContain('log lines')
  })

  it('suggests titles and handles row keys', () => {
    expect(titleFromBranch('feature/add-thing_now')).toBe('Add thing now')
    let opened = 0
    const k = rowKeys(() => opened++)
    type Ev = Parameters<typeof k.onKeyDown>[0]
    const row = {}
    k.onKeyDown({ key: 'Enter', target: row, currentTarget: row, preventDefault: () => {} } as unknown as Ev)
    k.onKeyDown({ key: 'Enter', target: {}, currentTarget: row, preventDefault: () => {} } as unknown as Ev)
    expect(opened).toBe(1)
  })
})

describe('expiresIn', () => {
  it('counts forward', () => {
    const now = Date.parse('2026-09-28T09:00:00Z')
    expect(expiresIn('2026-12-27T09:00:00Z', now)).toBe('in 90 days')
    expect(expiresIn('2026-09-28T14:30:00Z', now)).toBe('in 5 hours')
    expect(expiresIn('2026-09-28T09:20:00Z', now)).toBe('within an hour')
    expect(expiresIn('2026-09-27T09:00:00Z', now)).toBe('expired')
    expect(expiresIn('garbage', now)).toBe('')
  })
})
