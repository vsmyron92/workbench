// Pure helpers of the changelist view of the Commit window (unit-tested in
// p3.test.ts): grouping the status by changelist, the status letter a file shows
// there (HEAD → working tree, like CLion), and which files a commit includes.

import type { Changelist, Changelists, GitStatusFile, StatusCode } from './types'

export interface ListGroup {
  list: Changelist
  files: GitStatusFile[]
}

export interface ChangelistGroups {
  conflicts: GitStatusFile[]
  lists: ListGroup[]
  unversioned: GitStatusFile[]
}

const byPath = (a: GitStatusFile, b: GitStatusFile) => a.path.localeCompare(b.path)

/**
 * Every changed file in exactly one place: conflicts first, unversioned files apart
 * (not in a changelist until added), the rest in their list (the active list when
 * the server has not assigned it yet).
 */
export function groupChangelists(files: GitStatusFile[], cls: Changelists | undefined): ChangelistGroups {
  const lists: Changelist[] = cls?.lists.length
    ? cls.lists
    : [{ id: 'default', name: 'Changes', comment: '', active: true, files: [] }]
  const activeId = cls?.active ?? lists.find((l) => l.active)?.id ?? lists[0].id
  const owner = new Map<string, string>()
  for (const l of lists) for (const p of l.files) owner.set(p, l.id)
  const groups: ListGroup[] = lists.map((list) => ({ list, files: [] }))
  const byId = new Map(groups.map((g) => [g.list.id, g]))
  const out: ChangelistGroups = { conflicts: [], lists: groups, unversioned: [] }
  for (const f of files) {
    if (f.index === '!') continue
    if (f.conflict) out.conflicts.push(f)
    else if (f.index === '?') out.unversioned.push(f)
    else (byId.get(owner.get(f.path) ?? activeId) ?? byId.get(activeId) ?? groups[0]).files.push(f)
  }
  out.conflicts.sort(byPath)
  out.unversioned.sort(byPath)
  for (const g of groups) g.files.sort(byPath)
  return out
}

/** The status letter of a file against HEAD (staged and unstaged together). */
export function headCode(f: GitStatusFile): StatusCode {
  if (f.index === '?') return '?'
  if (f.index === 'D' || f.worktree === 'D') return 'D'
  if (f.index === 'A' || f.worktree === 'A') return 'A'
  if (f.index === 'R' || f.index === 'C') return f.index
  if (f.index === 'T' || f.worktree === 'T') return 'T'
  return 'M'
}

/** What a changelist commit includes by default: the active list's files. */
export function defaultIncluded(g: ChangelistGroups): Set<string> {
  const active = g.lists.find((l) => l.list.active) ?? g.lists[0]
  return new Set(active ? active.files.map((f) => f.path) : [])
}

/** Tri-state of a group of files against the included set. */
export function groupState(paths: string[], included: ReadonlySet<string>, partial: ReadonlySet<string>): 'all' | 'some' | 'none' {
  if (!paths.length) return 'none'
  const n = paths.filter((p) => included.has(p)).length
  if (n === paths.length && !paths.some((p) => partial.has(p))) return 'all'
  return n > 0 ? 'some' : 'none'
}
