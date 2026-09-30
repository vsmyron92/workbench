import { afterEach, describe, expect, it } from 'vitest'
import {
  agentPrompt,
  applyCompletion,
  consoleText,
  defaultConfig,
  expressionAt,
  frameLocation,
  glyphKind,
  groupConfigs,
  moveLines,
  patchLine,
  rememberedConfig,
  revealFrame,
  sourceLines,
  sourcePanel,
  sourceViewSession,
  stateLabel,
  toggleLine,
} from './logic'
import { inTurn } from './api'
import { setHealth } from '@/api/health'
import { modelFile } from '@/features/files/modelAccess'
import type { DebugSession, Frame, LaunchConfig, LineBreakpoint } from './types'

const bp = (line: number, extra: Partial<LineBreakpoint> = {}): LineBreakpoint => ({ id: `b${line}`, path: 'src/main.c', line, enabled: true, ...extra })

const session = (over: Partial<DebugSession> = {}): DebugSession => ({
  id: 'd1',
  projectId: 'app',
  name: 'server',
  adapter: 'gdb',
  adapterLabel: 'GDB',
  request: 'launch',
  state: 'stopped',
  stopEpoch: 3,
  threads: [{ id: 1, name: 'main' }],
  capabilities: {},
  startedAt: 0,
  inContainer: false,
  outputSeq: 0,
  ...over,
})

describe('editor models (the files contract: modelFile)', () => {
  it('maps model URIs to project files', () => {
    expect(modelFile('file:///app/src/main.c')).toEqual({ projectId: 'app', path: 'src/main.c' })
    expect(modelFile('file:///app/dir%20x/m%C3%A9.c')).toEqual({ projectId: 'app', path: 'dir x/mé.c' })
    expect(modelFile('file:///~abs/usr/include/stdio.h')).toEqual({ projectId: null, path: '/usr/include/stdio.h' })
    expect(modelFile('file:///~abs/usr/include/stdio.h', true)).toBeNull()
    expect(modelFile('inmemory://model/1')).toBeNull()
    expect(modelFile('lsp-src://app/usr/lib/x.rs')).toBeNull()
    expect(modelFile('file:///app')).toBeNull()
    expect(modelFile('file:///app/')).toBeNull()
  })
})

describe('breakpoint edits', () => {
  const all = [bp(3), bp(10, { condition: 'x > 1' }), bp(4, { path: 'other.c' })]

  it('toggles one line of one file', () => {
    expect(toggleLine(all, 'src/main.c', 10)).toEqual([{ id: 'b3', path: 'src/main.c', line: 3, enabled: true }])
    expect(toggleLine(all, 'src/main.c', 7).map((b) => b.line)).toEqual([3, 10, 7])
    expect(toggleLine(all, 'new.c', 1)).toEqual([{ line: 1, enabled: true }])
  })

  it('patches a breakpoint and drops empty fields', () => {
    const list = patchLine(all, 'src/main.c', 10, { condition: '  ', logMessage: 'x={x}' })
    expect(list.find((b) => b.line === 10)).toEqual({ id: 'b10', path: 'src/main.c', line: 10, enabled: true, logMessage: 'x={x}' })
    expect(patchLine(all, 'src/main.c', 12, { enabled: false }).map((b) => b.line)).toEqual([3, 10, 12])
  })

  it('follows moved lines and merges collisions', () => {
    expect(moveLines(all, 'src/main.c', new Map([['b3', 3]]))).toBeNull()
    expect(moveLines(all, 'src/main.c', new Map([['b3', 5], ['b10', 12]]))?.map((b) => b.line)).toEqual([5, 12])
    expect(moveLines(all, 'src/main.c', new Map([['b3', 10]]))?.map((b) => b.id)).toEqual(['b3'])
  })

  it('draws the gutter like CLion', () => {
    expect(glyphKind(bp(1), false, false)).toBe('bp')
    expect(glyphKind(bp(1, { condition: 'i == 3' }), false, false)).toBe('bp-cond')
    expect(glyphKind(bp(1, { logMessage: 'hi' }), false, false)).toBe('bp-log')
    expect(glyphKind(bp(1, { enabled: false }), true, true)).toBe('bp-disabled')
    expect(glyphKind(bp(1), true, true)).toBe('bp-muted')
    expect(glyphKind(bp(1, { status: { verified: false } }), false, true)).toBe('bp-unverified')
    // Without a session nothing is judged unverified.
    expect(glyphKind(bp(1, { status: { verified: false } }), false, false)).toBe('bp')
  })
})

describe('console', () => {
  it('completes like DAP says', () => {
    // gdb: no start, length = the typed prefix to replace.
    expect(applyCompletion('print tot', 9, { label: 'print total', length: 9 })).toEqual({ text: 'print total', caret: 11 })
    // start (1-based) and length.
    expect(applyCompletion('p.na + 1', 4, { label: 'name', start: 3, length: 2 })).toEqual({ text: 'p.name + 1', caret: 6 })
    // Plain insertion at the caret.
    expect(applyCompletion('x', 1, { label: 'yz' })).toEqual({ text: 'xyz', caret: 3 })
  })

  it('ends log point lines and joins stream chunks', () => {
    const lines = [
      { seq: 1, category: 'stdout', text: 'i=0 ', at: 0 },
      { seq: 2, category: 'stdout', text: 'total=0\n', at: 0 },
      { seq: 3, category: 'console', text: 'total is 116', at: 0 },
      { seq: 4, category: 'stdout', text: 'done\n', at: 0 },
    ]
    expect(consoleText(lines).map((b) => [b.category, b.text])).toEqual([
      ['stdout', 'i=0 total=0\n'],
      ['console', 'total is 116\n'],
      ['stdout', 'done\n'],
    ])
  })
})

describe('evaluate on hover', () => {
  it('takes member chains, not calls or numbers', () => {
    const line = '    total += p->pos.x + ns::VALUE * items[3];'
    expect(expressionAt(line, line.indexOf('total') + 2)?.expr).toBe('total')
    expect(expressionAt(line, line.indexOf('x') + 1)?.expr).toBe('p->pos.x')
    expect(expressionAt(line, line.indexOf('VALUE') + 1)?.expr).toBe('ns::VALUE')
    expect(expressionAt(line, line.indexOf('3') + 1)).toBeNull()
    expect(expressionAt(line, line.indexOf('+') + 1)).toBeNull()
    const e = expressionAt('a.b', 3)!
    expect([e.start, e.end]).toEqual([1, 4])
  })
})

describe('sessions', () => {
  it('labels states', () => {
    expect(stateLabel(session({ stopped: { reason: 'breakpoint', allThreadsStopped: true, at: 0 } }))).toBe('Paused (breakpoint)')
    expect(stateLabel(session({ stopped: { reason: 'exception', description: 'SIGSEGV', allThreadsStopped: true, at: 0 } }))).toBe('Exception: SIGSEGV')
    expect(stateLabel(session({ stopped: { reason: 'pause', allThreadsStopped: true, at: 0 } }))).toBe('Paused')
    expect(stateLabel(session({ state: 'terminated', exitCode: 3 }))).toBe('Exited with code 3')
    expect(stateLabel(session({ state: 'starting', phase: 'Pre-launch: build' }))).toBe('Pre-launch: build')
  })

  it('groups and picks launch configurations', () => {
    const c = (name: string, origin: LaunchConfig['origin']): LaunchConfig => ({ name, origin, request: 'launch', adapterAvailable: true, language: 'c', args: [], stopOnEntry: false, problems: [] })
    const list = [c('Cargo: bin a', 'cargo'), c('server', 'config'), c('Python: api', 'python')]
    expect(groupConfigs(list).map((g) => g.title)).toEqual(['Launch configurations', 'Cargo', 'Python'])
    expect(defaultConfig(list, null)?.name).toBe('server')
    expect(defaultConfig(list, 'Python: api')?.name).toBe('Python: api')
    expect(defaultConfig(list, 'gone')?.name).toBe('server')
    expect(defaultConfig([], null)).toBeNull()
  })

  it('writes a prompt with the stop, the stack and the locals', () => {
    const p = agentPrompt({
      session: session({ stopped: { reason: 'exception', description: 'SIGSEGV', allThreadsStopped: true, at: 0 } }),
      frames: [
        { id: 1, name: 'parse', line: 42, column: 1, source: { path: 'src/parse.c', inProject: true } },
        { id: 2, name: 'main', line: 7, column: 1, source: { path: 'src/main.c', inProject: true } },
      ],
      frameIndex: 0,
      locals: [{ name: 'buf', value: '0x0', type: 'char *', variablesReference: 0 }],
      console: [{ seq: 1, category: 'stderr', text: 'reading input\n', at: 0 }],
    })
    expect(p).toContain('stopped: exception — SIGSEGV at @src/parse.c:42')
    expect(p).toContain('→ #0 parse at src/parse.c:42')
    expect(p).toContain('buf: char * = 0x0')
    expect(p).toContain('reading input')
  })
})

describe('phase-3 review fixes', () => {
  it('labels a session the user stopped as stopped or detached, not by the killed code', () => {
    expect(stateLabel(session({ state: 'terminated', stopRequested: true }))).toBe('Stopped')
    expect(stateLabel(session({ state: 'terminated', stopRequested: true, request: 'attach' }))).toBe('Detached')
    // A subprocess's child session (an attach) of a launched program was ended with it.
    expect(stateLabel(session({ state: 'terminated', stopRequested: true, request: 'attach', parentId: 'd0' }))).toBe('Stopped')
    expect(stateLabel(session({ state: 'terminated' }))).toBe('Terminated')
  })

  it('starts only a configuration the user picked or started before (Shift+F9)', () => {
    const c = (name: string, origin: LaunchConfig['origin']): LaunchConfig => ({ name, origin, request: 'launch', adapterAvailable: true, language: 'c', args: [], stopOnEntry: false, problems: [] })
    const list = [c('leak', 'config'), c('Cargo: bin a', 'cargo')]
    // Nothing picked or used yet: the picker opens instead of the first repository entry.
    expect(rememberedConfig(list, undefined, null)).toBeNull()
    expect(rememberedConfig(list, 'Cargo: bin a', 'leak')).toBe('Cargo: bin a')
    expect(rememberedConfig(list, 'gone', 'leak')).toBe('leak')
    expect(rememberedConfig(list, 'gone', 'also gone')).toBeNull()
  })

  it('shows the top frame after a step into a library, the project frame after a signal', () => {
    const f = (name: string, path: string | null, inProject: boolean, ref?: number): Frame => ({ id: 1, name, line: 2, column: 1, source: path || ref ? { path: path ?? undefined, inProject, sourceReference: ref } : null })
    const frames = [f('helper', '/lib/helper.c', false), f('main', 'main.c', true)]
    expect(revealFrame(frames, 'step')).toBe(0)
    expect(revealFrame(frames, 'breakpoint')).toBe(0)
    expect(revealFrame(frames, 'signal')).toBe(1)
    expect(revealFrame(frames, 'pause')).toBe(1)
    expect(revealFrame([f('raise', null, false), f('main', 'main.c', true)], 'step')).toBe(1)
    expect(revealFrame([f('<eval>', null, false, 7), f('main', 'main.c', true)], 'step')).toBe(0)
  })

  it('marks the execution point and the selected frame in a source view', () => {
    const frames: Frame[] = [
      { id: 1, name: 'helper', line: 2, column: 1, source: { path: '/lib/helper.c', inProject: false } },
      { id: 2, name: 'wrap', line: 9, column: 1, source: { path: '/lib/helper.c', inProject: false } },
      { id: 3, name: 'main', line: 4, column: 1, source: { path: 'main.c', inProject: true } },
    ]
    const s = session({ stopEpoch: 5 })
    expect(sourceLines({ path: '/lib/helper.c' }, s, { epoch: 5, frames }, 0)).toEqual([{ line: 2, kind: 'exec' }])
    expect(sourceLines({ path: '/lib/helper.c' }, s, { epoch: 5, frames }, 1)).toEqual([
      { line: 2, kind: 'exec' },
      { line: 9, kind: 'frame' },
    ])
    expect(sourceLines({ path: '/lib/helper.c' }, s, { epoch: 4, frames }, 0)).toEqual([])
    expect(sourceLines({ path: '/lib/helper.c' }, session({ state: 'running' }), { epoch: 5, frames }, 0)).toEqual([])
    expect(sourceLines({ sourceReference: 3 }, s, { epoch: 5, frames: [{ id: 1, name: 'x', line: 7, column: 1, source: { inProject: false, sourceReference: 3 } }] }, 0)).toEqual([{ line: 7, kind: 'exec' }])
    // debugpy: exec'd code has a pseudo path and a reference; the reference wins.
    expect(sourceLines({ sourceReference: 2 }, s, { epoch: 5, frames: [{ id: 1, name: 'f', line: 4, column: 1, source: { path: '<generated>', inProject: false, sourceReference: 2 } }] }, 0)).toEqual([{ line: 4, kind: 'exec' }])
    expect(sourceViewSession({ scheme: 'inmemory', authority: 'debug-source', path: '/d123/usr/include/stdio.h' })).toBe('d123')
    expect(sourceViewSession({ scheme: 'file', authority: '', path: '/app/src/main.c' })).toBeNull()
  })

  describe('sources outside the project, by the server OS', () => {
    afterEach(() => setHealth(null))
    const onOs = (os: string | null) => setHealth(os ? { ok: true, service: 'workbench', version: '0', startedAt: 1, os } : null)
    const at = (path: string | undefined, inProject = false, sourceReference?: number): Frame => ({ id: 1, name: 'f', line: 12, column: 1, source: { path, inProject, sourceReference } })
    const s = session()

    it('open a file at an absolute path in the debug.source panel', () => {
      for (const os of [null, 'linux', 'windows']) {
        onOs(os)
        expect(sourcePanel(s, at('/usr/include/stdio.h'))).toEqual({
          kind: 'debug.source',
          id: 'debug.source:app:/usr/include/stdio.h',
          title: 'stdio.h',
          params: { projectId: 'app', sessionId: 'd1', path: '/usr/include/stdio.h', name: undefined },
        })
        // Source the debugger holds, a project file, a relative path, none at all.
        expect(sourcePanel(s, at('<generated>', false, 4))).toMatchObject({ id: 'debug.source:app:d1:ref4', title: '<generated>' })
        expect(sourcePanel(s, at('src/main.c', true))).toBeNull()
        expect(sourcePanel(s, at('lib.c'))).toBeNull()
        expect(sourcePanel(s, at(undefined))).toBeNull()
      }
      // A stop in `C:\…` outside the project: a path under `/` on Linux, not absolute there.
      const win = at('C:\\Users\\me\\vendor\\lib.c')
      for (const os of [null, 'linux']) {
        onOs(os)
        expect(sourcePanel(s, win)).toBeNull()
        expect(frameLocation(win)).toBe('C:\\Users\\me\\vendor\\lib.c:12')
      }
      onOs('windows')
      expect(sourcePanel(s, win)).toEqual({
        kind: 'debug.source',
        id: 'debug.source:app:C:\\Users\\me\\vendor\\lib.c',
        title: 'lib.c',
        params: { projectId: 'app', sessionId: 'd1', path: 'C:\\Users\\me\\vendor\\lib.c', name: undefined },
      })
      expect(sourcePanel(s, at('D:/src/dep.rs'))).toMatchObject({ title: 'dep.rs', params: { path: 'D:/src/dep.rs' } })
      // UNC paths are not roots Workbench serves.
      expect(sourcePanel(s, at('\\\\server\\share\\x.c'))).toBeNull()
      expect(frameLocation(win)).toBe('lib.c:12')
      expect(frameLocation(at('src/main.c', true))).toBe('main.c:12')
    })

    it('mark the execution point however the adapter spells the file on Windows', () => {
      const frames = [at('c:/users/me/vendor/lib.c')]
      const view = { path: 'C:\\Users\\me\\vendor\\lib.c' }
      onOs('linux')
      expect(sourceLines(view, s, { epoch: 3, frames }, 0)).toEqual([])
      onOs('windows')
      expect(sourceLines(view, s, { epoch: 3, frames }, 0)).toEqual([{ line: 12, kind: 'exec' }])
      expect(sourceLines({ path: 'C:\\Users\\me\\vendor\\other.c' }, s, { epoch: 3, frames }, 0)).toEqual([])
    })
  })

  it('evaluates watches one at a time', async () => {
    const order: string[] = []
    let running = 0
    let most = 0
    const job = (name: string, ms: number, fail = false) => () =>
      new Promise<string>((resolve, reject) => {
        running++
        most = Math.max(most, running)
        order.push(`start ${name}`)
        setTimeout(() => {
          running--
          order.push(`end ${name}`)
          if (fail) reject(new Error(name))
          else resolve(name)
        }, ms)
      })
    const results = await Promise.allSettled([inTurn('w', job('a', 20)), inTurn('w', job('b', 5, true)), inTurn('w', job('c', 1))])
    expect(most).toBe(1)
    expect(order).toEqual(['start a', 'end a', 'start b', 'end b', 'start c', 'end c'])
    expect(results.map((r) => r.status)).toEqual(['fulfilled', 'rejected', 'fulfilled'])
  })
})
