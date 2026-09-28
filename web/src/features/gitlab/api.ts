// REST client, query keys and hooks for /api/projects/{pid}/gitlab/**.
// Every key starts with ['gitlab', projectId] so an event for a project can
// invalidate exactly its views (see GitlabEvents in providers.tsx).

import { useInfiniteQuery, useQuery } from '@tanstack/react-query'
import { api, ApiError } from '@/api/client'
import { useProjects } from '@/api/queries'
import { isActive } from './logic'
import type {
  BranchInfo,
  Commit,
  Discussion,
  Environment,
  FileVersions,
  GitlabSummary,
  Issue,
  IssueDetail,
  Job,
  ListPage,
  Mr,
  MrDiffs,
  Note,
  Pipeline,
  PipelineDetail,
  RegistryRepo,
  RegistryTag,
  TagsPage,
  TestFailures,
  TraceChunk,
  TraceTail,
  Deployment,
} from './types'

export const gl = (pid: string) => `/api/projects/${encodeURIComponent(pid)}/gitlab`

export const glk = {
  all: (pid: string) => ['gitlab', pid] as const,
  summary: (pid: string) => ['gitlab', pid, 'summary'] as const,
  pipelines: (pid: string, f?: object) => (f ? (['gitlab', pid, 'pipelines', f] as const) : (['gitlab', pid, 'pipelines'] as const)),
  pipeline: (pid: string, id: number) => ['gitlab', pid, 'pipeline', id] as const,
  tests: (pid: string, id: number) => ['gitlab', pid, 'pipeline', id, 'tests'] as const,
  job: (pid: string, id: number) => ['gitlab', pid, 'job', id] as const,
  mrs: (pid: string, f?: object) => (f ? (['gitlab', pid, 'mrs', f] as const) : (['gitlab', pid, 'mrs'] as const)),
  mr: (pid: string, iid: number) => ['gitlab', pid, 'mr', iid] as const,
  mrPart: (pid: string, iid: number, part: string) => ['gitlab', pid, 'mr', iid, part] as const,
  mrFile: (pid: string, iid: number, path: string, base: string, head: string) =>
    ['gitlab', pid, 'mrfile', iid, path, base, head] as const,
  issues: (pid: string, f?: object) => (f ? (['gitlab', pid, 'issues', f] as const) : (['gitlab', pid, 'issues'] as const)),
  issue: (pid: string, iid: number) => ['gitlab', pid, 'issue', iid] as const,
  envs: (pid: string) => ['gitlab', pid, 'envs'] as const,
  deployments: (pid: string, env: string) => ['gitlab', pid, 'deployments', env] as const,
  registry: (pid: string) => ['gitlab', pid, 'registry'] as const,
  tags: (pid: string, rid: number) => ['gitlab', pid, 'tags', rid] as const,
  tag: (pid: string, rid: number, tag: string) => ['gitlab', pid, 'tag', rid, tag] as const,
  branch: (pid: string, name: string) => ['gitlab', pid, 'branch', name] as const,
}

/** Don't retry what retrying cannot fix (setup, permissions, missing things). */
function retry(count: number, e: unknown) {
  if (e instanceof ApiError && [400, 401, 403, 404, 409, 412].includes(e.status)) return false
  return count < 2
}

/** The current project's summary entry says whether it is on GitLab. */
export function useHasGitlab(pid: string | null): boolean {
  const { data } = useProjects()
  return !!pid && !!data?.find((p) => p.id === pid)?.gitlab
}

export function useGitlabSummary(pid: string | null, enabled = true) {
  return useQuery({
    queryKey: glk.summary(pid ?? ''),
    queryFn: () => api.get<GitlabSummary>(`${gl(pid!)}/summary`),
    enabled: !!pid && enabled,
    retry,
    staleTime: 15_000,
    // Events keep it fresh; this is the fallback while something runs.
    refetchInterval: (q) => {
      const s = q.state.data
      return s && (isActive(s.branchPipeline?.status) || isActive(s.headStatus?.status)) ? 15_000 : 120_000
    },
  })
}

export interface PipelineFilters {
  ref?: string
  status?: string
}

export function usePipelines(pid: string, filters: PipelineFilters, perPage = 30) {
  return useInfiniteQuery({
    queryKey: glk.pipelines(pid, { ...filters, perPage }),
    queryFn: ({ pageParam }) =>
      api.get<ListPage<Pipeline>>(`${gl(pid)}/pipelines`, { ...filters, page: pageParam, perPage }),
    initialPageParam: 1,
    getNextPageParam: (last) => last.nextPage ?? undefined,
    retry,
    staleTime: 20_000,
    refetchInterval: (q) => (q.state.data?.pages[0]?.items.some((p) => isActive(p.status)) ? 10_000 : false),
  })
}

export function usePipeline(pid: string, id: number) {
  return useQuery({
    queryKey: glk.pipeline(pid, id),
    queryFn: () => api.get<PipelineDetail>(`${gl(pid)}/pipelines/${id}`),
    retry,
    refetchInterval: (q) => {
      const d = q.state.data
      return d && (isActive(d.pipeline.status) || d.stages.some((s) => s.jobs.some((j) => isActive(j.status)))) ? 5_000 : false
    },
  })
}

/** The pipeline's failed tests; `updatedAt` refetches them when the pipeline changes. */
export function useTestFailures(pid: string, id: number, enabled: boolean, updatedAt?: string | null) {
  return useQuery({
    queryKey: [...glk.tests(pid, id), updatedAt ?? ''],
    queryFn: ({ signal }) => api.get<TestFailures>(`${gl(pid)}/pipelines/${id}/tests`, undefined, signal),
    enabled,
    retry,
    staleTime: 60_000,
  })
}

export function useJob(pid: string, id: number) {
  return useQuery({
    queryKey: glk.job(pid, id),
    queryFn: () => api.get<Job>(`${gl(pid)}/jobs/${id}`),
    retry,
  })
}

export function fetchTrace(pid: string, jobId: number, offset: number, signal?: AbortSignal) {
  return api.get<TraceChunk>(`${gl(pid)}/jobs/${jobId}/trace`, { offset }, signal)
}

export function fetchTraceTail(pid: string, jobId: number, lines: number) {
  return api.get<TraceTail>(`${gl(pid)}/jobs/${jobId}/trace`, { tail: lines, plain: true })
}

export interface MrFilters {
  state?: string
  search?: string
  scope?: string
}

export function useMrs(pid: string, filters: MrFilters) {
  return useInfiniteQuery({
    queryKey: glk.mrs(pid, filters),
    queryFn: ({ pageParam }) =>
      api.get<ListPage<Mr>>(`${gl(pid)}/mrs`, { ...filters, page: pageParam, perPage: 30 }),
    initialPageParam: 1,
    getNextPageParam: (last) => last.nextPage ?? undefined,
    retry,
    staleTime: 30_000,
  })
}

export function useMr(pid: string, iid: number) {
  return useQuery({
    queryKey: glk.mr(pid, iid),
    queryFn: () => api.get<Mr>(`${gl(pid)}/mrs/${iid}`),
    retry,
    refetchInterval: (q) => {
      const m = q.state.data
      const checking = m && ['checking', 'unchecked', 'preparing', 'approvals_syncing'].includes(m.detailedMergeStatus ?? '')
      return m && (checking || m.rebaseInProgress || isActive(m.headPipeline?.status)) ? 8_000 : false
    },
  })
}

export function useMrDiffs(pid: string, iid: number, enabled = true) {
  return useQuery({
    queryKey: glk.mrPart(pid, iid, 'diffs'),
    queryFn: () => api.get<MrDiffs>(`${gl(pid)}/mrs/${iid}/diffs`),
    enabled,
    retry,
    staleTime: 60_000,
  })
}

export function useMrFile(
  pid: string,
  iid: number,
  f: { oldPath: string; newPath: string; newFile: boolean; deletedFile: boolean } | null,
  base: string,
  head: string,
) {
  return useQuery({
    queryKey: glk.mrFile(pid, iid, f ? `${f.oldPath}\u0000${f.newPath}` : '', base, head),
    queryFn: () =>
      api.get<FileVersions>(
        `${gl(pid)}/mrs/${iid}/file`,
        { oldPath: f!.oldPath, newPath: f!.newPath, newFile: f!.newFile, deletedFile: f!.deletedFile, base, head },
      ),
    enabled: !!f && !!base && !!head,
    retry,
    // Contents at fixed commits never change.
    staleTime: Infinity,
    gcTime: 5 * 60_000,
  })
}

export function useMrDiscussions(pid: string, iid: number, enabled = true) {
  return useQuery({
    queryKey: glk.mrPart(pid, iid, 'discussions'),
    queryFn: () => api.get<Discussion[]>(`${gl(pid)}/mrs/${iid}/discussions`),
    enabled,
    retry,
  })
}

export function useMrCommits(pid: string, iid: number, enabled = true) {
  return useQuery({
    queryKey: glk.mrPart(pid, iid, 'commits'),
    queryFn: () => api.get<Commit[]>(`${gl(pid)}/mrs/${iid}/commits`),
    enabled,
    retry,
  })
}

export function useMrPipelines(pid: string, iid: number, enabled = true) {
  return useQuery({
    queryKey: glk.mrPart(pid, iid, 'pipelines'),
    queryFn: () => api.get<Pipeline[]>(`${gl(pid)}/mrs/${iid}/pipelines`),
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
    queryKey: glk.issues(pid, filters),
    queryFn: ({ pageParam }) =>
      api.get<ListPage<Issue>>(`${gl(pid)}/issues`, { ...filters, page: pageParam, perPage: 30 }),
    initialPageParam: 1,
    getNextPageParam: (last) => last.nextPage ?? undefined,
    retry,
    staleTime: 30_000,
  })
}

export function useIssue(pid: string, iid: number) {
  return useQuery({
    queryKey: glk.issue(pid, iid),
    queryFn: () => api.get<IssueDetail>(`${gl(pid)}/issues/${iid}`),
    retry,
  })
}

export function useEnvironments(pid: string, enabled = true) {
  return useQuery({
    queryKey: glk.envs(pid),
    queryFn: () => api.get<Environment[]>(`${gl(pid)}/environments`),
    enabled,
    retry,
    staleTime: 60_000,
  })
}

export function useDeployments(pid: string, env: string, enabled = true) {
  return useQuery({
    queryKey: glk.deployments(pid, env),
    queryFn: () =>
      api.get<ListPage<Deployment>>(`${gl(pid)}/deployments`, { environment: env, perPage: 10 }),
    enabled,
    retry,
  })
}

export function useRegistry(pid: string, enabled = true) {
  return useQuery({
    queryKey: glk.registry(pid),
    queryFn: () => api.get<RegistryRepo[]>(`${gl(pid)}/registry`),
    enabled,
    retry,
    staleTime: 5 * 60_000,
  })
}

export function useTags(pid: string, rid: number | null) {
  return useInfiniteQuery({
    queryKey: glk.tags(pid, rid ?? 0),
    queryFn: ({ pageParam }) =>
      api.get<TagsPage>(`${gl(pid)}/registry/${rid}/tags`, { cursor: pageParam || undefined, perPage: 50 }),
    initialPageParam: '',
    getNextPageParam: (last) => last.nextCursor ?? undefined,
    enabled: rid !== null,
    retry,
    staleTime: 60_000,
  })
}

export function useTag(pid: string, rid: number, tag: string | null) {
  return useQuery({
    queryKey: glk.tag(pid, rid, tag ?? ''),
    queryFn: () => api.get<RegistryTag>(`${gl(pid)}/registry/${rid}/tags/${encodeURIComponent(tag!)}`),
    enabled: !!tag,
    retry,
    staleTime: 5 * 60_000,
  })
}

export function useBranch(pid: string, name: string | null) {
  return useQuery({
    queryKey: glk.branch(pid, name ?? ''),
    queryFn: () => api.get<BranchInfo>(`${gl(pid)}/branch`, { name: name! }),
    enabled: !!name,
    retry,
  })
}

// ---------------------------------------------------------------- mutations

export const glApi = {
  retryPipeline: (pid: string, id: number) => api.post<Pipeline>(`${gl(pid)}/pipelines/${id}/retry`),
  cancelPipeline: (pid: string, id: number) => api.post<Pipeline>(`${gl(pid)}/pipelines/${id}/cancel`),
  runPipeline: (pid: string, ref: string, variables: { key: string; value: string }[]) =>
    api.post<Pipeline>(`${gl(pid)}/pipelines`, { ref, variables }),
  retryJob: (pid: string, id: number) => api.post<Job>(`${gl(pid)}/jobs/${id}/retry`),
  cancelJob: (pid: string, id: number) => api.post<Job>(`${gl(pid)}/jobs/${id}/cancel`),
  playJob: (pid: string, id: number) => api.post<Job>(`${gl(pid)}/jobs/${id}/play`),
  createMr: (
    pid: string,
    body: {
      sourceBranch?: string
      targetBranch?: string
      title: string
      description?: string
      draft?: boolean
      removeSourceBranch?: boolean
      squash?: boolean
    },
  ) => api.post<Mr>(`${gl(pid)}/mrs`, body),
  updateMr: (pid: string, iid: number, body: { title?: string; description?: string; stateEvent?: 'close' | 'reopen'; draft?: boolean }) =>
    api.put<Mr>(`${gl(pid)}/mrs/${iid}`, body),
  approve: (pid: string, iid: number, sha?: string | null) => api.post<Mr>(`${gl(pid)}/mrs/${iid}/approve`, { sha: sha ?? undefined }),
  unapprove: (pid: string, iid: number) => api.post<Mr>(`${gl(pid)}/mrs/${iid}/unapprove`),
  merge: (
    pid: string,
    iid: number,
    body: { sha: string; squash?: boolean; removeSourceBranch?: boolean; autoMerge?: boolean; mergeCommitMessage?: string },
  ) => api.post<Mr>(`${gl(pid)}/mrs/${iid}/merge`, body),
  rebase: (pid: string, iid: number) => api.post<{ rebaseInProgress: boolean }>(`${gl(pid)}/mrs/${iid}/rebase`),
  addNote: (pid: string, iid: number, body: string) => api.post<Note>(`${gl(pid)}/mrs/${iid}/notes`, { body }),
  addDiscussion: (
    pid: string,
    iid: number,
    body: string,
    position?: { oldPath?: string; newPath: string; oldLine?: number; newLine?: number },
  ) => api.post<Discussion>(`${gl(pid)}/mrs/${iid}/discussions`, { body, position }),
  reply: (pid: string, iid: number, did: string, body: string) =>
    api.post<Note>(`${gl(pid)}/mrs/${iid}/discussions/${encodeURIComponent(did)}/notes`, { body }),
  resolve: (pid: string, iid: number, did: string, resolved: boolean) =>
    api.put<Discussion>(`${gl(pid)}/mrs/${iid}/discussions/${encodeURIComponent(did)}`, { resolved }),
  createIssue: (pid: string, body: { title: string; description?: string; labels?: string[] }) =>
    api.post<Issue>(`${gl(pid)}/issues`, body),
  updateIssue: (pid: string, iid: number, body: { stateEvent?: 'close' | 'reopen'; title?: string; description?: string }) =>
    api.put<Issue>(`${gl(pid)}/issues/${iid}`, body),
  issueNote: (pid: string, iid: number, body: string) => api.post<Note>(`${gl(pid)}/issues/${iid}/notes`, { body }),
  artifactsUrl: (pid: string, jobId: number) => api.url(`${gl(pid)}/jobs/${jobId}/artifacts`),
  logUrl: (pid: string, jobId: number) => api.url(`${gl(pid)}/jobs/${jobId}/log`),
}
