// Mirrors of server/src/github/model.rs and the operation results (serde
// camelCase). GitHub fields are optional on the wire, so most are nullable here.
//
// `state` is the shared CI vocabulary (success | failed | running | pending |
// canceled | skipped | manual); `status` + `conclusion` are GitHub's own words.

export interface GhUser {
  id: number
  login: string
  name: string | null
  htmlUrl: string
  userType: string
}

export interface Label {
  id: number
  name: string
  description: string | null
}

export interface Milestone {
  number: number
  title: string
}

export interface PrRef {
  label: string
  ref: string
  sha: string
  repo: { id: number; fullName: string; fork: boolean; htmlUrl: string } | null
}

export interface Pull {
  id: number
  nodeId: string
  number: number
  /** open | closed (see `merged`) */
  state: string
  title: string
  body: string | null
  draft: boolean
  locked: boolean
  user: GhUser | null
  assignees: GhUser[]
  requestedReviewers: GhUser[]
  labels: Label[]
  milestone: Milestone | null
  /** Absent on search results. */
  head: PrRef | null
  base: PrRef | null
  htmlUrl: string
  createdAt: string | null
  updatedAt: string | null
  closedAt: string | null
  mergedAt: string | null
  mergeCommitSha: string | null
  authorAssociation: string | null
  merged: boolean
  mergeable: boolean | null
  /** clean | dirty | blocked | behind | unstable | has_hooks | draft | unknown */
  mergeableState: string | null
  rebaseable: boolean | null
  mergedBy: GhUser | null
  comments: number | null
  reviewComments: number | null
  commits: number | null
  additions: number | null
  deletions: number | null
  changedFiles: number | null
  maintainerCanModify: boolean | null
  autoMerge: unknown
}

export interface ReviewState {
  user: GhUser | null
  /** APPROVED | CHANGES_REQUESTED | COMMENTED | DISMISSED */
  state: string
  submittedAt: string | null
}

export interface CheckItem {
  name: string
  /** check | status | run */
  kind: string
  state: string
  status: string
  conclusion: string | null
  url: string | null
  app: string | null
  description: string | null
  runId: number | null
  jobId: number | null
  /** Workflow name and trigger of the run it belongs to (Actions only). */
  workflow: string | null
  event: string | null
  startedAt: string | null
  completedAt: string | null
}

export interface CommitChecks {
  sha: string
  state: string | null
  items: CheckItem[]
  runs: Run[]
}

export interface PullDetail extends Pull {
  checks: CommitChecks | null
  reviewStates: ReviewState[]
  mergeBaseSha: string | null
  warnings: string[]
}

export interface PrFile {
  sha: string | null
  filename: string
  /** added | removed | modified | renamed | copied | changed | unchanged */
  status: string
  additions: number
  deletions: number
  changes: number
  previousFilename: string | null
  patch: string | null
  tooLarge: boolean
}

export interface PrFiles {
  files: PrFile[]
  truncated: boolean
}

export interface FileVersions {
  original: string
  modified: string
  binary: boolean
  tooLarge: boolean
  baseSha: string
  headSha: string
}

export interface Review {
  id: number
  user: GhUser | null
  body: string | null
  state: string
  submittedAt: string | null
  commitId: string | null
  htmlUrl: string
  authorAssociation: string | null
}

export interface ReviewComment {
  id: number
  nodeId: string
  pullRequestReviewId: number | null
  diffHunk: string
  path: string
  commitId: string | null
  originalCommitId: string | null
  inReplyToId: number | null
  user: GhUser | null
  body: string
  createdAt: string | null
  updatedAt: string | null
  htmlUrl: string
  line: number | null
  originalLine: number | null
  startLine: number | null
  originalStartLine: number | null
  /** LEFT (old side) | RIGHT (new side) */
  side: string | null
  startSide: string | null
  subjectType: string | null
  authorAssociation: string | null
}

export interface Thread {
  /** GraphQL id when known (resolvable), else `c<root comment id>`. */
  id: string
  rootId: number
  path: string
  line: number | null
  originalLine: number | null
  startLine: number | null
  side: string | null
  outdated: boolean
  /** null: unknown (no token). */
  resolved: boolean | null
  resolvedBy: string | null
  canResolve: boolean
  comments: ReviewComment[]
}

export interface IssueComment {
  id: number
  user: GhUser | null
  body: string
  createdAt: string | null
  updatedAt: string | null
  htmlUrl: string
  authorAssociation: string | null
}

export interface Commit {
  sha: string
  shortSha: string
  title: string
  message: string
  authorName: string
  authorLogin: string | null
  date: string | null
  htmlUrl: string
}

export interface Issue {
  id: number
  number: number
  title: string
  body: string | null
  /** open | closed */
  state: string
  stateReason: string | null
  labels: Label[]
  assignees: GhUser[]
  user: GhUser | null
  milestone: Milestone | null
  comments: number
  createdAt: string | null
  updatedAt: string | null
  closedAt: string | null
  htmlUrl: string
  locked: boolean
  authorAssociation: string | null
}

export interface IssueDetail {
  issue: Issue
  comments: IssueComment[]
}

export interface Asset {
  id: number
  name: string
  size: number
  downloadCount: number
  browserDownloadUrl: string
  contentType: string | null
}

export interface Release {
  id: number
  tagName: string
  name: string | null
  body: string | null
  draft: boolean
  prerelease: boolean
  createdAt: string | null
  publishedAt: string | null
  htmlUrl: string
  author: GhUser | null
  targetCommitish: string | null
  assets: Asset[]
}

export interface Workflow {
  id: number
  name: string
  path: string
  state: string
  htmlUrl: string
  createdAt: string | null
  updatedAt: string | null
}

export interface Run {
  id: number
  name: string | null
  displayTitle: string | null
  runNumber: number
  runAttempt: number | null
  event: string
  status: string
  conclusion: string | null
  workflowId: number
  headBranch: string | null
  headSha: string
  path: string | null
  htmlUrl: string
  createdAt: string | null
  updatedAt: string | null
  runStartedAt: string | null
  actor: GhUser | null
  triggeringActor: GhUser | null
  headCommit: { id: string; message: string; timestamp: string | null } | null
  pullRequests: { number: number }[]
  state: string
  duration: number | null
  commitTitle: string | null
}

export interface Step {
  name: string
  status: string
  conclusion: string | null
  number: number
  startedAt: string | null
  completedAt: string | null
  state: string
}

export interface Job {
  id: number
  runId: number
  runAttempt: number | null
  name: string
  workflowName: string | null
  status: string
  conclusion: string | null
  createdAt: string | null
  startedAt: string | null
  completedAt: string | null
  htmlUrl: string
  headSha: string
  headBranch: string | null
  labels: string[]
  runnerName: string | null
  steps: Step[]
  state: string
  duration: number | null
}

export interface RunDetail {
  run: Run
  jobs: Job[]
  truncated: boolean
}

export interface StepMark {
  number: number
  line: number
}

export interface JobLog {
  /** Present when `available`. */
  text?: string
  steps?: StepMark[]
  truncated?: boolean
  size?: number
  available: boolean
  /** running | needs_token | gone */
  reason: string | null
  message: string | null
  job: Job
}

export interface LogTail {
  text: string
  totalLines: number
  truncated: boolean
  available: boolean
  message: string | null
  state: string
  complete: boolean
}

/** An artifact a run uploaded. */
export interface Artifact {
  id: number
  name: string
  sizeInBytes: number
  /** Past its retention: gone from GitHub. */
  expired: boolean
  createdAt: string | null
  expiresAt: string | null
  digest: string | null
}

export interface Annotation {
  path: string
  startLine: number | null
  endLine: number | null
  /** notice | warning | failure */
  annotationLevel: string
  title: string | null
  message: string
}

export interface DispatchInput {
  name: string
  description: string | null
  required: boolean
  default: string | null
  /** string | choice | boolean | number | environment */
  type: string
  options: string[]
}

export interface DispatchInfo {
  dispatchable: boolean
  inputs: DispatchInput[]
}

export interface RateInfo {
  limit: number | null
  remaining: number | null
  /** Epoch seconds. */
  reset: number | null
  used: number | null
}

/** The shared CI status (forge contract). */
export interface CiStatus {
  status: string
  pipelineId: number | null
  webUrl: string | null
  sha: string | null
  ref: string | null
}

export interface GithubSummary {
  host: string
  path: string
  webUrl: string
  description: string | null
  private: boolean
  archived: boolean
  defaultBranch: string | null
  branch: string | null
  head: string | null
  headStatus: CiStatus | null
  branchRuns: Run[]
  branchRun: Run | null
  defaultRun: Run | null
  currentPr: Pull | null
  openPrCount: number | null
  openIssueCount: number | null
  hasIssues: boolean
  canPush: boolean | null
  mergeMethods: { merge: boolean; squash: boolean; rebase: boolean } | null
  deleteBranchOnMerge: boolean | null
  auth: { authenticated: boolean; reason: string | null; viewer: string | null; rate: RateInfo | null }
  warnings: string[]
  fetchedAt: number
}

export interface BranchInfo {
  name: string
  exists: boolean
  protected: boolean
  sha: string | null
  title: string | null
}

export interface ListPage<T> {
  items: T[]
  page: number
  nextPage: number | null
  total: number | null
}

/** `github.run` event data. */
export interface RunEvent {
  runId: number | null
  workflowId: number | null
  name?: string | null
  state?: string
  status?: string
  conclusion?: string | null
  branch?: string | null
  sha?: string
  event?: string
  runNumber?: number
  webUrl?: string
  action: string
  previousState?: string | null
}
