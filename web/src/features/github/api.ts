// REST client, query keys and hooks for /api/projects/{pid}/github/**. The "pid" every
// function takes is a repository scope id (`api/repos.ts`): the project id for the default
// repository, else `<project>::<repo>`. Every key starts with ['github', scope] so an event
// for a project can invalidate its views (see providers.tsx).
//
// Without a token GitHub allows 60 requests an hour: the server caches hard,
// and these hooks do not poll detail views at all when the summary says the
// connection is anonymous (see "polling" in logic.ts).

import { useInfiniteQuery, useQuery } from '@tanstack/react-query'
import { api, ApiError } from '@/api/client'
import { useProjects } from '@/api/queries'
import { repoForge, scopeProject, withRepo } from '@/api/repos'
import { isActive, livePoll, summaryPoll } from './logic'
import type {
  Annotation,
  Artifact,
  BranchInfo,
  Commit,
  CommitChecks,
  DispatchInfo,
  FileVersions,
  GithubSummary,
  Issue,
  IssueComment,
  IssueDetail,
  Job,
  JobLog,
  ListPage,
  LogTail,
  PrFile,
  PrFiles,
  Pull,
  PullDetail,
  Release,
  Review,
  ReviewComment,
  Run,
  RunDetail,
  Thread,
  Workflow,
} from './types'

/** `/api/projects/{pid}/github/{path}` for the repository a scope id names (`?repo=`; `api/repos.ts`). */
export const ghUrl = (scope: string, path: string) => withRepo(`/api/projects/${encodeURIComponent(scopeProject(scope))}/github/${path}`, scope)

export const ghk = {
  all: (pid: string) => ['github', pid] as const,
  summary: (pid: string) => ['github', pid, 'summary'] as const,
  runs: (pid: string, f?: object) => (f ? (['github', pid, 'runs', f] as const) : (['github', pid, 'runs'] as const)),
  run: (pid: string, id: number) => ['github', pid, 'run', id] as const,
  artifacts: (pid: string, id: number) => ['github', pid, 'run', id, 'artifacts'] as const,
  job: (pid: string, id: number) => ['github', pid, 'job', id] as const,
  jobLog: (pid: string, id: number) => ['github', pid, 'job', id, 'log'] as const,
  annotations: (pid: string, id: number) => ['github', pid, 'job', id, 'annotations'] as const,
  workflows: (pid: string) => ['github', pid, 'workflows'] as const,
  inputs: (pid: string, id: number, ref: string) => ['github', pid, 'inputs', id, ref] as const,
  pulls: (pid: string, f?: object) => (f ? (['github', pid, 'pulls', f] as const) : (['github', pid, 'pulls'] as const)),
  pull: (pid: string, n: number) => ['github', pid, 'pull', n] as const,
  pullPart: (pid: string, n: number, part: string) => ['github', pid, 'pull', n, part] as const,
  prFile: (pid: string, n: number, path: string, base: string, head: string) => ['github', pid, 'prfile', n, path, base, head] as const,
  issues: (pid: string, f?: object) => (f ? (['github', pid, 'issues', f] as const) : (['github', pid, 'issues'] as const)),
  issue: (pid: string, n: number) => ['github', pid, 'issue', n] as const,
  releases: (pid: string) => ['github', pid, 'releases'] as const,
  branch: (pid: string, name: string) => ['github', pid, 'branch', name] as const,
}

/** Don't retry what retrying cannot fix (setup, permissions, limits, missing things). */
function retry(count: number, e: unknown) {
  if (e instanceof ApiError && [400, 401, 403, 404, 409, 412, 429].includes(e.status)) return false
  return count < 2
}

/** The project list says whether the repository a scope names is on GitHub. */
export function useHasGithub(pid: string | null): boolean {
  const { data } = useProjects()
  return !!pid && !!repoForge(data?.find((p) => p.id === scopeProject(pid)), pid, 'github')
}

/** GitHub is this repository's forge for CI (GitLab wins when it has both). */
export function useGithubIsForge(pid: string | null): boolean {
  const { data } = useProjects()
  const p = pid ? data?.find((x) => x.id === scopeProject(pid)) : undefined
  return !!pid && !!repoForge(p, pid, 'github') && !repoForge(p, pid, 'gitlab')
}

export function useGithubSummary(pid: string | null, enabled = true) {
  return useQuery({
    queryKey: ghk.summary(pid ?? ''),
    queryFn: () => api.get<GithubSummary>(ghUrl(pid!, 'summary')),
    enabled: !!pid && enabled,
    retry,
    staleTime: 15_000,
    // Events keep it fresh; this is the fallback (none for a missing or
    // private repository without a token: a timer cannot fix that).
    refetchInterval: (q) => summaryPoll(q.state.data, q.state.error),
  })
}

export interface RunFilters {
  branch?: string
  status?: string
  event?: string
  workflowId?: number
}

export function useRuns(pid: string, filters: RunFilters, anonymous = false, perPage = 25) {
  return useInfiniteQuery({
    queryKey: ghk.runs(pid, { ...filters, perPage }),
    queryFn: ({ pageParam }) => api.get<ListPage<Run>>(ghUrl(pid, 'actions/runs'), { ...filters, page: pageParam, perPage }),
    initialPageParam: 1,
    getNextPageParam: (last) => last.nextPage ?? undefined,
    retry,
    staleTime: 20_000,
    refetchInterval: (q) => livePoll(anonymous, !!q.state.data?.pages[0]?.items.some((r) => isActive(r.state)), 10_000),
  })
}

/** A run's artifacts; `updatedAt` refetches them as the run moves on. */
export function useRunArtifacts(pid: string, id: number, updatedAt: string | null, enabled = true) {
  return useQuery({
    queryKey: [...ghk.artifacts(pid, id), updatedAt ?? ''],
    queryFn: ({ signal }) => api.get<Artifact[]>(ghUrl(pid, `actions/runs/${id}/artifacts`), undefined, signal),
    enabled: enabled && id > 0,
    retry,
    staleTime: 60_000,
  })
}

export function useRun(pid: string, id: number, anonymous = false, enabled = true) {
  return useQuery({
    queryKey: ghk.run(pid, id),
    queryFn: () => api.get<RunDetail>(ghUrl(pid, `actions/runs/${id}`)),
    enabled: enabled && id > 0,
    retry,
    refetchInterval: (q) => {
      const d = q.state.data
      return livePoll(anonymous, !!d && (isActive(d.run.state) || d.jobs.some((j) => isActive(j.state))), 5_000)
    },
  })
}

export function useJob(pid: string, id: number, anonymous = false) {
  return useQuery({
    queryKey: ghk.job(pid, id),
    queryFn: () => api.get<Job>(ghUrl(pid, `actions/jobs/${id}`)),
    retry,
    refetchInterval: (q) => livePoll(anonymous, isActive(q.state.data?.state), 5_000),
  })
}

/** The job's log; re-asked every 10 s until GitHub publishes it. */
export function useJobLog(pid: string, id: number, enabled = true) {
  return useQuery({
    queryKey: ghk.jobLog(pid, id),
    queryFn: () => api.get<JobLog>(ghUrl(pid, `actions/jobs/${id}/logs`)),
    enabled,
    retry,
    staleTime: Infinity,
    refetchInterval: (q) => (q.state.data?.reason === 'running' ? 10_000 : false),
  })
}

export function useAnnotations(pid: string, id: number, enabled = true) {
  return useQuery({
    queryKey: ghk.annotations(pid, id),
    queryFn: () => api.get<Annotation[]>(ghUrl(pid, `actions/jobs/${id}/annotations`)),
    enabled,
    retry,
    staleTime: 60_000,
  })
}

export function fetchLogTail(pid: string, jobId: number, lines: number) {
  return api.get<LogTail>(ghUrl(pid, `actions/jobs/${jobId}/logs`), { tail: lines, plain: true })
}

export function fetchAnnotations(pid: string, jobId: number) {
  return api.get<Annotation[]>(ghUrl(pid, `actions/jobs/${jobId}/annotations`))
}

export function useWorkflows(pid: string, enabled = true) {
  return useQuery({
    queryKey: ghk.workflows(pid),
    queryFn: () => api.get<Workflow[]>(ghUrl(pid, 'actions/workflows')),
    enabled,
    retry,
    staleTime: 5 * 60_000,
  })
}

export function useDispatchInfo(pid: string, id: number | null, ref: string) {
  return useQuery({
    queryKey: ghk.inputs(pid, id ?? 0, ref),
    queryFn: () => api.get<DispatchInfo>(ghUrl(pid, `actions/workflows/${id}/inputs`), { ref: ref || undefined }),
    enabled: id !== null,
    retry,
    staleTime: 5 * 60_000,
  })
}

export interface PullFilters {
  state?: string
  search?: string
}

export function usePulls(pid: string, filters: PullFilters) {
  return useInfiniteQuery({
    queryKey: ghk.pulls(pid, filters),
    queryFn: ({ pageParam }) => api.get<ListPage<Pull>>(ghUrl(pid, 'pulls'), { ...filters, page: pageParam, perPage: 25 }),
    initialPageParam: 1,
    getNextPageParam: (last) => last.nextPage ?? undefined,
    retry,
    staleTime: 30_000,
  })
}

export function usePull(pid: string, n: number, anonymous = false) {
  return useQuery({
    queryKey: ghk.pull(pid, n),
    queryFn: () => api.get<PullDetail>(ghUrl(pid, `pulls/${n}`)),
    retry,
    refetchInterval: (q) => {
      const p = q.state.data
      const computing = !!p && p.state === 'open' && (p.mergeable === null || p.mergeableState === 'unknown')
      return livePoll(anonymous, !!p && (computing || isActive(p.checks?.state)), computing ? 4_000 : 15_000)
    },
  })
}

export function usePrFiles(pid: string, n: number, enabled = true) {
  return useQuery({
    queryKey: ghk.pullPart(pid, n, 'files'),
    queryFn: () => api.get<PrFiles>(ghUrl(pid, `pulls/${n}/files`)),
    enabled,
    retry,
    staleTime: 60_000,
  })
}

export function usePrFile(pid: string, n: number, f: PrFile | null, base: string, head: string) {
  return useQuery({
    queryKey: ghk.prFile(pid, n, f ? `${f.previousFilename ?? ''}\u0000${f.filename}` : '', base, head),
    queryFn: () =>
      api.get<FileVersions>(ghUrl(pid, `pulls/${n}/file`), {
        path: f!.filename,
        previousPath: f!.previousFilename ?? undefined,
        status: f!.status,
        base,
        head,
      }),
    enabled: !!f && !!base && !!head,
    retry,
    // Contents at fixed commits never change.
    staleTime: Infinity,
    gcTime: 5 * 60_000,
  })
}

export function useThreads(pid: string, n: number, enabled = true) {
  return useQuery({
    queryKey: ghk.pullPart(pid, n, 'threads'),
    queryFn: () => api.get<Thread[]>(ghUrl(pid, `pulls/${n}/threads`)),
    enabled,
    retry,
  })
}

export function useReviews(pid: string, n: number, enabled = true) {
  return useQuery({
    queryKey: ghk.pullPart(pid, n, 'reviews'),
    queryFn: () => api.get<Review[]>(ghUrl(pid, `pulls/${n}/reviews`)),
    enabled,
    retry,
  })
}

export function usePrComments(pid: string, n: number, enabled = true) {
  return useQuery({
    queryKey: ghk.pullPart(pid, n, 'comments'),
    queryFn: () => api.get<IssueComment[]>(ghUrl(pid, `pulls/${n}/comments`)),
    enabled,
    retry,
  })
}

export function usePrCommits(pid: string, n: number, enabled = true) {
  return useQuery({
    queryKey: ghk.pullPart(pid, n, 'commits'),
    queryFn: () => api.get<Commit[]>(ghUrl(pid, `pulls/${n}/commits`)),
    enabled,
    retry,
  })
}

export interface IssueFilters {
  state?: string
  search?: string
}

export function useIssues(pid: string, filters: IssueFilters) {
  return useInfiniteQuery({
    queryKey: ghk.issues(pid, filters),
    queryFn: ({ pageParam }) => api.get<ListPage<Issue>>(ghUrl(pid, 'issues'), { ...filters, page: pageParam, perPage: 25 }),
    initialPageParam: 1,
    getNextPageParam: (last) => last.nextPage ?? undefined,
    retry,
    staleTime: 30_000,
  })
}

export function useIssue(pid: string, n: number) {
  return useQuery({
    queryKey: ghk.issue(pid, n),
    queryFn: () => api.get<IssueDetail>(ghUrl(pid, `issues/${n}`)),
    retry,
  })
}

export function useReleases(pid: string) {
  return useInfiniteQuery({
    queryKey: ghk.releases(pid),
    queryFn: ({ pageParam }) => api.get<ListPage<Release>>(ghUrl(pid, 'releases'), { page: pageParam, perPage: 20 }),
    initialPageParam: 1,
    getNextPageParam: (last) => last.nextPage ?? undefined,
    retry,
    staleTime: 5 * 60_000,
  })
}

export function useBranch(pid: string, name: string | null) {
  return useQuery({
    queryKey: ghk.branch(pid, name ?? ''),
    queryFn: () => api.get<BranchInfo>(ghUrl(pid, 'branch'), { name: name! }),
    enabled: !!name,
    retry,
    staleTime: 30_000,
  })
}

export function fetchChecks(pid: string, sha: string) {
  return api.get<CommitChecks | null>(ghUrl(pid, `commits/${sha}/checks`))
}

// ---------------------------------------------------------------- mutations

export const ghApi = {
  rerun: (pid: string, id: number) => api.post<Run>(ghUrl(pid, `actions/runs/${id}/rerun`)),
  rerunFailed: (pid: string, id: number) => api.post<Run>(ghUrl(pid, `actions/runs/${id}/rerun-failed`)),
  cancel: (pid: string, id: number) => api.post<Run>(ghUrl(pid, `actions/runs/${id}/cancel`)),
  rerunJob: (pid: string, id: number) => api.post<Job>(ghUrl(pid, `actions/jobs/${id}/rerun`)),
  dispatch: (pid: string, workflowId: number, ref: string, inputs: Record<string, string | boolean>) =>
    api.post<{ ok: boolean }>(ghUrl(pid, `actions/workflows/${workflowId}/dispatch`), { ref, inputs }),
  createPr: (pid: string, body: { title: string; body?: string; head?: string; base?: string; draft?: boolean }) =>
    api.post<Pull>(ghUrl(pid, 'pulls'), body),
  updatePr: (pid: string, n: number, body: { title?: string; body?: string; state?: 'open' | 'closed'; draft?: boolean }) =>
    api.patch<PullDetail>(ghUrl(pid, `pulls/${n}`), body),
  review: (pid: string, n: number, body: { event: 'APPROVE' | 'REQUEST_CHANGES' | 'COMMENT'; body?: string; sha?: string | null }) =>
    api.post<Review>(ghUrl(pid, `pulls/${n}/reviews`), { ...body, sha: body.sha ?? undefined }),
  merge: (pid: string, n: number, body: { sha: string; method: 'merge' | 'squash' | 'rebase'; title?: string; message?: string; deleteBranch?: boolean }) =>
    api.post<PullDetail>(ghUrl(pid, `pulls/${n}/merge`), body),
  comment: (pid: string, n: number, body: string) => api.post<IssueComment>(ghUrl(pid, `pulls/${n}/comments`), { body }),
  reviewComment: (pid: string, n: number, body: { body: string; path: string; line?: number; side?: 'LEFT' | 'RIGHT'; commitId?: string }) =>
    api.post<ReviewComment>(ghUrl(pid, `pulls/${n}/review-comments`), body),
  reply: (pid: string, n: number, commentId: number, body: string) =>
    api.post<ReviewComment>(ghUrl(pid, `pulls/${n}/review-comments/${commentId}/replies`), { body }),
  resolve: (pid: string, n: number, threadId: string, resolved: boolean) =>
    api.post<{ id: string; resolved: boolean }>(ghUrl(pid, `pulls/${n}/threads/${encodeURIComponent(threadId)}/resolve`), { resolved }),
  createIssue: (pid: string, body: { title: string; body?: string; labels?: string[] }) => api.post<Issue>(ghUrl(pid, 'issues'), body),
  updateIssue: (pid: string, n: number, body: { state?: 'open' | 'closed'; stateReason?: string; title?: string; body?: string }) =>
    api.patch<Issue>(ghUrl(pid, `issues/${n}`), body),
  issueComment: (pid: string, n: number, body: string) => api.post<IssueComment>(ghUrl(pid, `issues/${n}/comments`), { body }),
  logUrl: (pid: string, jobId: number) => api.url(ghUrl(pid, `actions/jobs/${jobId}/log`)),
  artifactUrl: (pid: string, artifactId: number) => api.url(ghUrl(pid, `actions/artifacts/${artifactId}/zip`)),
}
