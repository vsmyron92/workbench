import type { Uri } from 'monaco-editor'
import { afterEach, describe, expect, it } from 'vitest'
import { setHealth } from '@/api/health'
import type { FileEntry, GitStatus } from './api'
import { diffLines, rollbackBlock, splitLines } from './lineDiff'
import { modelFile, modelUriString, parseModelUri } from './modelAccess'
import { absolutePath } from './openers'
import {
  ancestors,
  basename,
  dirname,
  extname,
  fenced,
  hljsLanguage,
  isAbsolutePath,
  isExternalHref,
  mediaKind,
  parseGoto,
  resolveLink,
  samePath,
  segments,
  splitAnchor,
  tabTitle,
  viewerFor,
} from './paths'
import { nextSelection, rankItems } from './quickOpenModel'
import { groupRows, previewParts } from './searchModel'
import { eolOf, minimalEdit, slugify } from './text'
import { dirsToReload, flattenTree, pruneDirs } from './treeModel'
import { buildVcsIndex, fileVcsKind, vcsKindOf } from './vcs'

describe('paths', () => {
  it('splits paths', () => {
    expect(basename('a/b/c.rs')).toBe('c.rs')
    expect(dirname('a/b/c.rs')).toBe('a/b')
    expect(dirname('c.rs')).toBe('')
    expect(dirname('/x')).toBe('/')
    expect(extname('A/B.TSX')).toBe('tsx')
    expect(extname('.env')).toBe('')
    expect(ancestors('a/b/c')).toEqual(['', 'a', 'a/b'])
  })

  it('resolves document links', () => {
    expect(resolveLink('docs/guide.md', '../README.md')).toBe('README.md')
    expect(resolveLink('docs/guide.md', './img/a%20b.png')).toBe('docs/img/a b.png')
    expect(resolveLink('docs/guide.md', '/src/main.rs')).toBe('src/main.rs')
    expect(resolveLink('README.md', '../../etc/passwd')).toBeNull()
    expect(resolveLink('/tmp/x/plan.md', 'shot.png')).toBe('/tmp/x/shot.png')
    expect(splitAnchor('src/main.rs#L42')).toEqual({ path: 'src/main.rs', line: 42 })
    expect(splitAnchor('a.md#usage')).toEqual({ path: 'a.md', anchor: 'usage' })
    expect(isExternalHref('https://x.dev')).toBe(true)
    expect(isExternalHref('mailto:a@b.c')).toBe(true)
    expect(isExternalHref('docs/a.md')).toBe(false)
  })

  it('titles and viewers', () => {
    expect(tabTitle('server/src/files/mod.rs')).toBe('files/mod.rs')
    expect(tabTitle('src/app.rs')).toBe('app.rs')
    expect(mediaKind('x.PNG')).toBe('image')
    expect(mediaKind('x.svg')).toBeNull()
    const text = { content: 'x', binary: false, tooLarge: false, sensitive: false, etag: 'e' }
    expect(viewerFor('a.rs', text)).toBe('text')
    expect(viewerFor('a.bin', { ...text, content: null, binary: true })).toBe('binary')
    expect(viewerFor('a.log', { ...text, content: null, tooLarge: true, etag: null })).toBe('tooLarge')
    expect(viewerFor('.env', { ...text, content: null, sensitive: true, etag: null })).toBe('sensitive')
    expect(viewerFor('a.pdf', { ...text, content: null, binary: true })).toBe('pdf')
  })

  it('parses go-to input', () => {
    expect(parseGoto('main.rs:12')).toEqual({ query: 'main.rs', line: 12, column: undefined })
    expect(parseGoto(':7:3')).toEqual({ query: '', line: 7, column: 3 })
    expect(parseGoto('lib')).toEqual({ query: 'lib' })
  })

  it('names highlight.js languages', () => {
    expect(hljsLanguage('main.c')).toBe('c')
    expect(hljsLanguage('a/b.cxx')).toBe('cpp')
    expect(hljsLanguage('rtl/top.sv')).toBe('verilog')
    expect(hljsLanguage('rtl/alu.vhd')).toBe('vhdl')
    expect(hljsLanguage('Makefile')).toBe('makefile')
    expect(hljsLanguage('a.unknown')).toBe('')
  })

  it('fences code safely', () => {
    expect(fenced('a ``` b', 'rust')).toBe('````rust\na ``` b\n````\n')
    expect(fenced('x', '')).toBe('```\nx\n```\n')
  })
})

// The server's OS decides what an absolute path is (`GET /api/health`); before the report
// arrives, and on Linux, the rules Workbench always had apply.
describe('absolute paths by the server OS', () => {
  const onOs = (os: string | null) => setHealth(os ? { ok: true, service: 'workbench', version: '0', startedAt: 1, os } : null)
  afterEach(() => setHealth(null))

  // Monaco's `Uri.from({scheme: 'file', path}).toString()` for these paths.
  const WINDOWS_URIS: [string, string][] = [
    ['C:\\Users\\me\\x.rs', 'file:///~abs/C%3A%5CUsers%5Cme%5Cx.rs'],
    ['C:/x', 'file:///~abs/C%3A/x'],
    ['d:\\My Files\\é ü\\main.rs', 'file:///~abs/d%3A%5CMy%20Files%5C%C3%A9%20%C3%BC%5Cmain.rs'],
  ]

  it('keeps Linux model URIs and paths byte for byte', () => {
    for (const os of [null, 'linux']) {
      onOs(os)
      expect(modelUriString(null, '/etc/hosts')).toBe('file:///~abs/etc/hosts')
      expect(modelUriString(null, '/home/u/My Files/é.rs')).toBe('file:///~abs/home/u/My%20Files/%C3%A9.rs')
      expect(modelUriString('api', 'src/main.rs')).toBe('file:///api/src/main.rs')
      expect(parseModelUri('file:///~abs/etc/hosts')).toEqual({ projectId: null, path: '/etc/hosts' })
      expect(parseModelUri('file:///~abs/home/u/My%20Files/%C3%A9.rs')).toEqual({ projectId: null, path: '/home/u/My Files/é.rs' })
      expect(parseModelUri('file:///api/src/main.rs')).toEqual({ projectId: 'api', path: 'src/main.rs' })
      // On Linux `/C:\x` is a name under `/`: the URI parses back to it.
      for (const [path, uri] of WINDOWS_URIS) {
        expect(modelUriString(null, path)).toBe(uri)
        expect(parseModelUri(uri)).toEqual({ projectId: null, path: '/' + path })
      }
      expect(isAbsolutePath('/x')).toBe(true)
      expect(isAbsolutePath('C:\\x')).toBe(false)
      expect(basename('a\\b.rs')).toBe('a\\b.rs')
      expect(dirname('C:/x')).toBe('C:')
      expect(segments('/a\\b/c')).toEqual(['a\\b', 'c'])
      expect(samePath('/a/B.c', '/a/b.c')).toBe(false)
      expect(absolutePath('/home/u/p', 'src/a.rs')).toBe('/home/u/p/src/a.rs')
      expect(absolutePath('/home/u/p/', 'src/a.rs')).toBe('/home/u/p/src/a.rs')
      expect(absolutePath('/home/u/we\\ird', 'a.rs')).toBe('/home/u/we\\ird/a.rs')
      expect(absolutePath('/', 'a.rs')).toBe('/a.rs')
      expect(absolutePath('/home/u/p', '')).toBe('/home/u/p')
      expect(absolutePath('/home/u/p', '/tmp/x.md')).toBe('/tmp/x.md')
      expect(absolutePath('/home/u/p', 'C:\\x')).toBe('/home/u/p/C:\\x')
      expect(absolutePath(undefined, 'a.rs')).toBe('a.rs')
      expect(resolveLink('C:\\notes\\plan.md', 'a.png')).toBe('a.png')
    }
  })

  it('round-trips Windows drive paths through model URIs', () => {
    onOs('windows')
    for (const [path, uri] of WINDOWS_URIS) {
      expect(modelUriString(null, path)).toBe(uri)
      expect(parseModelUri(uri)).toEqual({ projectId: null, path })
      expect(modelFile(uri)).toEqual({ projectId: null, path })
      expect(modelFile(uri, true)).toBeNull()
    }
    // A Monaco `Uri` holds the decoded path.
    expect(parseModelUri({ scheme: 'file', path: '/~abs/C:\\Users\\me\\x.rs' } as Uri)).toEqual({ projectId: null, path: 'C:\\Users\\me\\x.rs' })
    // Project files and `/…` paths read as on Linux.
    expect(parseModelUri('file:///api/src/main.rs')).toEqual({ projectId: 'api', path: 'src/main.rs' })
    expect(parseModelUri('file:///~abs/etc/hosts')).toEqual({ projectId: null, path: '/etc/hosts' })
    expect(modelUriString(null, '/etc/hosts')).toBe('file:///~abs/etc/hosts')
    // Neither a drive-relative `C:x` nor a UNC path is a drive path.
    expect(parseModelUri('file:///~abs/C%3Ax')).toEqual({ projectId: null, path: '/C:x' })
    expect(parseModelUri('file:///~abs/%5C%5Cserver%5Cshare')).toEqual({ projectId: null, path: '/\\\\server\\share' })
  })

  it('reads drive paths on a Windows server', () => {
    onOs('windows')
    for (const p of ['C:\\x', 'c:/x', 'D:\\', '/x']) expect(isAbsolutePath(p)).toBe(true)
    for (const p of ['C:x', 'x\\y', 'src/a.rs', '\\\\server\\share\\x', '']) expect(isAbsolutePath(p)).toBe(false)
    expect(basename('C:\\Users\\me\\x.rs')).toBe('x.rs')
    expect(basename('C:/Users/me\\x.rs')).toBe('x.rs')
    expect(basename('src/a.rs')).toBe('a.rs')
    expect(dirname('C:\\Users\\me\\x.rs')).toBe('C:\\Users\\me')
    expect(dirname('C:\\x.rs')).toBe('C:\\')
    expect(dirname('C:/x.rs')).toBe('C:/')
    expect(dirname('src/a.rs')).toBe('src')
    expect(tabTitle('C:\\p\\server\\src\\files\\mod.rs')).toBe('files/mod.rs')
    expect(tabTitle('C:\\mod.rs')).toBe('mod.rs')
    expect(segments('C:\\Users/me\\x.rs')).toEqual(['C:', 'Users', 'me', 'x.rs'])
    expect(samePath('C:\\Users\\Me\\x.c', 'c:/users/me/X.C')).toBe(true)
    expect(samePath('src/Main.c', 'src/main.c')).toBe(true)
    expect(samePath('C:\\a\\x.c', 'C:\\b\\x.c')).toBe(false)
    // Only ASCII letters fold, as on the server.
    expect(samePath('C:\\É.c', 'C:\\é.c')).toBe(false)
  })

  it('builds Copy Path and drag and drop paths with the root separator on Windows', () => {
    onOs('windows')
    expect(absolutePath('C:\\Users\\me\\p', 'src/main.rs')).toBe('C:\\Users\\me\\p\\src\\main.rs')
    expect(absolutePath('C:\\Users\\me\\p\\', 'a.rs')).toBe('C:\\Users\\me\\p\\a.rs')
    expect(absolutePath('C:\\', 'a/b.rs')).toBe('C:\\a\\b.rs')
    expect(absolutePath('C:/Users/me/p', 'src/main.rs')).toBe('C:/Users/me/p/src/main.rs')
    expect(absolutePath('C:\\Users\\me\\p', '')).toBe('C:\\Users\\me\\p')
    expect(absolutePath('C:\\Users\\me\\p', 'D:\\x\\y.rs')).toBe('D:\\x\\y.rs')
    expect(absolutePath('C:\\Users\\me\\p', 'd:/x')).toBe('d:/x')
    expect(absolutePath(undefined, 'a.rs')).toBe('a.rs')
  })

  it('resolves links in a document at a drive path', () => {
    onOs('windows')
    expect(resolveLink('C:\\notes\\plan.md', 'img/a%20b.png')).toBe('C:\\notes\\img\\a b.png')
    expect(resolveLink('C:\\notes\\plan.md', '../x.md')).toBe('C:\\x.md')
    expect(resolveLink('C:\\plan.md', '../x.md')).toBeNull()
    expect(resolveLink('C:/notes/plan.md', './a.png')).toBe('C:/notes/a.png')
    expect(resolveLink('docs/guide.md', '../README.md')).toBe('README.md')
    expect(resolveLink('docs/guide.md', '/src/main.rs')).toBe('src/main.rs')
    expect(resolveLink('/tmp/x/plan.md', 'shot.png')).toBe('/tmp/x/shot.png')
  })
})

describe('vcs', () => {
  const f = (path: string, index: string, worktree: string, conflict = false) =>
    ({ path, index, worktree, conflict }) as GitStatus['files'][number]

  it('maps porcelain codes to CLion kinds', () => {
    expect(fileVcsKind(f('a', ' ', 'M'))).toBe('modified')
    expect(fileVcsKind(f('a', 'A', ' '))).toBe('added')
    expect(fileVcsKind(f('a', 'A', 'M'))).toBe('added')
    expect(fileVcsKind(f('a', '?', '?'))).toBe('untracked')
    expect(fileVcsKind(f('a', 'R', ' '))).toBe('renamed')
    expect(fileVcsKind(f('a', 'U', 'U', true))).toBe('conflict')
    expect(fileVcsKind(f('a', ' ', 'D'))).toBe('deleted')
    expect(fileVcsKind(f('a', ' ', ' '))).toBeNull()
  })

  it('colours folders that contain changes', () => {
    const idx = buildVcsIndex({
      branch: 'main', head: null, upstream: null, ahead: 0, behind: 0, state: 'clean', stashes: 0,
      files: [f('src/a/x.rs', ' ', 'M'), f('src/b/y.rs', 'U', 'U', true), f('new/', '?', '?'), f('build/', '!', '!')],
    })
    expect(idx.dirs.get('src')).toBe('conflict')
    expect(idx.dirs.get('src/a')).toBe('modified')
    expect(idx.dirs.has('build')).toBe(false)
    expect(vcsKindOf(idx, 'new/deep/file.txt')).toBe('untracked')
    expect(vcsKindOf(idx, 'build/out.o')).toBe('ignored')
    expect(vcsKindOf(idx, 'src/a/x.rs')).toBe('modified')
    expect(vcsKindOf(idx, 'src/a/z.rs')).toBeNull()
  })
})

describe('lineDiff', () => {
  const d = (a: string, b: string) => diffLines(splitLines(a), splitLines(b))

  it('finds added, modified and deleted blocks', () => {
    expect(d('a\nb\nc', 'a\nb\nc')).toEqual([])
    expect(d('a\nb\nc', 'a\nX\nb\nc')).toEqual([{ kind: 'added', start: 2, end: 2, baseStart: 1, baseEnd: 1 }])
    expect(d('a\nb\nc', 'a\nB\nc')).toEqual([{ kind: 'modified', start: 2, end: 2, baseStart: 1, baseEnd: 2 }])
    expect(d('a\nb\nc', 'a\nc')).toEqual([{ kind: 'deleted', start: 1, end: 1, baseStart: 1, baseEnd: 2 }])
    expect(d('a\nb\nc', 'b\nc')).toEqual([{ kind: 'deleted', start: 0, end: 0, baseStart: 0, baseEnd: 1 }])
    const blocks = d('1\n2\n3\n4\n5\n6', '1\nx\n3\n4\n6\n7')
    expect(blocks.map((b) => [b.kind, b.start, b.end])).toEqual([
      ['modified', 2, 2],
      ['deleted', 4, 4],
      ['added', 6, 6],
    ])
  })

  it('rolls blocks back', () => {
    const base = splitLines('a\nb\nc\nd')
    for (const cur of ['a\nX\nY\nc\nd', 'a\nc\nd', 'a\nb\nc\nd\ne', 'Z\na\nb\nc\nd']) {
      const lines = splitLines(cur)
      let out = lines
      for (const b of diffLines(base, lines).reverse()) out = rollbackBlock(out, base, b)
      expect(out).toEqual(base)
    }
  })

  it('degrades to one block when too different', () => {
    const a = Array.from({ length: 300 }, (_, i) => `a${i}`)
    const b = Array.from({ length: 300 }, (_, i) => `b${i}`)
    expect(diffLines(a, b, 10)).toEqual([{ kind: 'modified', start: 1, end: 300, baseStart: 0, baseEnd: 300 }])
  })

  it('handles large files with local edits quickly', () => {
    const a = Array.from({ length: 50_000 }, (_, i) => `line ${i}`)
    const b = a.slice()
    b[25_000] = 'changed'
    b.splice(40_000, 0, 'inserted')
    const t = performance.now()
    const blocks = diffLines(a, b)
    expect(performance.now() - t).toBeLessThan(500)
    expect(blocks.map((x) => x.kind)).toEqual(['modified', 'added'])
  })
})

describe('text', () => {
  it('computes minimal edits', () => {
    expect(minimalEdit('abc', 'abc')).toBeNull()
    expect(minimalEdit('hello world', 'hello brave world')).toEqual({ start: 6, end: 6, text: 'brave ' })
    expect(minimalEdit('aaa', 'aa')).toEqual({ start: 2, end: 3, text: '' })
    const e = minimalEdit('x😀y', 'x😁y')!
    expect('x😀y'.slice(0, e.start) + e.text + 'x😀y'.slice(e.end)).toBe('x😁y')
  })

  it('detects line endings', () => {
    expect(eolOf('a\nb\n')).toBe('LF')
    expect(eolOf('a\r\nb\r\n')).toBe('CRLF')
    expect(eolOf('a\r\nb\n')).toBe('Mixed')
    expect(eolOf('a')).toBeNull()
  })

  it('slugifies headings', () => {
    expect(slugify('Quick Start: v2!')).toBe('quick-start-v2')
  })
})

describe('tree', () => {
  const e = (path: string, kind: FileEntry['kind'] = 'file'): FileEntry => ({
    name: basename(path), path, kind, size: 0, mtime: 0, ignored: false, hidden: false, sensitive: false,
  })
  const dirs = {
    '': { entries: [e('src', 'dir'), e('docs', 'dir'), e('README.md')] },
    src: { entries: [e('src/app', 'dir'), e('src/main.rs')] },
    'src/app': { entries: [e('src/app/mod.rs')] },
    docs: { entries: [e('docs/guide.md')] },
  }

  it('flattens expanded directories', () => {
    const rows = flattenTree(dirs, new Set(['src']))
    expect(rows.map((r) => `${r.depth}:${r.entry.path}`)).toEqual(['0:src', '1:src/app', '1:src/main.rs', '0:docs', '0:README.md'])
    expect(rows[0].expanded).toBe(true)
    expect(rows[1].expanded).toBe(false)
  })

  it('adds a placeholder after truncated folders', () => {
    const rows = flattenTree({ '': { entries: [e('a')], truncated: true, total: 5001 } }, new Set())
    expect(rows).toHaveLength(2)
    expect(rows[1].more).toBe(5000)
  })

  it('filters loaded entries and keeps ancestors', () => {
    const rows = flattenTree(dirs, new Set(), 'mod')
    expect(rows.map((r) => r.entry.path)).toEqual(['src', 'src/app', 'src/app/mod.rs'])
    expect(rows.map((r) => r.match)).toEqual([false, false, true])
  })

  it('reloads only affected loaded directories', () => {
    expect(dirsToReload(['src/main.rs', 'other/x.rs', 'docs'], Object.keys(dirs)).sort()).toEqual(['', 'docs', 'src'])
    expect(dirsToReload(['a.txt'], ['src'])).toEqual([])
    expect(dirsToReload([], ['', 'src'], true)).toEqual(['', 'src'])
    expect(Object.keys(pruneDirs(dirs, 'src'))).toEqual(['', 'docs'])
  })
})

describe('search rows', () => {
  const hit = (path: string, line: number, column: number, endColumn: number, preview: string, previewOffset = 0) => ({
    path, line, column, endColumn, preview, previewOffset,
  })

  it('groups by file and line', () => {
    const m = [hit('a.rs', 1, 1, 4, 'foo foo'), hit('a.rs', 1, 5, 8, 'foo foo'), hit('a.rs', 3, 1, 4, 'foo'), hit('b.rs', 2, 1, 4, 'foo')]
    const { rows, files } = groupRows(m, new Set())
    expect(files).toBe(2)
    expect(rows.map((r) => (r.kind === 'file' ? `F:${r.path}:${r.count}` : `L:${r.line}:${r.hits.length}`))).toEqual([
      'F:a.rs:3', 'L:1:2', 'L:3:1', 'F:b.rs:1', 'L:2:1',
    ])
    expect(groupRows(m, new Set(['a.rs'])).rows).toHaveLength(3)
  })

  it('highlights every hit and trims indentation', () => {
    const p = previewParts([hit('a', 1, 5, 8, '    foo(foo)'), hit('a', 1, 9, 12, '    foo(foo)')])
    expect(p.cut).toBe(false)
    expect(p.segments).toEqual([
      { text: 'foo', hit: true },
      { text: '(', hit: false },
      { text: 'foo', hit: true },
      { text: ')', hit: false },
    ])
    const w = previewParts([hit('a', 1, 1001, 1007, 'xxneedleyy', 998)])
    expect(w.cut).toBe(true)
    expect(w.segments.find((s) => s.hit)?.text).toBe('needle')
  })
})

describe('quick open', () => {
  const found = (...paths: [string, number][]) => paths.map(([path, score]) => ({ path, score, positions: [] }))

  it('lists recent files without a query and floats close recent matches up', () => {
    expect(rankItems('', [], ['a.ts', 'b.ts']).map((i) => [i.path, i.recent])).toEqual([
      ['a.ts', true],
      ['b.ts', true],
    ])
    const items = rankItems('x', found(['x1', 100], ['x2', 80], ['x3', 10]), ['x3', 'x2'])
    expect(items.map((i) => i.path)).toEqual(['x2', 'x1', 'x3'])
  })

  it('moves the selection onto the new results (Enter after typing opens the top one)', () => {
    // Opened with recent files: the first one is selected.
    let sel = nextSelection('', ['todo.md', 'util.py'], true)
    expect(sel).toBe('todo.md')
    // Typing: the recent list goes away before the results arrive.
    sel = nextSelection(sel, [], false)
    expect(sel).toBe('')
    // The results for the query arrive.
    sel = nextSelection(sel, ['index.html'], true)
    expect(sel).toBe('index.html')
    // A stale selection that is still listed gives way to the new top result…
    expect(nextSelection('src/index.ts', ['index.html', 'src/index.ts'], true)).toBe('index.html')
    // …and one that disappeared is replaced even without a new query.
    expect(nextSelection('todo.md', ['index.html'], false)).toBe('index.html')
  })

  it('keeps what the user picked while the list only refreshes', () => {
    expect(nextSelection('b', ['a', 'b', 'c'], false)).toBe('b')
    expect(nextSelection('goto-line', ['goto-line'], false)).toBe('goto-line')
    expect(nextSelection('a', [], true)).toBe('')
  })
})
