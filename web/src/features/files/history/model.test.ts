import { describe, expect, it } from 'vitest'
import { clockTime, describeEntry, groupByDay, historyPanelId, historyTitle, keepSelection, revertLines, shortWhen } from './model'
import type { HistoryEntry } from './api'

const entry = (id: number, ts: number, kind: HistoryEntry['kind'] = 'disk', extra: Partial<HistoryEntry> = {}): HistoryEntry => ({ id, ts, path: 'a.rs', kind, size: 1, ...extra })

describe('describeEntry', () => {
  it('labels every kind', () => {
    expect(describeEntry({ kind: 'save' })).toBe('Saved in Workbench')
    expect(describeEntry({ kind: 'save', label: 'Replace in Files' })).toBe('Replace in Files')
    expect(describeEntry({ kind: 'disk' })).toBe('Changed on disk')
    expect(describeEntry({ kind: 'agent', who: 'Claude · fix parser' })).toBe('External (agent) edit · Claude · fix parser')
    expect(describeEntry({ kind: 'agent' })).toBe('External (agent) edit')
    expect(describeEntry({ kind: 'base' })).toBe('Opened in Workbench')
    expect(describeEntry({ kind: 'deleted' })).toBe('Deleted')
    expect(describeEntry({ kind: 'label', label: 'Before refactoring' })).toBe('Before refactoring')
  })
})

describe('groupByDay', () => {
  it('groups newest-first entries into Today, Yesterday and dates', () => {
    const now = new Date(2026, 8, 27, 15, 0, 0).getTime()
    const today = new Date(2026, 8, 27, 9, 30).getTime()
    const yesterday = new Date(2026, 8, 26, 23, 59).getTime()
    const older = new Date(2026, 8, 20, 12, 0).getTime()
    const groups = groupByDay([entry(4, now), entry(3, today), entry(2, yesterday), entry(1, older)], now)
    expect(groups.map((g) => [g.label, g.entries.map((e) => e.id)])).toEqual([
      ['Today', [4, 3]],
      ['Yesterday', [2]],
      [new Date(older).toLocaleDateString(undefined, { weekday: 'long', month: 'short', day: 'numeric' }), [1]],
    ])
    expect(groupByDay([], now)).toEqual([])
  })
})

describe('ids and titles', () => {
  it('follow the panel convention', () => {
    expect(historyPanelId('p', 'src/a.rs', false)).toBe('localHistory:p:src/a.rs')
    expect(historyPanelId('p', 'src', true)).toBe('localHistory:p:src/')
    expect(historyPanelId('p', '', true)).toBe('localHistory:p:/')
    expect(historyTitle('', true)).toBe('Recent Changes')
    expect(historyTitle('src/lib', true)).toBe('lib/ (Local History)')
    expect(historyTitle('src/a.rs', false)).toBe('a.rs (Local History)')
    expect(clockTime(new Date(2026, 0, 2, 3, 4, 5).getTime())).toBe('03:04:05')
  })
})

describe('revertLines', () => {
  const revision = ['a', 'b', 'c', 'd', 'e', 'f'].join('\n')
  const current = ['a', 'B', 'c', 'd', 'E', 'f', 'g'].join('\n')

  it('reverts only the changes inside the selection', () => {
    expect(revertLines(current, revision, 2, 2)).toBe(['a', 'b', 'c', 'd', 'E', 'f', 'g'].join('\n'))
    expect(revertLines(current, revision, 5, 7)).toBe(['a', 'B', 'c', 'd', 'e', 'f'].join('\n'))
    expect(revertLines(current, revision, 1, 7)).toBe(revision)
    expect(revertLines(current, revision, 3, 4)).toBeNull()
  })

  it('restores deleted lines next to the cursor and keeps CRLF', () => {
    const cur = 'a\r\nd\r\n'
    const rev = 'a\r\nb\r\nc\r\nd\r\n'
    expect(revertLines(cur, rev, 1, 1)).toBe(rev)
    expect(revertLines(cur, rev, 2, 2)).toBe(rev)
  })
})

describe('keepSelection', () => {
  it('keeps a selection that is still listed, else picks the newest revision', () => {
    const list = [entry(3, 3, 'label', { label: 'L' }), entry(2, 2), entry(1, 1)]
    expect(keepSelection(list, 1)).toBe(1)
    expect(keepSelection(list, 9)).toBe(2)
    expect(keepSelection(list, null)).toBe(2)
    expect(keepSelection([], null)).toBeNull()
  })

  it('skips versions identical to the file on disk', () => {
    const list = [entry(3, 3, 'disk', { hash: 'now' }), entry(2, 2, 'save', { hash: 'old' }), entry(1, 1, 'base', { hash: 'older' })]
    expect(keepSelection(list, null, 'now')).toBe(2)
    expect(keepSelection([entry(3, 3, 'disk', { hash: 'now' })], null, 'now')).toBe(3)
    expect(keepSelection(list, 3, 'now')).toBe(3)
  })
})

describe('shortWhen', () => {
  it('shows the time today and the date otherwise', () => {
    const now = new Date(2026, 8, 27, 15).getTime()
    expect(shortWhen(new Date(2026, 8, 27, 9, 5, 7).getTime(), now)).toBe('09:05:07')
    expect(shortWhen(new Date(2026, 8, 20, 9, 5, 7).getTime(), now)).toMatch(/09:05$/)
  })
})
