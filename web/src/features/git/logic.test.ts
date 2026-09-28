import { describe, expect, it } from 'vitest'
import {
  buildFileTree,
  diffPanelId,
  groupStatus,
  hunkAtLine,
  parseConflictBlocks,
  resolveConflictBlock,
  sectionCode,
  splitLinesKeep,
  splitMessage,
  statusClass,
  type TreeDir,
} from './logic'
import type { ChangedFile, GitStatusFile } from './types'

const f = (path: string, index: GitStatusFile['index'], worktree: GitStatusFile['worktree'], conflict = false): GitStatusFile => ({
  path,
  index,
  worktree,
  conflict,
})

describe('groupStatus', () => {
  it('puts files in CLion change lists', () => {
    const s = groupStatus([
      f('b.rs', 'M', 'M'),
      f('a.rs', 'A', ' '),
      f('c.rs', ' ', 'D'),
      f('u.txt', '?', '?'),
      f('x.rs', 'U', 'U', true),
      f('ign', '!', '!'),
    ])
    expect(s.staged.map((x) => x.path)).toEqual(['a.rs', 'b.rs'])
    expect(s.unstaged.map((x) => x.path)).toEqual(['b.rs', 'c.rs'])
    expect(s.untracked.map((x) => x.path)).toEqual(['u.txt'])
    expect(s.conflicts.map((x) => x.path)).toEqual(['x.rs'])
    expect(sectionCode(s.unstaged[1], 'unstaged')).toBe('D')
    expect(sectionCode(s.staged[0], 'staged')).toBe('A')
    expect(statusClass('D')).toBe('git-c-deleted')
  })
})

describe('panel ids', () => {
  it('follow the documented convention', () => {
    expect(diffPanelId('p', 'working', 'src/a.rs')).toBe('diff:p:working::src/a.rs')
    expect(diffPanelId('p', 'commit', 'a', { sha: 'abc' })).toBe('diff:p:commit:abc:a')
    expect(diffPanelId('p', 'compare', 'a', { base: 'main', head: 'dev' })).toBe('diff:p:compare:main..dev:a')
  })
})

describe('hunkAtLine', () => {
  const hunks = [
    { header: '', oldStart: 1, oldLines: 3, newStart: 1, newLines: 3 },
    { header: '', oldStart: 20, oldLines: 3, newStart: 21, newLines: 5 },
    { header: '', oldStart: 40, oldLines: 2, newStart: 43, newLines: 0 },
  ]
  it('finds the hunk at or before a line', () => {
    expect(hunkAtLine(hunks, 1)).toBe(0)
    expect(hunkAtLine(hunks, 15)).toBe(0)
    expect(hunkAtLine(hunks, 22)).toBe(1)
    expect(hunkAtLine(hunks, 100)).toBe(2)
    expect(hunkAtLine([], 3)).toBe(-1)
  })
})

describe('buildFileTree', () => {
  it('nests and compresses directories', () => {
    const files: ChangedFile[] = ['src/git/a.rs', 'src/git/b.rs', 'src/main.rs', 'README.md'].map((path) => ({
      path,
      status: 'M',
      additions: 1,
      deletions: 0,
      binary: false,
    }))
    const tree = buildFileTree(files)
    expect(tree.map((n) => n.name)).toEqual(['src', 'README.md'])
    const src = tree[0] as TreeDir
    expect(src.count).toBe(3)
    expect(src.children.map((n) => n.name)).toEqual(['git', 'main.rs'])
    const deep = buildFileTree([{ path: 'a/b/c/d.txt', status: 'A', additions: 1, deletions: 0, binary: false }])
    expect(deep[0].name).toBe('a/b/c')
  })
})

describe('conflict markers', () => {
  const text = 'top\n<<<<<<< HEAD\nours 1\nours 2\n=======\ntheirs\n>>>>>>> feature\nmiddle\n<<<<<<< HEAD\nA\n||||||| base\nB\n=======\nC\n>>>>>>> x\nend'
  it('parses merge and diff3 blocks', () => {
    const b = parseConflictBlocks(text)
    expect(b.length).toBe(2)
    expect(b[0].ours).toEqual(['ours 1\n', 'ours 2\n'])
    expect(b[0].theirs).toEqual(['theirs\n'])
    expect(b[0].base).toBeNull()
    expect(b[1].base).toEqual(['B\n'])
    expect([b[1].start, b[1].end]).toEqual([8, 14])
  })
  it('resolves one block at a time', () => {
    const once = resolveConflictBlock(text, 0, 'theirs')
    expect(once.startsWith('top\ntheirs\nmiddle\n')).toBe(true)
    expect(parseConflictBlocks(once).length).toBe(1)
    const both = resolveConflictBlock(once, 0, 'both')
    expect(both).toBe('top\ntheirs\nmiddle\nA\nC\nend')
    expect(resolveConflictBlock(text, 5, 'ours')).toBe(text)
  })
  it('ignores marker-like text that is not a block', () => {
    expect(parseConflictBlocks('<<<<<<<< not a marker\n=======\n')).toEqual([])
    expect(parseConflictBlocks('<<<<<<< HEAD\nno end\n')).toEqual([])
  })
  it('keeps CRLF line endings', () => {
    expect(splitLinesKeep('a\r\nb\r\n')).toEqual(['a\r\n', 'b\r\n'])
    const crlf = 'x\r\n<<<<<<< a\r\n1\r\n=======\r\n2\r\n>>>>>>> b\r\ny\r\n'
    expect(resolveConflictBlock(crlf, 0, 'ours')).toBe('x\r\n1\r\ny\r\n')
  })
})

describe('splitMessage', () => {
  it('splits subject and body', () => {
    expect(splitMessage('Fix it\n\nBecause.')).toEqual({ subject: 'Fix it', body: 'Because.' })
    expect(splitMessage('One line')).toEqual({ subject: 'One line', body: '' })
  })
})
