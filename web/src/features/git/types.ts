// Types of the git slice's REST API (server/src/git/**, serde camelCase).
// GitStatus, GitFileDiff and GitBlame are cross-slice contracts (docs/ARCHITECTURE.md).

export type StatusCode = ' ' | 'M' | 'A' | 'D' | 'R' | 'C' | 'T' | 'U' | '?' | '!'

export interface GitStatusFile {
  path: string
  origPath?: string
  index: StatusCode
  worktree: StatusCode
  conflict: boolean
  score?: number
  submodule?: boolean
}

export type RepoState = 'clean' | 'merging' | 'rebasing' | 'cherry-picking' | 'reverting' | 'bisecting'

export interface GitStatus {
  branch: string | null
  head: string | null
  upstream: string | null
  ahead: number
  behind: number
  state: RepoState
  stashes: number
  files: GitStatusFile[]
  upstreamGone: boolean
  stateDetail: {
    branch?: string
    onto?: string
    step?: number
    total?: number
    am?: boolean
    /** An interactive rebase. */
    interactive?: boolean
    /** Stopped at an `edit` step: amend (or add commits), then continue. */
    edit?: boolean
    /** The commit the rebase stopped at. */
    stopped?: string
  }
  truncated: boolean
}

export type DiffMode = 'working' | 'staged' | 'commit' | 'compare'

export interface GitHunk {
  header: string
  oldStart: number
  oldLines: number
  newStart: number
  newLines: number
}

export interface GitFileDiff {
  path: string
  oldPath?: string
  original: string
  modified: string
  binary: boolean
  tooLarge: boolean
  hunks: GitHunk[]
  fingerprint: string
  mode: DiffMode
  canStageHunks: boolean
  lfs: boolean
  untracked: boolean
  conflict: boolean
  originalMissing: boolean
  modifiedMissing: boolean
  modeChange?: string
  originalLabel: string
  modifiedLabel: string
  /** The path is a submodule: no file content, `submoduleSummary` describes the change. */
  submodule?: boolean
  submoduleSummary?: string
  /** Working mode: a new submodule commit is checked out (stageable; changes inside it are not). */
  submoduleNewCommit?: boolean
  /** Single lines can be selected (line staging, partial commit). */
  canSelectLines: boolean
  /** Every changed line, when `canSelectLines`. */
  lines?: DiffLine[]
}

/** A changed line of a diff: `line` is on its own side; `at` is the modified-side line it shows at. */
export interface DiffLine {
  hunk: number
  kind: 'add' | 'del'
  line: number
  at: number
}

export interface LineRef {
  kind: 'add' | 'del'
  line: number
}

export interface GitBlame {
  lines: { line: number; sha: string; author: string; time: number; summary: string }[]
  truncated: boolean
}

export type RefKind = 'head' | 'branch' | 'remote' | 'tag' | 'HEAD'
export interface RefLabel {
  name: string
  kind: RefKind
}

export interface LogCommit {
  sha: string
  parents: string[]
  author: string
  email: string
  /** Author time, Unix ms. */
  time: number
  commitTime: number
  subject: string
  refs: RefLabel[]
}

export interface LogPage {
  commits: LogCommit[]
  hasMore: boolean
  /** Single-file history: draw as one chain (parents are not rewritten). */
  linear: boolean
}

export interface LogFilters {
  ref?: string
  all?: boolean
  path?: string
  author?: string
  grep?: string
  /** `start,end`: history of these lines of `path` (git log -L). */
  lines?: string
  /** `lines` are working-tree lines (an editor selection): the server maps them onto the logged revision. */
  worktreeLines?: boolean
}

export interface ChangedFile {
  path: string
  oldPath?: string
  status: 'A' | 'M' | 'D' | 'R' | 'C' | 'T' | 'U' | 'X'
  additions: number
  deletions: number
  binary: boolean
}

export interface CommitDetails {
  sha: string
  parents: string[]
  author: string
  email: string
  time: number
  committer: string
  committerEmail: string
  commitTime: number
  subject: string
  message: string
  refs: RefLabel[]
  files: ChangedFile[]
  filesTruncated: boolean
}

export interface LocalBranch {
  name: string
  sha: string
  upstream: string | null
  ahead: number
  behind: number
  gone: boolean
  subject: string
  time: number
  current: boolean
}

export interface RemoteBranch {
  name: string
  remote: string
  branch: string
  sha: string
  subject: string
  time: number
}

export interface TagInfo {
  name: string
  sha: string
  annotated: boolean
  subject: string
  time: number
}

export interface Branches {
  current: string | null
  head: string | null
  detached: boolean
  local: LocalBranch[]
  remote: RemoteBranch[]
  tags: TagInfo[]
  recent: string[]
  remotes: string[]
  tagsTruncated: boolean
}

export interface StashEntry {
  index: number
  ref: string
  sha: string
  time: number
  message: string
  branch: string | null
}

export interface StashDetails {
  index: number
  sha: string
  base: string
  untrackedSha: string | null
  files: ChangedFile[]
  untracked: ChangedFile[]
}

export interface ConflictVersions {
  path: string
  base: string | null
  ours: string | null
  theirs: string | null
  merged: string
  binary: boolean
  tooLarge: boolean
  oursLabel: string
  theirsLabel: string
  state: RepoState
  /** False once resolved (other fields empty). */
  inConflict: boolean
}

export interface OpOutcome {
  ok: boolean
  conflicts: boolean
  message: string
  state: RepoState
  /** Checkout refused: local changes would be overwritten. */
  dirty?: boolean
}

export interface GitOpEvent {
  opId: string
  op: string
  title?: string
  line?: string
  done?: boolean
  ok?: boolean
  message?: string
  /** Finished with files left in conflict (e.g. an update whose autostash conflicts). */
  conflicts?: boolean
  /** A rebase stopped (edit step, conflicts): continue, skip or abort it. */
  stopped?: boolean
}

export interface PushPreview {
  target: {
    localBranch: string
    remote: string
    remoteBranch: string
    hasUpstream: boolean
    remoteExists: boolean
    remotes: string[]
  }
  commits: LogCommit[]
  hasMore: boolean
}

export interface Comparison {
  base: string
  head: string
  headOnly: LogCommit[]
  baseOnly: LogCommit[]
  files: ChangedFile[]
  mergeBase: string | null
}

/** Params of the 'diff' panel (docs/ARCHITECTURE.md#panels). */
export interface DiffPanelParams {
  projectId: string
  path: string
  mode: DiffMode
  sha?: string
  base?: string
  head?: string
  oldPath?: string
}

// ---------------------------------------------------------------- rebase, bisect, changelists, shelf

export type RebaseAction = 'pick' | 'reword' | 'edit' | 'squash' | 'fixup' | 'drop'

export interface PlanCommit {
  sha: string
  subject: string
  message: string
  author: string
  email: string
  time: number
  /** Already on a remote branch (the upstream or any other remote-tracking branch). */
  pushed: boolean
}

export interface RebasePlan {
  head: string
  branch: string | null
  base: string | null
  root: boolean
  onto: string | null
  /** Oldest first. */
  commits: PlanCommit[]
  /** Onto a branch: commits whose change is already there (cherry-picked); git leaves them out, so they disappear. */
  skipped?: PlanCommit[]
  /** Tracked files with uncommitted changes. */
  dirty: number
  merges: boolean
  pushedRef: string | null
  /** Onto another branch: every commit gets a new id. */
  rewritesAll: boolean
  state: RepoState
}

export interface RebaseEntry {
  sha: string
  action: RebaseAction
  message?: string
}

export interface BisectState {
  active: boolean
  termGood: string
  termBad: string
  bad: string | null
  good: string[]
  skipped: string[]
  current: string | null
  start: string | null
  remaining: number | null
  steps: number | null
  result: string | null
  candidates: string[]
  log: string[]
}

export interface Changelist {
  id: string
  name: string
  comment: string
  active: boolean
  files: string[]
}

export interface Changelists {
  active: string
  lists: Changelist[]
}

export interface ShelfFile {
  path: string
  oldPath?: string
  status: 'A' | 'M' | 'D' | 'R' | 'C' | 'T'
  binary: boolean
  patch: string
  /** The changelist (id) it was in when shelved: unshelving puts it back there by default. */
  changelist?: string
}

export interface ShelfMeta {
  id: string
  name: string
  created: number
  base: string | null
  branch: string | null
  files: ShelfFile[]
  /** A commit holding the shelved changes on top of `base` (for diffs). */
  viewCommit?: string
}

export interface UnshelveResult {
  ok: boolean
  conflicts: boolean
  message: string
  applied: string[]
  conflicted: string[]
  removed: boolean
}
