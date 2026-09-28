import { describe, expect, it } from 'vitest'
import { encodeUriPath, modelUriString, parseModelUri } from '@/features/files/modelAccess'
import type { LspStatus, ServerStatus } from './api'
import { remapSemanticTokens, TOKEN_MODIFIERS, TOKEN_TYPES } from './semanticTokens'
import {
  applyEditsToText,
  completionKind,
  displayPath,
  flattenSymbols,
  groupByFile,
  markerSeverity,
  markupToMarkdown,
  projectOfUri,
  rangesOverlap,
  shortenPath,
  symbolKind,
  toLocs,
  toMarker,
  toRange,
  wordAt,
} from './convert'
import { filterDiagnostics, previewParts, progressText, serverFor, serversInUse, summarize } from './logic'
import { editsByUri } from './workspaceEdit'

const r = (l1: number, c1: number, l2: number, c2: number) => ({ start: { line: l1, character: c1 }, end: { line: l2, character: c2 } })

describe('model URIs (files contract)', () => {
  it('match Monaco\'s encoding and parse back', () => {
    expect(modelUriString('api', 'src/main.rs')).toBe('file:///api/src/main.rs')
    expect(modelUriString('api', 'dir x/é:1.ts')).toBe('file:///api/dir%20x/%C3%A9%3A1.ts')
    expect(modelUriString(null, '/etc/hosts')).toBe('file:///~abs/etc/hosts')
    expect(parseModelUri('file:///api/dir%20x/%C3%A9%3A1.ts')).toEqual({ projectId: 'api', path: 'dir x/é:1.ts' })
    expect(parseModelUri('file:///~abs/etc/hosts')).toEqual({ projectId: null, path: '/etc/hosts' })
    expect(parseModelUri('lsp-src://api/usr/lib/x.rs')).toBeNull()
    expect(parseModelUri('inmemory://model/1')).toBeNull()
    expect(encodeUriPath('a-b_c.d~/e')).toBe('a-b_c.d~/e')
  })

  it('name projects and display paths of browser URIs', () => {
    expect(projectOfUri('file:///api/src/a.rs')).toBe('api')
    expect(projectOfUri('lsp-src://api/home/u/.cargo/x.rs')).toBe('api')
    expect(projectOfUri('file:///~abs/etc/x')).toBeNull()
    expect(displayPath('file:///api/src/a%20b.rs')).toBe('src/a b.rs')
    expect(displayPath('lsp-src://api/home/u/x.rs')).toBe('/home/u/x.rs')
    expect(shortenPath('/home/u/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/itoa-1.0/src/lib.rs', '/home/u')).toBe('~/.cargo/registry/…/itoa-1.0/src/lib.rs')
  })
})

describe('LSP ↔ Monaco conversions', () => {
  it('convert positions, ranges and kinds', () => {
    expect(toRange(r(0, 0, 2, 5))).toEqual({ startLineNumber: 1, startColumn: 1, endLineNumber: 3, endColumn: 6 })
    expect(rangesOverlap(toRange(r(1, 0, 1, 4)), toRange(r(1, 4, 1, 8)))).toBe(true)
    expect(rangesOverlap(toRange(r(1, 0, 1, 4)), toRange(r(2, 0, 2, 1)))).toBe(false)
    // LSP Method (2) → Monaco Method (0); unknown kinds are Text.
    expect(completionKind(2)).toBe(0)
    expect(completionKind(15)).toBe(28)
    expect(completionKind(undefined)).toBe(18)
    expect(symbolKind(12)).toBe(11)
    expect([1, 2, 3, 4, undefined].map(markerSeverity)).toEqual([8, 4, 2, 1, 8])
  })

  it('render hover markup as Markdown', () => {
    expect(markupToMarkdown({ kind: 'markdown', value: '**x**' })).toBe('**x**')
    expect(markupToMarkdown({ kind: 'plaintext', value: 'a < b' })).toBe('```text\na < b\n```')
    expect(markupToMarkdown([{ language: 'rust', value: 'fn f()' }, 'doc'])).toBe('```rust\nfn f()\n```\n\n---\n\ndoc')
    expect(markupToMarkdown({ language: 'ts', value: 'a ``` b' })).toBe('````ts\na ``` b\n````')
    expect(markupToMarkdown(null)).toBe('')
  })

  it('turn diagnostics into markers', () => {
    const m = toMarker({ range: r(3, 4, 3, 4), severity: 2, message: 'm', source: 's', code: 'E1', codeDescription: { href: 'https://x' }, tags: [1, 9] })
    expect(m).toMatchObject({ severity: 4, message: 'm', source: 's', code: { value: 'E1', target: 'https://x' }, startLineNumber: 4, startColumn: 5, endColumn: 6, tags: [1] })
    expect(toMarker({ range: r(0, 0, 0, 1), message: 'x', server: 'fake' }).source).toBe('fake')
  })

  it('flatten locations and group them by file', () => {
    const locs = toLocs([
      { targetUri: 'file:///p/b.rs', targetRange: r(0, 0, 5, 1), targetSelectionRange: r(0, 4, 0, 8) },
    ])
    expect(locs[0]).toMatchObject({ uri: 'file:///p/b.rs', range: r(0, 4, 0, 8) })
    expect(toLocs({ uri: 'file:///p/a.rs', range: r(1, 1, 1, 2) })).toHaveLength(1)
    expect(toLocs(null)).toEqual([])
    const g = groupByFile([
      { uri: 'file:///p/b.rs', range: r(9, 0, 9, 1) },
      { uri: 'file:///p/a.rs', range: r(1, 0, 1, 1) },
      { uri: 'file:///p/b.rs', range: r(2, 0, 2, 1) },
      { uri: 'file:///p/b.rs', range: r(2, 0, 2, 1) },
    ])
    expect(g.map((x) => [x.uri, x.locs.map((l) => l.range.start.line)])).toEqual([
      ['file:///p/b.rs', [2, 9]],
      ['file:///p/a.rs', [1]],
    ])
  })

  it('flatten document symbols with depths', () => {
    const tree = [{ name: 'S', kind: 23, range: r(0, 0, 9, 1), selectionRange: r(0, 7, 0, 8), children: [{ name: 'f', kind: 8, range: r(1, 0, 1, 9), selectionRange: r(1, 4, 1, 5) }] }]
    expect(flattenSymbols(tree).map((s) => [s.name, s.depth, s.container])).toEqual([
      ['S', 0, undefined],
      ['f', 1, 'S'],
    ])
    const flat = [
      { name: 'b', kind: 12, location: { uri: 'u', range: r(5, 0, 5, 1) } },
      { name: 'a', kind: 12, containerName: 'M', location: { uri: 'u', range: r(1, 0, 1, 1) } },
    ]
    expect(flattenSymbols(flat).map((s) => [s.name, s.depth])).toEqual([
      ['a', 1],
      ['b', 0],
    ])
  })

  it('apply edits to text (UTF-16 offsets, last first)', () => {
    const text = 'let é = 1;\nfoo(é);\n'
    expect(applyEditsToText(text, [{ range: r(0, 4, 0, 5), newText: 'x' }, { range: r(1, 4, 1, 5), newText: 'x' }])).toBe('let x = 1;\nfoo(x);\n')
    expect(applyEditsToText('ab', [{ range: r(0, 9, 0, 9), newText: '!' }])).toBe('ab!')
    expect(wordAt('  foo_bar(x)', 5)).toEqual({ start: 2, end: 9, word: 'foo_bar' })
    expect(wordAt('a + b', 2)).toBeNull()
  })

  it('collect workspace edits by file and refuse file operations', () => {
    const a = editsByUri({ changes: { 'file:///p/a.rs': [{ range: r(0, 0, 0, 1), newText: 'x' }] } })
    expect([...a.files.keys()]).toEqual(['file:///p/a.rs'])
    const b = editsByUri({
      documentChanges: [
        { textDocument: { uri: 'file:///p/a.rs', version: 3 }, edits: [{ range: r(0, 0, 0, 1), newText: 'x' }] },
        { textDocument: { uri: 'file:///p/a.rs', version: 3 }, edits: [{ range: r(1, 0, 1, 1), newText: 'y' }] },
        { kind: 'rename', oldUri: 'file:///p/a.rs', newUri: 'file:///p/b.rs' },
      ],
    })
    expect(b.files.get('file:///p/a.rs')).toHaveLength(2)
    expect(b.unsupported).toEqual(['rename b.rs'])
  })
})

function server(p: Partial<ServerStatus> & { id: string }): ServerStatus {
  return {
    label: p.id,
    languages: [],
    extensions: [],
    command: p.id,
    preset: true,
    enabled: true,
    disabledHere: false,
    available: true,
    missing: null,
    installHint: '',
    side: null,
    state: 'off',
    progress: null,
    error: null,
    restarts: 0,
    pid: null,
    startedAt: null,
    running: null,
    serverInfo: null,
    openDocs: 0,
    relevant: false,
    ...p,
  }
}

function status(servers: ServerStatus[], enabled = true): LspStatus {
  return { projectId: 'p', enabled, mode: 'auto', enabledAt: 1, devcontainer: null, servers, counts: { errors: 0, warnings: 0, infos: 0, hints: 0, files: 0 }, warnings: [] }
}

describe('UI logic', () => {
  const py = server({ id: 'pyright', label: 'Pyright', languages: ['python'], extensions: ['py'], relevant: true })
  const based = server({ id: 'basedpyright', languages: ['python'], extensions: ['py'], relevant: true, available: false, state: 'unavailable' })
  const ra = server({ id: 'rust-analyzer', languages: ['rust'], extensions: ['rs'], relevant: true, state: 'indexing', progress: { title: 'Indexing', percentage: 42, message: 'crates' } })
  const sh = server({ id: 'bash', languages: ['shell'], extensions: ['sh'] })

  it('pick the server of a file like the server does', () => {
    expect(serverFor(status([py, based]), 'app/main.py')?.id).toBe('pyright')
    expect(serverFor(status([based, py]), 'app/main.py')?.id).toBe('pyright')
    expect(serverFor(status([based]), 'app/main.py')?.id).toBe('basedpyright')
    expect(serverFor(status([sh]), 'run', 'shell')?.id).toBe('bash')
    expect(serverFor(status([sh]), 'notes.txt', 'plaintext')).toBeNull()
    expect(serverFor(status([{ ...py, disabledHere: true }]), 'a.py')).toBeNull()
  })

  it('list one server per language first', () => {
    expect(serversInUse(status([py, based, ra, sh])).map((s) => s.id)).toEqual(['pyright', 'rust-analyzer'])
    expect(serversInUse(status([based, sh])).map((s) => s.id)).toEqual(['basedpyright'])
  })

  it('summarize for the status bar', () => {
    expect(summarize(status([py], false))).toMatchObject({ text: 'Code intelligence off', tone: 'muted' })
    expect(summarize(status([py]))).toMatchObject({ text: 'No language server running' })
    expect(summarize(status([ra, { ...py, state: 'ready' }]))).toMatchObject({ tone: 'accent', busy: true, text: 'rust-analyzer: indexing 42%' })
    expect(summarize(status([{ ...ra, state: 'ready' }, { ...py, state: 'ready' }]))).toMatchObject({ tone: 'success', text: 'rust-analyzer, Pyright' })
    expect(summarize(status([{ ...ra, state: 'crashed' }]))).toMatchObject({ tone: 'danger', text: 'rust-analyzer crashed' })
    expect(progressText(ra)).toBe('Indexing 42% · crates')
  })

  it('filter and order diagnostics', () => {
    const list = [
      { severity: 2 as const, range: r(5, 0, 5, 1) },
      { severity: 1 as const, range: r(9, 0, 9, 1) },
      { range: r(1, 0, 1, 1) },
      { severity: 4 as const, range: r(0, 0, 0, 1) },
    ]
    const shown = filterDiagnostics(list, { error: true, warning: true, info: true, hint: false })
    expect(shown.map((d) => d.range.start.line)).toEqual([1, 9, 5])
  })

  it('cut preview lines around the match', () => {
    expect(previewParts('        let x = foo(1);', 16, 19)).toEqual({ before: 'let x = ', match: 'foo', after: '(1);' })
    const long = previewParts('x'.repeat(300) + 'MATCH' + 'y'.repeat(300), 300, 305, 120)
    expect(long.before.startsWith('…')).toBe(true)
    expect(long.match).toBe('MATCH')
    expect(long.after.endsWith('…')).toBe(true)
  })
})


describe('remapSemanticTokens', () => {
  it('maps a server legend onto Workbench legend and re-encodes positions around dropped tokens', () => {
    // Server legend: 0 = function, 1 = unknownThing, 2 = variable; modifiers: 0 = readonly, 1 = weird.
    const data = [
      0, 4, 3, 0, 0, // line 0 col 4: function
      0, 5, 2, 1, 0, // line 0 col 9: unknownThing (dropped)
      0, 4, 1, 2, 1, // line 0 col 13: variable readonly
      2, 1, 5, 2, 2, // line 2 col 1: variable with an unknown modifier
    ]
    const out = Array.from(remapSemanticTokens(data, ['function', 'unknownThing', 'variable'], ['readonly', 'weird']))
    const fn = TOKEN_TYPES.indexOf('function')
    const v = TOKEN_TYPES.indexOf('variable')
    const ro = 1 << TOKEN_MODIFIERS.indexOf('readonly')
    expect(out).toEqual([0, 4, 3, fn, 0, 0, 9, 1, v, ro, 2, 1, 5, v, 0])
  })
  it('reads typescript-language-server\'s member as a method', () => {
    expect(Array.from(remapSemanticTokens([0, 0, 3, 0, 0], ['member'], []))).toEqual([0, 0, 3, TOKEN_TYPES.indexOf('method'), 0])
  })
})

describe('flattenSymbols order', () => {
  const sym = (name: string, line: number, children?: object[]) => ({
    name,
    kind: 12,
    range: { start: { line, character: 0 }, end: { line: line + 1, character: 0 } },
    selectionRange: { start: { line, character: 0 }, end: { line, character: name.length } },
    children,
  })
  it('lists symbols in source order at every level, whatever order the server used', () => {
    const alphabetical = [sym('Circle', 10, [sym('radius', 12), sym('area', 11)]), sym('Shape', 1), sym('total', 30)]
    expect(flattenSymbols(alphabetical as never).map((s) => `${'  '.repeat(s.depth)}${s.name}`)).toEqual(['Shape', 'Circle', '  area', '  radius', 'total'])
  })
})
