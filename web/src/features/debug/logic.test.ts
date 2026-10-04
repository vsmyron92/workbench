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
  configSubtitle,
  configTitle,
  moveLines,
  patchLine,
  rememberedConfig,
  remoteChip,
  remoteLines,
  fieldRange,
  formatLiveValue,
  LIVE_HISTORY,
  numericValue,
  pushSample,
  sparklineRuns,
  fieldValueText,
  filterPeripherals,
  filterRegisters,
  hexAddress,
  registerNote,
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
import type { DebugSession, Frame, LaunchConfig, LineBreakpoint, RemoteConfig, SvdRegister } from './types'

const bp = (line: number, extra: Partial<LineBreakpoint> = {}): LineBreakpoint => ({ id: `b${line}`, path: 'src/main.c', line, enabled: true, ...extra })

const session = (over: Partial<DebugSession> = {}): DebugSession => ({
  id: 'd1',
  projectId: 'app',
  name: 'server',
  adapter: 'gdb',
  adapterLabel: 'GDB',
  adapterKind: 'gdb',
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

  it('includes a microcontroller\'s own output channel but not the debug server\'s chatter', () => {
    const p = agentPrompt({
      session: session(),
      frames: [{ id: 1, name: 'main', line: 7, column: 1, source: { path: 'src/main.c', inProject: true } }],
      frameIndex: 0,
      locals: [],
      console: [
        { seq: 1, category: 'server', text: 'Info : Listening on port 3333 for gdb connections\n', at: 0 },
        { seq: 2, category: 'target', text: 'boot: sensor ok\n', at: 0 },
      ],
    })
    expect(p).toContain('boot: sensor ok')
    expect(p).not.toContain('Listening on port')
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

describe('remote targets (embedded)', () => {
  const remote = (over: Partial<RemoteConfig> = {}): RemoteConfig => ({
    server: 'openocd',
    serverLabel: 'OpenOCD',
    serverAvailable: true,
    commandLine: 'openocd -c gdb_port {port} -f board.cfg',
    init: [],
    reset: ['monitor reset halt'],
    download: true,
    stopAt: 'main',
    extended: false,
    channels: [],
    inContainer: false,
    ...over,
  })
  const config = (over: Partial<LaunchConfig> = {}): LaunchConfig => ({
    name: 'board',
    origin: 'config',
    request: 'attach',
    adapterAvailable: true,
    adapterLabel: 'GDB (multi-architecture)',
    language: 'cpp',
    program: 'build/fw.elf',
    args: [],
    stopOnEntry: true,
    problems: [],
    remote: remote(),
    ...over,
  })

  it('spells out what starting one runs, in the order it runs', () => {
    expect(remoteLines(remote(), true)).toEqual([
      'Debug server: openocd -c gdb_port {port} -f board.cfg',
      'gdb connects to: the server on this computer',
      'Then: monitor reset halt',
      'Download the program (load), then monitor reset halt',
      'Stops at main',
    ])
    // Init commands first, then the reset; nothing to download; runs freely.
    expect(remoteLines(remote({ init: ['monitor adapter speed 4000'], download: false }), false)).toEqual([
      'Debug server: openocd -c gdb_port {port} -f board.cfg',
      'gdb connects to: the server on this computer',
      'Then: monitor adapter speed 4000; monitor reset halt',
      'Runs',
    ])
    // A stub that already runs: no server line to hide behind.
    expect(remoteLines(remote({ server: undefined, commandLine: undefined, connect: 'localhost:3333', reset: [], download: false, stopAt: 'reset' }), true)).toEqual([
      'No debug server: gdb connects to a stub that is already running',
      'gdb connects to: localhost:3333',
      'Stops at the reset vector',
    ])
  })

  it('says what is different about an extended-remote stub, a container build, channels and a register map', () => {
    // The stub runs the program; the stop is at its main.
    expect(remoteLines(remote({ server: undefined, commandLine: undefined, connect: 'localhost:2331', extended: true, reset: [], download: false }), true)).toEqual([
      'No debug server: gdb connects to a stub that is already running',
      'gdb connects (target extended-remote) to: localhost:2331',
      'The stub runs the program',
      'Stops at main',
    ])
    expect(remoteLines(remote({ server: undefined, commandLine: undefined, connect: 'bmp:2000', extended: true, attach: 1, reset: [], download: false, stopAt: 'reset' }), true)).toContain('Attaches to target 1')
    // The build runs in the dev container; the output channels and the SVD file are listed last.
    const lines = remoteLines(remote({ inContainer: true, channels: ['uart (5000)', 'swo (5001)'], svd: 'STM32F407.svd' }), true)
    expect(lines[0]).toBe('Built in the dev container; the debugger and the debug server run on this computer')
    expect(lines.slice(-2)).toEqual(['Target output: uart (5000), swo (5001)', 'Register map: STM32F407.svd'])
  })

  it('shows the commands in the tooltip and the server in the subtitle', () => {
    const c = config({ preLaunch: 'make' })
    const title = configTitle(c)
    expect(title.split('\n')).toEqual([
      'build/fw.elf',
      'before: make',
      'Debug server: openocd -c gdb_port {port} -f board.cfg',
      'gdb connects to: the server on this computer',
      'Then: monitor reset halt',
      'Download the program (load), then monitor reset halt',
      'Stops at main',
    ])
    expect(configTitle(config({ problems: ['OpenOCD: `openocd` was not found on PATH'] }))).toMatch(/⚠ OpenOCD: `openocd` was not found on PATH$/)
    expect(configSubtitle(c)).toBe('GDB (multi-architecture) · via OpenOCD · make')
    expect(configSubtitle(config({ remote: remote({ server: undefined, serverLabel: undefined, connect: 'board.local:2345' }) }))).toBe('GDB (multi-architecture) · at board.local:2345')
    // An ordinary configuration is unchanged.
    const plain = config({ remote: undefined, adapterLabel: 'GDB', preLaunch: 'cargo build' })
    expect(configSubtitle(plain)).toBe('GDB · cargo build')
    expect(configTitle(plain).split('\n')).toEqual(['build/fw.elf', 'before: cargo build'])
  })

  it('names the server and target of a session, and what a halt at reset is', () => {
    expect(remoteChip(session())).toBeNull()
    expect(remoteChip(session({ remote: { server: 'OpenOCD', target: '127.0.0.1:3333' } }))).toBe('OpenOCD · 127.0.0.1:3333')
    expect(remoteChip(session({ remote: { target: 'localhost:3333' } }))).toBe('localhost:3333')
    expect(remoteChip(session({ remote: {} }))).toBe('remote target')
    expect(stateLabel(session({ stopped: { reason: 'entry', allThreadsStopped: true, at: 0 } }))).toBe('Paused (at the reset vector)')
    expect(stateLabel(session({ state: 'starting', phase: 'Downloading fw.elf' }))).toBe('Downloading fw.elf')
    expect(stateLabel(session({ state: 'terminated', request: 'attach', stopRequested: true }))).toBe('Detached')
  })

  it('groups a remote configuration with the explicit ones', () => {
    const groups = groupConfigs([config(), config({ name: 'cargo bin', origin: 'cargo', remote: undefined })])
    expect(groups.map((g) => [g.title, g.items.map((c) => c.name)])).toEqual([
      ['Launch configurations', ['board']],
      ['Cargo', ['cargo bin']],
    ])
  })
})

describe('peripherals', () => {
  const reg = (over: Partial<SvdRegister> = {}): SvdRegister => ({
    name: 'CR1',
    offset: 0,
    address: 0x40000000,
    size: 32,
    access: 'read-write',
    readAction: false,
    fields: [],
    ...over,
  })

  it('spells addresses and bit ranges the way a datasheet does', () => {
    expect(hexAddress(0x40020000)).toBe('0x40020000')
    expect(hexAddress(0x10)).toBe('0x00000010')
    expect(hexAddress(0x1_0000_0000)).toBe('0x100000000')
    expect(fieldRange({ bitOffset: 3, bitWidth: 1 })).toBe('[3]')
    expect(fieldRange({ bitOffset: 4, bitWidth: 4 })).toBe('[7:4]')
    expect(fieldRange({ bitOffset: 0, bitWidth: 32 })).toBe('[31:0]')
  })

  it('shows a field as its value name and number, hex when it is wide', () => {
    expect(fieldValueText({ bitWidth: 2, value: 1, valueName: 'Down' })).toBe('Down (1)')
    expect(fieldValueText({ bitWidth: 1, value: 0, valueName: null })).toBe('0')
    expect(fieldValueText({ bitWidth: 24, value: 0xffffff, valueName: null })).toBe('0xFFFFFF')
    expect(fieldValueText({ bitWidth: 24, value: 0, valueName: 'Zero' })).toBe('Zero (0x0)')
    expect(fieldValueText({ bitWidth: 3, value: null, valueName: null })).toBe('')
  })

  it('filters peripherals and registers by every word, in name, group or description', () => {
    const list = [
      { name: 'USART1', base: 1, registers: 7, description: 'Universal synchronous asynchronous receiver transmitter', group: 'USART' },
      { name: 'TIM2', base: 2, registers: 12, description: 'General-purpose timer' },
      { name: 'GPIOA', base: 3, registers: 9, description: 'General-purpose I/O' },
    ]
    expect(filterPeripherals(list, '').map((p) => p.name)).toEqual(['USART1', 'TIM2', 'GPIOA'])
    // Words match as substrings, so "general purpose" finds "General-purpose" in two descriptions.
    expect(filterPeripherals(list, 'general purpose').map((p) => p.name)).toEqual(['TIM2', 'GPIOA'])
    expect(filterPeripherals(list, 'general purpose timer').map((p) => p.name)).toEqual(['TIM2'])
    expect(filterPeripherals(list, 'general-purpose timer').map((p) => p.name)).toEqual(['TIM2'])
    expect(filterPeripherals(list, '  usart ').map((p) => p.name)).toEqual(['USART1'])
    expect(filterPeripherals(list, 'nothing')).toEqual([])
    const regs = [reg({ name: 'CR1', description: 'Control register 1' }), reg({ name: 'SR', description: 'Status' })]
    expect(filterRegisters(regs, 'control').map((r) => r.name)).toEqual(['CR1'])
    expect(filterRegisters(regs, 'sr').map((r) => r.name)).toEqual(['SR'])
    expect(filterRegisters(regs, '')).toHaveLength(2)
  })

  it('says why a register has no value', () => {
    expect(registerNote(reg({ value: '0x00000001' }))).toBe('')
    expect(registerNote(reg({ error: 'Cannot access memory' }))).toBe('Cannot access memory')
    expect(registerNote(reg({ skipped: 'reading it changes the chip' }))).toBe('reading it changes the chip')
    expect(registerNote(reg({ access: 'write-only', skipped: 'write-only' }))).toBe('write-only')
    expect(registerNote(reg())).toBe('')
    // A value wins over a stale note.
    expect(registerNote(reg({ value: '0x0', skipped: 'x' }))).toBe('')
  })
})

describe('live watch', () => {
  it('turns a reading into the number a sparkline plots', () => {
    expect(numericValue(42)).toBe(42)
    expect(numericValue(-1.5)).toBe(-1.5)
    expect(numericValue(true)).toBe(1)
    expect(numericValue(false)).toBe(0)
    expect(numericValue('18446744073709551615')).toBe(18446744073709552000)
    expect(numericValue('-7')).toBe(-7)
    // Bytes, NaN and a missing reading are not plotted.
    expect(numericValue('01 ab ff')).toBeNull()
    expect(numericValue('NaN')).toBeNull()
    expect(numericValue(Number.NaN)).toBeNull()
    expect(numericValue(undefined)).toBeNull()
  })

  it('keeps the newest readings', () => {
    expect(pushSample(undefined, 1)).toEqual([1])
    expect(pushSample([1, 2], 3)).toEqual([1, 2, 3])
    const full = Array.from({ length: LIVE_HISTORY }, (_, i) => i)
    const next = pushSample(full, 999)
    expect(next).toHaveLength(LIVE_HISTORY)
    expect([next[0], next[next.length - 1]]).toEqual([1, 999])
    expect(pushSample([1, 2, 3], 4, 3)).toEqual([2, 3, 4])
  })

  it('shows integers in the radix asked for, a negative one as its two\'s complement', () => {
    expect(formatLiveValue('uint', 4, 1307)).toBe('1307')
    expect(formatLiveValue('uint', 4, 1307, 'hex')).toBe('0x0000051b')
    expect(formatLiveValue('uint', 1, 5, 'bin')).toBe('0b0000_0101')
    expect(formatLiveValue('int', 2, -16, 'dec')).toBe('-16')
    expect(formatLiveValue('int', 2, -16, 'hex')).toBe('0xfff0')
    expect(formatLiveValue('int', 1, -1, 'bin')).toBe('0b1111_1111')
    expect(formatLiveValue('uint', 8, '18446744073709551615', 'hex')).toBe('0xffffffffffffffff')
    expect(formatLiveValue('uint', 8, '18446744073709551615')).toBe('18446744073709551615')
    expect(formatLiveValue('enum', 4, 2)).toBe('2')
    // A pointer is always hex, sized by the target.
    expect(formatLiveValue('ptr', 4, 0x20000000)).toBe('0x20000000')
    expect(formatLiveValue('ptr', 4, 0)).toBe('0x00000000')
  })

  it('shows the other kinds as they are', () => {
    expect(formatLiveValue('bool', 1, true)).toBe('true')
    expect(formatLiveValue('bool', 1, false)).toBe('false')
    expect(formatLiveValue('float', 4, 1.5)).toBe('1.5')
    expect(formatLiveValue('float', 4, 0.1 + 0.2)).toBe('0.3')
    expect(formatLiveValue('float', 8, 3.141592653589793)).toBe('3.141593')
    expect(formatLiveValue('float', 4, 'NaN')).toBe('NaN')
    expect(formatLiveValue('float', 4, '-inf')).toBe('-inf')
    expect(formatLiveValue('bytes', 3, '01 ab ff')).toBe('01 ab ff')
    expect(formatLiveValue('uint', 4, undefined)).toBe('—')
    expect(formatLiveValue('uint', 4, 'not a number')).toBe('not a number')
  })

  it('draws the readings as runs of a polyline, split where a reading failed', () => {
    expect(sparklineRuns([], 100, 20)).toEqual([])
    expect(sparklineRuns([null, null], 100, 20)).toEqual([])
    // A constant series sits in the middle; one point is a run of one.
    expect(sparklineRuns([5, 5, 5], 100, 20)).toEqual(['2.0,10.0 50.0,10.0 98.0,10.0'])
    expect(sparklineRuns([7], 100, 20)).toEqual(['50.0,10.0'])
    // The smallest value is at the bottom, the largest at the top, the oldest on the left.
    expect(sparklineRuns([0, 10], 100, 20)).toEqual(['2.0,18.0 98.0,2.0'])
    // A gap splits the line in two.
    const runs = sparklineRuns([1, 2, null, 3, 4], 100, 20)
    expect(runs).toHaveLength(2)
    expect(runs[0].split(' ')).toHaveLength(2)
    expect(runs[1].split(' ')).toHaveLength(2)
  })
})
