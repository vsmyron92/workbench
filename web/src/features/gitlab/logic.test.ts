import { describe, expect, it } from 'vitest'
import {
  changeKind,
  elapsedSeconds,
  fileRows,
  testFixPrompt,
  initials,
  isActive,
  isFinished,
  jobFixPrompt,
  lastLines,
  mergePipelines,
  mergeStatusLabel,
  parseLog,
  positionForLine,
  renderLog,
  sectionSearchText,
  statusLabel,
  rowKeys,
  statusTone,
  stripAnsi,
  type LineChange,
} from './logic'
import type { MrDiffFile, Pipeline } from './types'

// A prefix-stripped log as the server delivers it (see server/src/gitlab/trace.rs).
const LOG = [
  '\x1b[0KRunning with gitlab-runner 19.5.0\x1b[0;m',
  'section_start:100:prepare_executor\r\x1b[0K\x1b[0K\x1b[36;1mPreparing the "docker+machine" executor\x1b[0;m',
  'Using Docker executor',
  'section_end:105:prepare_executor\r\x1b[0Ksection_start:105:get_sources[collapsed=true]\r\x1b[0K\x1b[0K\x1b[36;1mGetting source\x1b[0;m',
  'Fetching changes',
  'section_start:106:nested\r\x1b[0KInner',
  'inner body',
  'section_end:107:nested\r\x1b[0K',
  'section_end:108:get_sources\r\x1b[0K',
  '\x1b[32;1mJob succeeded\x1b[0;m',
].join('\n')

describe('statuses', () => {
  it('classifies and labels', () => {
    expect(isActive('running')).toBe(true)
    expect(isActive('success')).toBe(false)
    expect(isFinished('manual')).toBe(true)
    expect(isFinished(null)).toBe(false)
    expect(statusTone('failed')).toBe('danger')
    expect(statusTone('pending')).toBe('warning')
    expect(statusTone('canceled')).toBe('muted')
    expect(statusLabel('success')).toBe('passed')
    expect(statusLabel('waiting_for_resource')).toBe('waiting for resource')
    expect(statusLabel(null)).toBe('no pipeline')
    expect(mergeStatusLabel('need_rebase')).toBe('Needs a rebase')
    expect(mergeStatusLabel('brand_new_status')).toBe('brand new status')
  })

  it('computes live durations', () => {
    const now = Date.parse('2026-09-26T10:10:00Z')
    expect(elapsedSeconds({ duration: 42, startedAt: null, status: 'success' }, now)).toBe(42)
    expect(elapsedSeconds({ duration: null, startedAt: '2026-09-26T10:09:00Z', status: 'running' }, now)).toBe(60)
    expect(elapsedSeconds({ duration: null, startedAt: null, status: 'pending' }, now)).toBe(null)
  })
})

describe('job log sections', () => {
  it('parses nested sections with times and titles', () => {
    const { sections } = parseLog(LOG)
    expect(sections.map((s) => [s.name, s.title, s.start, s.end, s.depth, s.collapsedByDefault])).toEqual([
      ['prepare_executor', 'Preparing the "docker+machine" executor', 100, 105, 0, false],
      ['get_sources', 'Getting source', 105, 108, 0, true],
      ['nested', 'Inner', 106, 107, 1, false],
    ])
  })

  it('renders chevrons and folds collapsed bodies', () => {
    const parsed = parseLog(LOG)
    const plain = (overrides: Map<number, boolean>) => stripAnsi(renderLog(parsed, overrides)).split('\n')
    // get_sources is collapsed by default ([collapsed=true]).
    expect(plain(new Map())).toEqual([
      'Running with gitlab-runner 19.5.0',
      '▾ Preparing the "docker+machine" executor',
      'Using Docker executor',
      '▸ Getting source',
      'Job succeeded',
    ])
    expect(plain(new Map([[1, false]]))).toEqual([
      'Running with gitlab-runner 19.5.0',
      '▾ Preparing the "docker+machine" executor',
      'Using Docker executor',
      '▾ Getting source',
      'Fetching changes',
      '▾ Inner',
      'inner body',
      'Job succeeded',
    ])
    expect(plain(new Map([[0, true], [1, false], [2, true]]))).toEqual([
      'Running with gitlab-runner 19.5.0',
      '▸ Preparing the "docker+machine" executor',
      '▾ Getting source',
      'Fetching changes',
      '▸ Inner',
      'Job succeeded',
    ])
    const s = parsed.sections[0]
    expect(sectionSearchText(s, new Map())).toBe('▾ Preparing the "docker+machine" executor')
  })

  it('renders a growing log as appends', () => {
    const lines = LOG.split('\n')
    const overrides = new Map([[1, false]])
    for (let i = 1; i < lines.length; i++) {
      const shorter = renderLog(parseLog(lines.slice(0, i).join('\n')), overrides)
      const longer = renderLog(parseLog(lines.slice(0, i + 1).join('\n')), overrides)
      expect(longer.startsWith(shorter), `after line ${i}`).toBe(true)
    }
    // A header whose text arrives later (a continuation line) still appends.
    const a = renderLog(parseLog('x\nsection_start:1:s\r\x1b[0K'), new Map())
    const b = renderLog(parseLog('x\nsection_start:1:s\r\x1b[0K\x1b[36mTitle'), new Map())
    expect(b.startsWith(a)).toBe(true)
  })

  it('keeps plain logs untouched and takes tails', () => {
    expect(renderLog(parseLog('a\nb'), new Map())).toBe('a\nb')
    expect(lastLines('1\n2\n3', 2)).toBe('2\n3')
    expect(lastLines('1', 5)).toBe('1')
  })
})

describe('diff comment positions', () => {
  // original: 10 lines. Change A: lines 3-4 replaced by 3-5 (one extra line).
  // Change B: original line 8 deleted (after modified line 8).
  const changes: LineChange[] = [
    { originalStartLineNumber: 3, originalEndLineNumber: 4, modifiedStartLineNumber: 3, modifiedEndLineNumber: 5 },
    { originalStartLineNumber: 8, originalEndLineNumber: 8, modifiedStartLineNumber: 8, modifiedEndLineNumber: 0 },
  ]

  it('maps modified-side lines', () => {
    expect(positionForLine(changes, 'modified', 1)).toEqual({ oldLine: 1, newLine: 1 })
    expect(positionForLine(changes, 'modified', 4)).toEqual({ newLine: 4 })
    expect(positionForLine(changes, 'modified', 6)).toEqual({ oldLine: 5, newLine: 6 })
    expect(positionForLine(changes, 'modified', 8)).toEqual({ oldLine: 7, newLine: 8 })
    expect(positionForLine(changes, 'modified', 9)).toEqual({ oldLine: 9, newLine: 9 })
  })

  it('maps original-side lines', () => {
    expect(positionForLine(changes, 'original', 3)).toEqual({ oldLine: 3 })
    expect(positionForLine(changes, 'original', 8)).toEqual({ oldLine: 8 })
    expect(positionForLine(changes, 'original', 6)).toEqual({ oldLine: 6, newLine: 7 })
    expect(positionForLine(changes, 'original', 10)).toEqual({ oldLine: 10, newLine: 10 })
  })

  it('handles pure insertions', () => {
    const ins: LineChange[] = [{ originalStartLineNumber: 2, originalEndLineNumber: 0, modifiedStartLineNumber: 3, modifiedEndLineNumber: 4 }]
    expect(positionForLine(ins, 'modified', 3)).toEqual({ newLine: 3 })
    expect(positionForLine(ins, 'modified', 5)).toEqual({ oldLine: 3, newLine: 5 })
    expect(positionForLine(ins, 'original', 3)).toEqual({ oldLine: 3, newLine: 5 })
  })
})

describe('changed files tree', () => {
  const f = (path: string, extra: Partial<MrDiffFile> = {}): MrDiffFile => ({
    oldPath: path,
    newPath: path,
    aMode: null,
    bMode: null,
    newFile: false,
    renamedFile: false,
    deletedFile: false,
    generatedFile: null,
    tooLarge: null,
    collapsed: null,
    diff: '',
    additions: 0,
    deletions: 0,
    ...extra,
  })
  const files = [
    f('app/server/crates/api/src/main.rs'),
    f('app/server/crates/api/src/catalog.rs', { newFile: true }),
    f('CLAUDE.md'),
    f('tools/gen.py', { deletedFile: true }),
    f('app/web/src/App.tsx'),
  ]

  it('compacts single-child directories and sorts dirs before files', () => {
    expect(fileRows(files).map((r) => `${'  '.repeat(r.depth)}${r.kind === 'dir' ? r.name + '/' : r.name}`)).toEqual([
      'app/',
      '  server/crates/api/src/',
      '    catalog.rs',
      '    main.rs',
      '  web/src/',
      '    App.tsx',
      'tools/',
      '  gen.py',
      'CLAUDE.md',
    ])
  })

  it('hides children of collapsed directories', () => {
    const rows = fileRows(files, new Set(['app']))
    expect(rows.map((r) => r.name)).toEqual(['app', 'tools', 'gen.py', 'CLAUDE.md'])
  })

  it('labels change kinds', () => {
    expect(files.map(changeKind)).toEqual(['M', 'A', 'M', 'D', 'M'])
    expect(changeKind(f('x', { renamedFile: true }))).toBe('R')
  })
})

describe('prompts and misc', () => {
  it('builds the fix-this-job prompt', () => {
    const p = jobFixPrompt({
      projectPath: 'acme/shop',
      job: { id: 7, name: 'server-tests', stage: 'test', failureReason: 'script_failure', status: 'failed' },
      pipeline: { id: 99, iid: 824, ref: 'main', sha: '6dc3ed1a0000' },
      log: 'error: boom\n```',
      shownLines: 2,
      totalLines: 800,
    })
    expect(p).toContain('"server-tests" (stage test, job id 7)')
    expect(p).toContain('pipeline #824 (id 99) on main @ 6dc3ed1a')
    expect(p).toContain('Failure reason: script failure.')
    expect(p).toContain('gitlab_job_log tool (jobId 7')
    expect(p).toContain("error: boom\n'''")
  })

  it('makes initials and merges pages', () => {
    expect(initials({ name: 'Ada Lovelace' })).toBe('AL')
    expect(initials({ username: 'dev' })).toBe('DE')
    expect(initials(null)).toBe('?')
    const p = (id: number) => ({ id }) as Pipeline
    expect(mergePipelines([[p(3), p(2)], [p(2), p(1)]]).map((x) => x.id)).toEqual([3, 2, 1])
  })
})

describe('rowKeys', () => {
  const key = (k: string, fromInside = false) => {
    const row = {}
    let prevented = false
    const e = { key: k, currentTarget: row, target: fromInside ? {} : row, preventDefault: () => (prevented = true) }
    return { e: e as unknown as Parameters<ReturnType<typeof rowKeys>['onKeyDown']>[0], prevented: () => prevented }
  }

  it('makes a row focusable and opens it with Enter or Space', () => {
    let opened = 0
    const r = rowKeys(() => opened++)
    expect(r.tabIndex).toBe(0)
    expect(r.role).toBe('button')
    for (const k of ['Enter', ' ']) {
      const { e, prevented } = key(k)
      r.onKeyDown(e)
      expect(prevented()).toBe(true)
    }
    r.onKeyDown(key('a').e)
    expect(opened).toBe(2)
  })

  it('leaves keys on a control inside the row to that control', () => {
    let opened = 0
    const r = rowKeys(() => opened++)
    const { e, prevented } = key('Enter', true)
    r.onKeyDown(e)
    expect(opened).toBe(0)
    expect(prevented()).toBe(false)
  })
})

describe('testFixPrompt', () => {
  it('names the test, the file and the output', () => {
    const p = testFixPrompt({
      projectPath: 'g/app',
      pipeline: { id: 303, iid: 12, ref: 'main', sha: '5babfd548d64a14e' },
      suite: 'unit',
      test: { status: 'failed', name: 'adds', classname: 'calc', file: 'src/calc.rs', output: 'left: 4\n```', outputTruncated: true },
    })
    expect(p).toContain('The test "calc › adds" (suite unit) fails in GitLab CI pipeline #12 (id 303) of g/app on main @ 5babfd54.')
    expect(p).toContain('Test file: src/calc.rs')
    expect(p).toContain('Output (cut):')
    expect(p).not.toContain('left: 4\n```\n```')
    expect(p).toContain('gitlab_test_failures (pipelineId 303)')
  })
})
