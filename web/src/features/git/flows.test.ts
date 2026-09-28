import { describe, expect, it } from 'vitest'
import { defaultIncluded, groupChangelists, groupState, headCode } from './changelistView'
import { hunkKeys, lineKey, linesInRanges, parseLineKey, prune, refsOf, toggleKeys } from './lineSelection'
import { bisectMark, bisectProgress } from './logic'
import {
  chainHead,
  combinedMessage,
  editMessage,
  firstChanged,
  initialRows,
  moveRow,
  problems,
  resetSquashMessage,
  rewrittenPushed,
  setAction,
  squashMessage,
  summary,
  toEntries,
} from './rebasePlan'
import type { DiffLine, GitStatusFile, PlanCommit } from './types'

const commit = (sha: string, pushed = false): PlanCommit => ({
  sha,
  subject: `subject ${sha}`,
  message: `subject ${sha}\n\nbody ${sha}`,
  author: 'A',
  email: 'a@x',
  time: 0,
  pushed,
})

describe('interactive rebase plan', () => {
  const commits = [commit('a', true), commit('b', true), commit('c'), commit('d')]

  it('moves rows and finds the first changed position', () => {
    const rows = initialRows(commits)
    expect(firstChanged(rows, commits)).toBe(4)
    const moved = moveRow(rows, 3, 1)
    expect(moved.map((r) => r.sha)).toEqual(['a', 'd', 'b', 'c'])
    expect(firstChanged(moved, commits)).toBe(1)
    expect(rewrittenPushed(moved, commits, false)).toBe(1)
    expect(rewrittenPushed(rows, commits, true)).toBe(2)
    expect(moveRow(rows, 0, 0)).toBe(rows)
  })

  it('prepares messages for reword and squash', () => {
    let rows = setAction(initialRows(commits), 2, 'reword')
    expect(rows[2].message).toBe(commits[2].message)
    rows = editMessage(rows, 2, 'new c')
    rows = setAction(rows, 3, 'squash')
    expect(chainHead(rows, 3)).toBe(2)
    expect(squashMessage(rows, 3)).toEqual({ text: 'new c\n\nsubject d\n\nbody d', edited: false, stale: false })
    expect(combinedMessage(rows, 3)).toBe('new c\n\nsubject d\n\nbody d')
    expect(toEntries(rows)[2]).toEqual({ sha: 'c', action: 'reword', message: 'new c' })
    expect(toEntries(rows)[3]).toEqual({ sha: 'd', action: 'squash', message: 'new c\n\nsubject d\n\nbody d' })
    expect(toEntries(rows)[0]).toEqual({ sha: 'a', action: 'pick' })
    expect(summary(rows, commits)).toBe('1 reworded, 1 squashed')
  })

  it('keeps one squash message per chain that follows the chain until edited', () => {
    const cs = [commit('a', true), commit('b'), commit('c'), commit('d')]
    const msg = (...shas: string[]) => shas.map((x) => `subject ${x}\n\nbody ${x}`).join('\n\n')
    let rows = setAction(initialRows(cs), 2, 'squash')
    expect(squashMessage(rows, 2)?.text).toBe(msg('b', 'c'))
    // b dropped: c melds into a now, and the message follows.
    rows = setAction(rows, 1, 'drop')
    expect(squashMessage(rows, 2)?.text).toBe(msg('a', 'c'))
    // A second squash: one editor, on the chain's last squash; only that row sends a message.
    rows = setAction(rows, 3, 'squash')
    expect(squashMessage(rows, 2)).toBeNull()
    expect(squashMessage(rows, 3)?.text).toBe(msg('a', 'c', 'd'))
    expect(toEntries(rows).map((e) => e.message)).toEqual([undefined, undefined, undefined, msg('a', 'c', 'd')])
    // Edited: kept, and flagged stale once the chain's commits change.
    rows = editMessage(rows, 3, 'A, C and D')
    expect(squashMessage(rows, 3)).toEqual({ text: 'A, C and D', edited: true, stale: false })
    rows = setAction(rows, 1, 'pick')
    expect(squashMessage(rows, 3)).toEqual({ text: 'A, C and D', edited: true, stale: true })
    expect(toEntries(rows)[3].message).toBe('A, C and D')
    rows = resetSquashMessage(rows, 3)
    expect(squashMessage(rows, 3)).toEqual({ text: msg('b', 'c', 'd'), edited: false, stale: false })
  })

  it('counts a pushed commit that a fixup amends, and skipped pushed commits', () => {
    const cs = [commit('a', true), commit('b')]
    expect(firstChanged(setAction(initialRows(cs), 1, 'fixup'), cs)).toBe(1)
    expect(rewrittenPushed(setAction(initialRows(cs), 1, 'fixup'), cs, false)).toBe(1)
    expect(rewrittenPushed(setAction(initialRows(cs), 1, 'reword'), cs, false)).toBe(0)
    expect(rewrittenPushed(initialRows(cs), cs, true, [commit('s', true), commit('t')])).toBe(2)
  })

  it('reports plans git would refuse', () => {
    let rows = setAction(initialRows(commits), 0, 'drop')
    rows = setAction(rows, 1, 'fixup')
    expect(chainHead(rows, 1)).toBe(-1)
    expect(problems(rows)[0]).toMatch(/cannot be fixed up/)
    rows = setAction(initialRows(commits), 1, 'reword').map((r, i) => (i === 1 ? { ...r, message: '  ' } : r))
    expect(problems(rows)[0]).toMatch(/empty/)
    expect(problems(initialRows(commits))).toEqual([])
  })
})

describe('line selection', () => {
  // A replaced block (old 2 → new 2-3) and a pure deletion before new line 7.
  const lines: DiffLine[] = [
    { hunk: 0, kind: 'del', line: 2, at: 2 },
    { hunk: 0, kind: 'add', line: 2, at: 2 },
    { hunk: 0, kind: 'add', line: 3, at: 3 },
    { hunk: 1, kind: 'del', line: 8, at: 7 },
  ]

  it('maps editor ranges to changed lines', () => {
    expect(linesInRanges(lines, [{ start: 3, end: 3 }], [])).toEqual(['a:3'])
    expect(linesInRanges(lines, [{ start: 2, end: 3 }], [])).toEqual(['d:2', 'a:2', 'a:3'])
    expect(linesInRanges(lines, [], [{ start: 8, end: 9 }])).toEqual(['d:8'])
    expect(linesInRanges(lines, [{ start: 7, end: 7 }], [])).toEqual(['d:8'])
  })

  it('keys, toggles and prunes', () => {
    expect(lineKey({ kind: 'add', line: 4 })).toBe('a:4')
    expect(parseLineKey('d:12')).toEqual({ kind: 'del', line: 12 })
    expect(parseLineKey('x:1')).toBeNull()
    expect(refsOf(['a:1', 'bad', 'd:2'])).toEqual([
      { kind: 'add', line: 1 },
      { kind: 'del', line: 2 },
    ])
    const hunk0 = hunkKeys(lines, 0)
    let sel = toggleKeys(new Set(['a:2']), hunk0)
    expect([...sel].sort()).toEqual(['a:2', 'a:3', 'd:2'])
    sel = toggleKeys(sel, hunk0)
    expect(sel.size).toBe(0)
    expect([...prune(new Set(['a:2', 'a:99']), lines)]).toEqual(['a:2'])
  })
})

describe('changelists', () => {
  const f = (path: string, index: GitStatusFile['index'], worktree: GitStatusFile['worktree'], conflict = false): GitStatusFile => ({
    path,
    index,
    worktree,
    conflict,
  })

  it('puts every file in one place', () => {
    const files = [f('a', 'M', ' '), f('b', ' ', 'M'), f('c', '?', '?'), f('d', 'U', 'U', true), f('e', ' ', 'M'), f('i', '!', '!')]
    const g = groupChangelists(files, {
      active: 'x',
      lists: [
        { id: 'default', name: 'Changes', comment: '', active: false, files: ['a'] },
        { id: 'x', name: 'Feature', comment: '', active: true, files: ['b'] },
      ],
    })
    expect(g.conflicts.map((x) => x.path)).toEqual(['d'])
    expect(g.unversioned.map((x) => x.path)).toEqual(['c'])
    expect(g.lists[0].files.map((x) => x.path)).toEqual(['a'])
    // e is not assigned yet: it shows in the active list.
    expect(g.lists[1].files.map((x) => x.path)).toEqual(['b', 'e'])
    expect([...defaultIncluded(g)]).toEqual(['b', 'e'])
    // Without changelists: one default list.
    expect(groupChangelists(files, undefined).lists[0].files.map((x) => x.path)).toEqual(['a', 'b', 'e'])
  })

  it('shows the status against HEAD and group check states', () => {
    expect(headCode(f('a', 'M', 'M'))).toBe('M')
    expect(headCode(f('a', 'A', 'M'))).toBe('A')
    expect(headCode(f('a', ' ', 'D'))).toBe('D')
    expect(headCode(f('a', 'R', ' '))).toBe('R')
    expect(groupState(['a', 'b'], new Set(['a', 'b']), new Set())).toBe('all')
    expect(groupState(['a', 'b'], new Set(['a', 'b']), new Set(['a']))).toBe('some')
    expect(groupState(['a', 'b'], new Set(['a']), new Set())).toBe('some')
    expect(groupState(['a'], new Set(), new Set())).toBe('none')
  })
})

describe('bisect', () => {
  const s = { active: true, bad: 'b', good: ['g'], skipped: ['s'], current: 'c', result: null as string | null }
  it('marks commits', () => {
    expect(bisectMark(s, 'b')).toBe('bad')
    expect(bisectMark(s, 'g')).toBe('good')
    expect(bisectMark(s, 's')).toBe('skip')
    expect(bisectMark(s, 'c')).toBe('current')
    expect(bisectMark({ ...s, result: 'c' }, 'c')).toBe('result')
    expect(bisectMark({ ...s, active: false }, 'b')).toBeNull()
    expect(bisectProgress(3, 2)).toBe('3 revisions left (about 2 steps)')
    expect(bisectProgress(null, null)).toMatch(/Mark/)
  })
})
