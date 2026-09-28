import { describe, expect, it } from 'vitest'
import type { BoardColumn, IssueSummary, JiraTransition } from '../api'
import { columnOf, groupByColumn, limitState, moveIssue, transitionsInto } from './board'

const col = (name: string, ids: string[], min: number | null = null, max: number | null = null): BoardColumn => ({ name, statusIds: ids, min, max })
const columns = [col('To Do', ['10000']), col('In Progress', ['3', '10002'], null, 1), col('Done', ['10001'])]
const issue = (key: string, statusId: string | null): IssueSummary => ({
  key,
  id: key,
  summary: key,
  status: statusId ? { id: statusId, name: statusId, category: 'new' } : null,
  assignee: null,
  priority: null,
  issueType: null,
  updated: null,
  labels: [],
  projectKey: 'WB',
})

describe('board columns', () => {
  it('places issues by status id and keeps the unmapped apart', () => {
    const g = groupByColumn(columns, [issue('A-1', '10000'), issue('A-2', '10002'), issue('A-3', '99'), issue('A-4', null), issue('A-5', '3')])
    expect(g.columns.map((c) => c.map((i) => i.key))).toEqual([['A-1'], ['A-2', 'A-5'], []])
    expect(g.unmapped.map((i) => i.key)).toEqual(['A-3', 'A-4'])
    expect(columnOf(columns, undefined)).toBe(-1)
  })

  it('finds the transitions into a column', () => {
    const t = (id: string, to: string | null): JiraTransition => ({ id, name: id, to: to ? { id: to, name: to, category: 'x' } : null, hasScreen: false })
    const ts = [t('11', '10000'), t('21', '3'), t('22', '10002'), t('31', '10001'), t('41', null)]
    expect(transitionsInto(ts, columns[1]).map((x) => x.id)).toEqual(['21', '22'])
    expect(transitionsInto(ts, col('Empty', []))).toEqual([])
  })

  it('flags WIP limits and moves cards optimistically', () => {
    expect(limitState(columns[1], 2)).toBe('over')
    expect(limitState(columns[1], 1)).toBeNull()
    expect(limitState(col('X', [], 2, null), 1)).toBe('under')
    const moved = moveIssue([issue('A-1', '10000'), issue('A-2', '10000')], 'A-1', columns[2])
    expect(moved.map((i) => i.status?.id)).toEqual(['10001', '10000'])
    expect(moveIssue([issue('A-1', '10000')], 'A-1', col('Empty', []))[0].status?.id).toBe('10000')
  })
})
