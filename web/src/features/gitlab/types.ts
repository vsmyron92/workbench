// Mirrors of server/src/gitlab/model.rs (serde camelCase). GitLab fields are
// optional on the wire, so most are nullable here.

export interface GlUser {
  id: number
  username: string
  name: string
}

export interface DetailedStatus {
  text: string
  label: string
  group: string
  icon: string
}

export interface Pipeline {
  id: number
  iid: number | null
  projectId: number | null
  status: string
  ref: string
  sha: string
  beforeSha: string | null
  tag: boolean
  source: string | null
  name: string | null
  createdAt: string | null
  updatedAt: string | null
  startedAt: string | null
  finishedAt: string | null
  duration: number | null
  queuedDuration: number | null
  coverage: unknown
  webUrl: string
  user: GlUser | null
  detailedStatus: DetailedStatus | null
  yamlErrors: string | null
  commitTitle: string | null
}

export interface PipelineRef {
  id: number
  iid: number | null
  projectId: number | null
  status: string
  ref: string
  sha: string
  webUrl: string
}

export interface Job {
  id: number
  name: string
  stage: string
  status: string
  ref: string
  tag: boolean
  createdAt: string | null
  startedAt: string | null
  finishedAt: string | null
  erasedAt: string | null
  duration: number | null
  queuedDuration: number | null
  allowFailure: boolean
  failureReason: string | null
  webUrl: string
  user: GlUser | null
  runner: { id: number; description: string; name: string | null; isShared: boolean | null } | null
  artifacts: { fileType: string; size: number | null; filename: string; fileFormat: string | null }[]
  artifactsFile: { filename: string; size: number | null } | null
  artifactsExpireAt: string | null
  pipeline: PipelineRef | null
  commit: { id: string; shortId: string; title: string; authorName: string; createdAt: string | null } | null
  coverage: unknown
  tagList: string[]
  archived: boolean
  /** 'job' | 'bridge' (trigger job) */
  kind: string
  downstreamPipeline: PipelineRef | null
}

export interface Stage {
  name: string
  status: string
  jobs: Job[]
}

export interface TestSummary {
  total: { time: number; count: number; success: number; failed: number; skipped: number; error: number; suiteError: string | null }
  testSuites: {
    name: string
    totalTime: number
    totalCount: number
    successCount: number
    failedCount: number
    skippedCount: number
    errorCount: number
    suiteError: string | null
  }[]
}

export interface PipelineDetail {
  pipeline: Pipeline
  stages: Stage[]
  testSummary: TestSummary | null
}

/** GET …/pipelines/{id}/tests: the failed and errored cases of the test report. */
export interface TestFailures {
  total: TestSummary['total']
  suites: SuiteFailures[]
  /** More failed cases exist than are listed. */
  truncated: boolean
}

export interface SuiteFailures {
  name: string
  totalCount: number
  failedCount: number
  errorCount: number
  skippedCount: number
  time: number
  suiteError: string | null
  cases: FailedCase[]
}

export interface FailedCase {
  status: 'failed' | 'error'
  name: string
  classname: string
  /** The test's file as the report names it. */
  file: string | null
  time: number
  /** The failure message, then the stack trace. */
  output: string
  outputTruncated: boolean
  recentFailures: number | null
  baseBranch: string | null
}

export interface ListPage<T> {
  items: T[]
  page: number
  nextPage: number | null
  total: number | null
}

export interface TraceChunk {
  text: string
  offset: number
  complete: boolean
  status: string
  reset: boolean
  truncated: boolean
  size: number | null
}

export interface TraceTail {
  text: string
  totalLines: number
  truncated: boolean
  status: string
  complete: boolean
}

export interface DiffRefs {
  baseSha: string
  headSha: string
  startSha: string
}

export interface Approvals {
  approved: boolean
  approvalsRequired: number
  approvalsLeft: number
  approvedBy: { user: GlUser }[]
  userCanApprove: boolean
  userHasApproved: boolean
}

export interface Mr {
  id: number
  iid: number
  projectId: number | null
  title: string
  description: string | null
  state: string
  draft: boolean
  sourceBranch: string
  targetBranch: string
  author: GlUser | null
  assignees: GlUser[]
  reviewers: GlUser[]
  labels: string[]
  createdAt: string | null
  updatedAt: string | null
  mergedAt: string | null
  closedAt: string | null
  mergeUser: GlUser | null
  webUrl: string
  sha: string | null
  mergeCommitSha: string | null
  squashCommitSha: string | null
  detailedMergeStatus: string | null
  hasConflicts: boolean
  userNotesCount: number
  upvotes: number
  downvotes: number
  references: { short: string; full: string } | null
  squash: boolean
  squashOnMerge: boolean | null
  forceRemoveSourceBranch: boolean | null
  shouldRemoveSourceBranch: boolean | null
  mergeWhenPipelineSucceeds: boolean
  blockingDiscussionsResolved: boolean | null
  discussionLocked: boolean | null
  diffRefs: DiffRefs | null
  changesCount: string | null
  headPipeline: Pipeline | null
  mergeError: string | null
  divergedCommitsCount: number | null
  rebaseInProgress: boolean | null
  user: { canMerge: boolean } | null
  approvals: Approvals | null
}

export interface MrDiffFile {
  oldPath: string
  newPath: string
  aMode: string | null
  bMode: string | null
  newFile: boolean
  renamedFile: boolean
  deletedFile: boolean
  generatedFile: boolean | null
  tooLarge: boolean | null
  collapsed: boolean | null
  diff: string
  additions: number
  deletions: number
}

export interface MrDiffs {
  files: MrDiffFile[]
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

export interface Position {
  baseSha: string | null
  startSha: string | null
  headSha: string | null
  oldPath: string | null
  newPath: string | null
  positionType: string | null
  oldLine: number | null
  newLine: number | null
}

export interface Note {
  id: number
  type: string | null
  body: string
  author: GlUser | null
  createdAt: string | null
  updatedAt: string | null
  system: boolean
  resolvable: boolean
  resolved: boolean
  resolvedBy: GlUser | null
  position: Position | null
}

export interface Discussion {
  id: string
  individualNote: boolean
  notes: Note[]
}

export interface Commit {
  id: string
  shortId: string
  title: string
  message: string
  authorName: string
  authorEmail: string
  authoredDate: string | null
  committedDate: string | null
  webUrl: string
  parentIds: string[]
}

export interface Issue {
  id: number
  iid: number
  title: string
  description: string | null
  state: string
  labels: string[]
  assignees: GlUser[]
  author: GlUser | null
  milestone: { id: number; title: string } | null
  dueDate: string | null
  createdAt: string | null
  updatedAt: string | null
  closedAt: string | null
  webUrl: string
  userNotesCount: number
  references: { short: string; full: string } | null
  confidential: boolean
  issueType: string | null
}

export interface IssueDetail {
  issue: Issue
  notes: Note[]
}

export interface Deployment {
  id: number
  iid: number | null
  ref: string
  sha: string
  status: string
  createdAt: string | null
  updatedAt: string | null
  finishedAt: string | null
  user: GlUser | null
  deployable: { id: number; name: string; status: string; stage: string; pipeline: PipelineRef | null } | null
  environment: { id: number; name: string; tier: string | null } | null
}

export interface Environment {
  id: number
  name: string
  slug: string
  state: string
  tier: string | null
  externalUrl: string | null
  createdAt: string | null
  updatedAt: string | null
  autoStopAt: string | null
  description: string | null
  lastDeployment: Deployment | null
}

export interface RegistryRepo {
  id: number
  name: string
  path: string
  location: string
  createdAt: string | null
  tagsCount: number | null
  status: string | null
}

export interface RegistryTag {
  name: string
  path: string | null
  location: string
  digest: string | null
  createdAt: string | null
  publishedAt: string | null
  totalSize: number | null
  revision: string | null
}

export interface TagsPage {
  items: RegistryTag[]
  nextCursor: string | null
  /** 'published' (newest first) | 'name' (alphabetical fallback) */
  order: string
  total: number | null
}

export interface CiStatus {
  status: string
  pipelineId: number | null
  webUrl: string | null
  sha: string | null
  ref: string | null
}

export interface GitlabSummary {
  host: string
  path: string
  gitlabProjectId: number
  webUrl: string
  defaultBranch: string | null
  branch: string | null
  head: string | null
  headStatus: CiStatus | null
  branchPipeline: Pipeline | null
  defaultPipeline: Pipeline | null
  currentMr: Mr | null
  openMrCount: number | null
  openIssueCount: number | null
  environmentCount: number | null
  registryEnabled: boolean
  issuesEnabled: boolean
  mergeRequestsEnabled: boolean
  mergeMethod: string | null
  squashOption: string | null
  removeSourceBranchAfterMerge: boolean | null
  onlyAllowMergeIfPipelineSucceeds: boolean | null
  accessLevel: number | null
  warnings: string[]
  fetchedAt: number
}

export interface BranchInfo {
  name: string
  exists: boolean
  merged: boolean
  protected: boolean
  default: boolean
  canPush: boolean
  webUrl: string | null
  commit: { id: string; shortId: string; title: string } | null
}

/** `gitlab.pipeline` event data. */
export interface PipelineEvent {
  pipelineId: number
  iid: number | null
  status: string
  ref: string
  sha: string
  webUrl: string
  previousStatus: string | null
}
