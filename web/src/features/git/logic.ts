// Pure helpers of the git UI (unit-tested in logic.test.ts).

import type { ChangedFile, DiffMode, GitHunk, GitStatusFile, RefKind, StatusCode } from './types'

// ---------------------------------------------------------------- panel ids (docs/ARCHITECTURE.md#panels)

export function diffPanelId(pid: string, mode: DiffMode, path: string, o: { sha?: string; base?: string; head?: string } = {}) {
  const rev = mode === 'commit' ? (o.sha ?? '') : mode === 'compare' ? `${o.base ?? ''}..${o.head ?? ''}` : ''
  return `diff:${pid}:${mode}:${rev}:${path}`
}
export const commitPanelId = (pid: string, sha: string) => `commit:${pid}:${sha}`
export const gitlogPanelId = (pid: string) => `gitlog:${pid}`
export const conflictPanelId = (pid: string, path: string) => `conflict:${pid}:${path}`

// ---------------------------------------------------------------- status

export type SectionId = 'conflicts' | 'staged' | 'unstaged' | 'untracked'

export interface StatusSections {
  conflicts: GitStatusFile[]
  staged: GitStatusFile[]
  unstaged: GitStatusFile[]
  untracked: GitStatusFile[]
}

const byPath = (a: GitStatusFile, b: GitStatusFile) => a.path.localeCompare(b.path)

/** Split status entries into CLion's change lists. A file can be staged *and* unstaged. */
export function groupStatus(files: GitStatusFile[]): StatusSections {
  const s: StatusSections = { conflicts: [], staged: [], unstaged: [], untracked: [] }
  for (const f of files) {
    if (f.conflict) s.conflicts.push(f)
    else if (f.index === '?') s.untracked.push(f)
    else if (f.index === '!') continue
    else {
      if (f.index !== ' ') s.staged.push(f)
      if (f.worktree !== ' ') s.unstaged.push(f)
    }
  }
  s.conflicts.sort(byPath)
  s.staged.sort(byPath)
  s.unstaged.sort(byPath)
  s.untracked.sort(byPath)
  return s
}

/** The status letter that describes a file in a section. */
export function sectionCode(f: GitStatusFile, section: SectionId): StatusCode {
  if (section === 'conflicts') return 'U'
  if (section === 'untracked') return '?'
  return section === 'staged' ? f.index : f.worktree
}

/** CSS class carrying the CLion VCS colour of a status letter (git.css). */
export function statusClass(code: string): string {
  switch (code) {
    case 'M':
    case 'T':
      return 'git-c-modified'
    case 'A':
      return 'git-c-added'
    case 'D':
      return 'git-c-deleted'
    case 'R':
    case 'C':
      return 'git-c-renamed'
    case '?':
      return 'git-c-untracked'
    case 'U':
      return 'git-c-conflict'
    case '!':
      return 'git-c-ignored'
    default:
      return ''
  }
}

export function statusLabel(code: string): string {
  return (
    {
      M: 'Modified',
      T: 'Type changed',
      A: 'Added',
      D: 'Deleted',
      R: 'Renamed',
      C: 'Copied',
      '?': 'Unversioned',
      U: 'Conflict',
      '!': 'Ignored',
    } as Record<string, string>
  )[code] ?? 'Changed'
}

export function splitPath(path: string): { name: string; dir: string } {
  const i = path.lastIndexOf('/')
  return i < 0 ? { name: path, dir: '' } : { name: path.slice(i + 1), dir: path.slice(0, i) }
}

export const shortSha = (sha: string | null | undefined) => (sha ?? '').slice(0, 8)

export function stateLabel(state: string): string {
  return (
    {
      merging: 'Merging',
      rebasing: 'Rebasing',
      'cherry-picking': 'Cherry-picking',
      reverting: 'Reverting',
      bisecting: 'Bisecting',
    } as Record<string, string>
  )[state] ?? ''
}

export function refTone(kind: RefKind): 'accent' | 'success' | 'warning' | undefined {
  if (kind === 'head' || kind === 'HEAD') return 'accent'
  if (kind === 'branch') return 'success'
  if (kind === 'tag') return 'warning'
  return undefined
}

// ---------------------------------------------------------------- hunks

/** Index of the hunk at (or the last one before) a 1-based line of the modified side. */
export function hunkAtLine(hunks: GitHunk[], line: number): number {
  if (!hunks.length) return -1
  let best = 0
  for (let i = 0; i < hunks.length; i++) {
    const start = Math.max(1, hunks[i].newStart)
    if (start <= line) best = i
    else break
  }
  return best
}

/** Line range of a hunk on the modified side (a pure deletion is a 1-line range). */
export function hunkRange(h: GitHunk): { start: number; end: number } {
  const start = Math.max(1, h.newLines === 0 ? h.newStart : h.newStart)
  return { start, end: Math.max(start, h.newStart + h.newLines - 1) }
}

// ---------------------------------------------------------------- file tree (commit details)

export interface TreeDir {
  kind: 'dir'
  name: string
  path: string
  children: TreeNode[]
  count: number
}
export interface TreeFile {
  kind: 'file'
  name: string
  file: ChangedFile
}
export type TreeNode = TreeDir | TreeFile

/** Directory tree of changed files; single-child directory chains are merged (`src/git`). */
export function buildFileTree(files: ChangedFile[]): TreeNode[] {
  const root: TreeDir = { kind: 'dir', name: '', path: '', children: [], count: 0 }
  for (const f of files) {
    const parts = f.path.split('/')
    let dir = root
    for (let i = 0; i < parts.length - 1; i++) {
      const path = parts.slice(0, i + 1).join('/')
      let next = dir.children.find((c): c is TreeDir => c.kind === 'dir' && c.path === path)
      if (!next) {
        next = { kind: 'dir', name: parts[i], path, children: [], count: 0 }
        dir.children.push(next)
      }
      dir = next
    }
    dir.children.push({ kind: 'file', name: parts[parts.length - 1], file: f })
  }
  const finish = (d: TreeDir): TreeDir => {
    d.children = d.children.map((c) => (c.kind === 'dir' ? finish(c) : c))
    // Merge a directory whose only child is a directory.
    while (d.path && d.children.length === 1 && d.children[0].kind === 'dir') {
      const only = d.children[0] as TreeDir
      d = { ...only, name: `${d.name}/${only.name}` }
    }
    d.children.sort((a, b) => (a.kind !== b.kind ? (a.kind === 'dir' ? -1 : 1) : a.name.localeCompare(b.name)))
    d.count = d.children.reduce((n, c) => n + (c.kind === 'dir' ? c.count : 1), 0)
    return d
  }
  return finish(root).children
}

// ---------------------------------------------------------------- conflict markers

export interface ConflictBlock {
  /** 0-based line of `<<<<<<<`. */
  start: number
  /** 0-based line of `>>>>>>>`. */
  end: number
  ours: string[]
  base: string[] | null
  theirs: string[]
}

/** Split text into lines, keeping each line's terminator. */
export function splitLinesKeep(text: string): string[] {
  const out: string[] = []
  let i = 0
  while (i < text.length) {
    const j = text.indexOf('\n', i)
    if (j < 0) {
      out.push(text.slice(i))
      break
    }
    out.push(text.slice(i, j + 1))
    i = j + 1
  }
  return out
}

const isMarker = (line: string, ch: string) => line.startsWith(ch.repeat(7)) && (line.length === 7 || /^[ \r\n]/.test(line.slice(7)))

/** Find `<<<<<<< … ||||||| … ======= … >>>>>>>` blocks (merge and diff3 styles). */
export function parseConflictBlocks(text: string): ConflictBlock[] {
  const lines = splitLinesKeep(text)
  const blocks: ConflictBlock[] = []
  for (let i = 0; i < lines.length; i++) {
    if (!isMarker(lines[i], '<')) continue
    const b: ConflictBlock = { start: i, end: -1, ours: [], base: null, theirs: [] }
    let part: 'ours' | 'base' | 'theirs' = 'ours'
    let j = i + 1
    for (; j < lines.length; j++) {
      const l = lines[j]
      if (part === 'ours' && isMarker(l, '|')) {
        part = 'base'
        b.base = []
      } else if (part !== 'theirs' && isMarker(l, '=')) {
        part = 'theirs'
      } else if (part === 'theirs' && isMarker(l, '>')) {
        b.end = j
        break
      } else if (isMarker(l, '<')) {
        break // malformed: a new block starts before this one ended
      } else if (part === 'ours') b.ours.push(l)
      else if (part === 'base') b.base!.push(l)
      else b.theirs.push(l)
    }
    if (b.end >= 0) {
      blocks.push(b)
      i = b.end
    } else {
      i = j - 1
    }
  }
  return blocks
}

export type BlockChoice = 'ours' | 'theirs' | 'both' | 'none'

/** Replace one conflict block with the chosen side(s). */
export function resolveConflictBlock(text: string, index: number, choice: BlockChoice): string {
  const blocks = parseConflictBlocks(text)
  const b = blocks[index]
  if (!b) return text
  const lines = splitLinesKeep(text)
  const pick = choice === 'ours' ? b.ours : choice === 'theirs' ? b.theirs : choice === 'both' ? [...b.ours, ...b.theirs] : []
  // Keep the file's line endings when joining sides that lost their last newline.
  const fixed = pick.map((l, i) => (i < pick.length - 1 && !l.endsWith('\n') ? l + '\n' : l))
  return [...lines.slice(0, b.start), ...fixed, ...lines.slice(b.end + 1)].join('')
}

// ---------------------------------------------------------------- misc

/** Case-insensitive subsequence-free substring match used by the branches popover. */
export function matches(name: string, query: string): boolean {
  const q = query.trim().toLowerCase()
  return !q || name.toLowerCase().includes(q)
}

/** Commit message subject (first line) and body. */
export function splitMessage(message: string): { subject: string; body: string } {
  const i = message.indexOf('\n')
  if (i < 0) return { subject: message, body: '' }
  return { subject: message.slice(0, i), body: message.slice(i + 1).replace(/^\n+/, '') }
}

// ---------------------------------------------------------------- bisect

export type BisectMark = 'bad' | 'good' | 'skip' | 'result' | 'current'

/** How a commit shows in the log while bisecting (the result wins over everything). */
export function bisectMark(
  s: { active: boolean; bad: string | null; good: string[]; skipped: string[]; current: string | null; result: string | null } | undefined,
  sha: string,
): BisectMark | null {
  if (!s?.active) return null
  if (s.result === sha) return 'result'
  if (s.bad === sha) return 'bad'
  if (s.good.includes(sha)) return 'good'
  if (s.skipped.includes(sha)) return 'skip'
  if (s.current === sha && !s.result) return 'current'
  return null
}

/** "3 revisions left (about 2 steps)". */
export function bisectProgress(remaining: number | null, steps: number | null): string {
  if (remaining === null) return 'Mark a good and a bad commit'
  const r = `${remaining} revision${remaining === 1 ? '' : 's'} left`
  return steps !== null ? `${r} (about ${steps} step${steps === 1 ? '' : 's'})` : r
}
