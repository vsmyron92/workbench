// Maps the git slice's GitStatus onto CLion's file colours (tokens --vcs-*).

import type { GitStatus, GitStatusFile } from './api'
import { ancestors } from './paths'

export type VcsKind = 'modified' | 'added' | 'deleted' | 'renamed' | 'untracked' | 'conflict' | 'ignored'

export function fileVcsKind(f: GitStatusFile): VcsKind | null {
  const { index: x, worktree: y } = f
  if (f.conflict || x === 'U' || y === 'U') return 'conflict'
  if (x === '?' || y === '?') return 'untracked'
  if (x === '!' || y === '!') return 'ignored'
  if (x === 'D' || y === 'D') return 'deleted'
  if (x === 'A') return 'added'
  if (x === 'R' || x === 'C') return 'renamed'
  if (x !== ' ' || y !== ' ') return 'modified'
  return null
}

export interface VcsIndex {
  files: Map<string, VcsKind>
  /** Folders containing changes (CLion colours them like a modification). */
  dirs: Map<string, 'modified' | 'conflict'>
}

export const EMPTY_VCS: VcsIndex = { files: new Map(), dirs: new Map() }

export function buildVcsIndex(status: GitStatus | undefined | null): VcsIndex {
  if (!status?.files?.length) return EMPTY_VCS
  const files = new Map<string, VcsKind>()
  const dirs = new Map<string, 'modified' | 'conflict'>()
  for (const f of status.files) {
    const kind = fileVcsKind(f)
    if (!kind) continue
    const path = f.path.replace(/\/$/, '')
    files.set(path, kind)
    if (kind === 'ignored') continue
    for (const d of ancestors(path)) {
      if (d === '') continue
      if (kind === 'conflict') dirs.set(d, 'conflict')
      else if (!dirs.has(d)) dirs.set(d, 'modified')
    }
    // Untracked folders are reported as `dir/`: everything below is untracked too.
  }
  return { files, dirs }
}

/** Status of a path, including "inside an untracked/ignored folder". */
export function vcsKindOf(index: VcsIndex, path: string): VcsKind | null {
  const direct = index.files.get(path)
  if (direct) return direct
  for (const a of ancestors(path).slice(1).reverse()) {
    const k = index.files.get(a)
    if (k === 'untracked' || k === 'ignored') return k
  }
  return null
}

export const VCS_LABEL: Record<VcsKind, string> = {
  modified: 'Modified',
  added: 'Added',
  deleted: 'Deleted',
  renamed: 'Renamed',
  untracked: 'Unversioned',
  conflict: 'Merge conflict',
  ignored: 'Ignored',
}
