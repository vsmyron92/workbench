// Pure helpers of the interactive rebase dialog (unit-tested in flows.test.ts). The
// server checks the same rules again (server/src/git/rebase_i.rs).

import type { PlanCommit, RebaseAction, RebaseEntry } from './types'

export interface RebaseRow {
  sha: string
  action: RebaseAction
  /** reword: the new message; squash: the combined commit's message once the user edited it. */
  message: string
  commit: PlanCommit
  /** squash: the user edited the combined message (else it follows the chain). */
  edited?: boolean
  /** squash: the chain the edited message was written for (`chainKey`). */
  editedFor?: string
}

export const ACTIONS: { id: RebaseAction; label: string; key: string; hint: string }[] = [
  { id: 'pick', label: 'Pick', key: 'p', hint: 'Use the commit as it is' },
  { id: 'reword', label: 'Reword', key: 'r', hint: 'Use the commit with a new message' },
  { id: 'edit', label: 'Edit', key: 'e', hint: 'Stop after this commit to amend it or add commits' },
  { id: 'squash', label: 'Squash', key: 's', hint: 'Meld into the commit above and combine the messages' },
  { id: 'fixup', label: 'Fixup', key: 'f', hint: 'Meld into the commit above, keeping its message' },
  { id: 'drop', label: 'Drop', key: 'd', hint: 'Remove the commit' },
]

export const actionByKey = (key: string): RebaseAction | undefined => ACTIONS.find((a) => a.key === key.toLowerCase())?.id

export function initialRows(commits: PlanCommit[]): RebaseRow[] {
  return commits.map((c) => ({ sha: c.sha, action: 'pick', message: '', commit: c }))
}

/** Move the row at `from` so it ends up at index `to`. */
export function moveRow(rows: RebaseRow[], from: number, to: number): RebaseRow[] {
  if (from === to || from < 0 || from >= rows.length) return rows
  const next = rows.slice()
  const [r] = next.splice(from, 1)
  next.splice(Math.max(0, Math.min(to, next.length)), 0, r)
  return next
}

/** The kept row a squash/fixup at `i` melds into (-1: none, i.e. invalid). */
export function chainHead(rows: RebaseRow[], i: number): number {
  for (let j = i - 1; j >= 0; j--) {
    const a = rows[j].action
    if (a === 'drop') continue
    if (a === 'squash' || a === 'fixup') continue
    return j
  }
  return -1
}

/** Git's combined message for a squash at `i`: the chain head's message and every squash's, fixups left out. */
export function combinedMessage(rows: RebaseRow[], i: number): string {
  const head = chainHead(rows, i)
  if (head < 0) return rows[i].commit.message
  const parts: string[] = []
  const h = rows[head]
  parts.push(h.action === 'reword' && h.message.trim() ? h.message.trim() : h.commit.message)
  for (let j = head + 1; j <= i; j++) {
    const r = rows[j]
    if (r.action === 'squash') parts.push(r.commit.message)
  }
  return parts.join('\n\n')
}

/**
 * The chain the squash/fixup at `i` belongs to: the kept row it melds into, the chain's
 * squash/fixup rows, and `end`, its last squash row (where git asks for the combined
 * message; -1 when the chain has only fixups). Null when `i` is not a squash/fixup or
 * has nothing to meld into.
 */
export function chainOf(rows: RebaseRow[], i: number): { head: number; members: number[]; end: number } | null {
  const a = rows[i]?.action
  if (a !== 'squash' && a !== 'fixup') return null
  const head = chainHead(rows, i)
  if (head < 0) return null
  const members: number[] = []
  for (let j = head + 1; j < rows.length; j++) {
    const b = rows[j].action
    if (b === 'drop') continue
    if (b !== 'squash' && b !== 'fixup') break
    members.push(j)
  }
  const squashes = members.filter((j) => rows[j].action === 'squash')
  return { head, members, end: squashes.length ? squashes[squashes.length - 1] : -1 }
}

/** What a chain's combined message is made of (its head and squashed commits): an edited message is stale once this changes. */
export function chainKey(rows: RebaseRow[], c: { head: number; members: number[] }): string {
  return [rows[c.head].sha, ...c.members.filter((j) => rows[j].action === 'squash').map((j) => rows[j].sha)].join(' ')
}

/**
 * The message editor of a squash row: shown only on its chain's last squash row. The
 * text is the user's edit (from any squash row of the chain), else the combined message
 * derived from the current rows. `stale`: edited for other commits than the chain has now.
 */
export function squashMessage(rows: RebaseRow[], i: number): { text: string; edited: boolean; stale: boolean } | null {
  const c = chainOf(rows, i)
  if (!c || c.end !== i) return null
  const owner = [...c.members].reverse().find((j) => rows[j].action === 'squash' && rows[j].edited)
  if (owner === undefined) return { text: combinedMessage(rows, i), edited: false, stale: false }
  const r = rows[owner]
  return { text: r.message, edited: true, stale: r.editedFor !== chainKey(rows, c) }
}

/** The user typed in the message editor of row `i` (a reword, or a chain's last squash). */
export function editMessage(rows: RebaseRow[], i: number, text: string): RebaseRow[] {
  const next = rows.slice()
  if (next[i].action !== 'squash') {
    next[i] = { ...next[i], message: text }
    return next
  }
  const c = chainOf(next, i)
  // One edited message per chain, kept on the row that shows the editor.
  for (const j of c?.members ?? []) if (j !== i && next[j].edited) next[j] = { ...next[j], edited: false, editedFor: undefined, message: '' }
  next[i] = { ...next[i], message: text, edited: true, editedFor: c ? chainKey(next, c) : undefined }
  return next
}

/** Drop a chain's edited message: it follows the commits again. */
export function resetSquashMessage(rows: RebaseRow[], i: number): RebaseRow[] {
  const c = chainOf(rows, i)
  return rows.map((r, j) => (c?.members.includes(j) && r.edited ? { ...r, edited: false, editedFor: undefined, message: '' } : r))
}

/** Change a row's action, preparing the message editor where it needs one. */
export function setAction(rows: RebaseRow[], i: number, action: RebaseAction): RebaseRow[] {
  const next = rows.slice()
  const r = { ...next[i], action }
  if (action === 'reword' && !r.message.trim()) r.message = r.commit.message
  if (action !== 'squash' && r.edited) Object.assign(r, { edited: false, editedFor: undefined, message: action === 'reword' ? r.commit.message : '' })
  next[i] = r
  return next
}

export function firstChanged(rows: RebaseRow[], commits: PlanCommit[]): number {
  const i = rows.findIndex((r, k) => r.sha !== commits[k]?.sha || r.action !== 'pick')
  return i < 0 ? rows.length : i
}

/**
 * Pushed commits the rebase would rewrite (or drop): from the first changed position,
 * or earlier, a kept commit a squash/fixup melds into (it is amended in place). Skipped
 * commits (already on the target) leave the branch too.
 */
export function rewrittenPushed(rows: RebaseRow[], commits: PlanCommit[], rewritesAll: boolean, skipped: PlanCommit[] = []): number {
  let from = rewritesAll ? 0 : firstChanged(rows, commits)
  rows.forEach((r, i) => {
    if (r.action === 'squash' || r.action === 'fixup') {
      const h = chainHead(rows, i)
      if (h >= 0) from = Math.min(from, h)
    }
  })
  return commits.slice(from).filter((c) => c.pushed).length + skipped.filter((c) => c.pushed).length
}

/** Why the plan cannot run (empty: it can). */
export function problems(rows: RebaseRow[]): string[] {
  const out: string[] = []
  const first = rows.find((r) => r.action !== 'drop')
  if (first && (first.action === 'squash' || first.action === 'fixup'))
    out.push(`“${first.commit.subject}” cannot be ${first.action === 'squash' ? 'squashed' : 'fixed up'}: there is no commit above it to meld into.`)
  for (const r of rows) if (r.action === 'reword' && !r.message.trim()) out.push(`The new message of “${r.commit.subject}” is empty.`)
  return out
}

/** The plan as the server takes it: a squash chain's message goes with its last squash row only. */
export function toEntries(rows: RebaseRow[]): RebaseEntry[] {
  return rows.map((r, i) => {
    if (r.action === 'reword') return { sha: r.sha, action: r.action, message: r.message }
    if (r.action === 'squash') {
      const m = squashMessage(rows, i)
      return m ? { sha: r.sha, action: r.action, message: m.text } : { sha: r.sha, action: r.action }
    }
    return { sha: r.sha, action: r.action }
  })
}

/** "2 reworded, 1 squashed, 1 dropped, 1 moved". */
export function summary(rows: RebaseRow[], commits: PlanCommit[]): string {
  const count = (a: RebaseAction) => rows.filter((r) => r.action === a).length
  const moved = rows.filter((r, i) => r.sha !== commits[i]?.sha).length
  const parts = [
    [count('reword'), 'reworded'],
    [count('edit'), 'to edit'],
    [count('squash'), 'squashed'],
    [count('fixup'), 'fixed up'],
    [count('drop'), 'dropped'],
    [moved, 'moved'],
  ]
    .filter(([n]) => (n as number) > 0)
    .map(([n, w]) => `${n} ${w}`)
  return parts.length ? parts.join(', ') : 'No changes yet'
}
