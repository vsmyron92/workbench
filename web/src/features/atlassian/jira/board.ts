// Pure board logic: which column an issue sits in (by status id, from the board
// configuration), and which workflow transitions move it into another column.

import type { BoardColumn, IssueSummary, JiraTransition } from '../api'

/** Index of the column holding `statusId`, or -1 when no column maps it. */
export function columnOf(columns: BoardColumn[], statusId: string | null | undefined): number {
  if (!statusId) return -1
  return columns.findIndex((c) => c.statusIds.includes(statusId))
}

export interface Grouped {
  columns: IssueSummary[][]
  /** Issues whose status no column maps (the board hides them; shown apart here). */
  unmapped: IssueSummary[]
}

export function groupByColumn(columns: BoardColumn[], issues: IssueSummary[]): Grouped {
  const out: Grouped = { columns: columns.map(() => []), unmapped: [] }
  for (const i of issues) {
    const c = columnOf(columns, i.status?.id)
    if (c < 0) out.unmapped.push(i)
    else out.columns[c].push(i)
  }
  return out
}

/** Transitions that land the issue in `column` (their target status is one of its statuses). */
export function transitionsInto(transitions: JiraTransition[], column: BoardColumn): JiraTransition[] {
  return transitions.filter((t) => !!t.to?.id && column.statusIds.includes(t.to.id))
}

/** A column's WIP limit state: over the max (or under the min) turns it red. */
export function limitState(column: BoardColumn, count: number): 'over' | 'under' | null {
  if (column.max !== null && column.max > 0 && count > column.max) return 'over'
  if (column.min !== null && column.min > 0 && count < column.min) return 'under'
  return null
}

/** Move an issue into another column optimistically: it takes the column's first status. */
export function moveIssue(issues: IssueSummary[], key: string, column: BoardColumn, statusName?: string): IssueSummary[] {
  const id = column.statusIds[0]
  if (!id) return issues
  return issues.map((i) =>
    i.key === key ? { ...i, status: { id, name: statusName ?? column.name, category: i.status?.category ?? 'indeterminate' } } : i,
  )
}
