// REST calls and react-query hooks of the git slice. Every key starts with
// ['git', projectId] so one `git.changed` event refreshes a project's views;
// commit details are immutable and live under ['git-commit', …].

import { useInfiniteQuery, useQuery } from '@tanstack/react-query'
import { api, ApiError } from '@/api/client'
import type {
  BisectState,
  Branches,
  Changelists,
  CommitDetails,
  Comparison,
  ConflictVersions,
  DiffPanelParams,
  GitFileDiff,
  GitStatus,
  LogFilters,
  LogPage,
  OpOutcome,
  PushPreview,
  RebasePlan,
  ShelfMeta,
  StashDetails,
  StashEntry,
} from './types'

export const LOG_PAGE = 300

export function gitUrl(pid: string, p: string) {
  return `/api/projects/${encodeURIComponent(pid)}/git/${p}`
}

export const gk = {
  all: (pid: string) => ['git', pid] as const,
  status: (pid: string) => ['git', pid, 'status'] as const,
  diffs: (pid: string) => ['git', pid, 'diff'] as const,
  diff: (p: DiffPanelParams) => ['git', p.projectId, 'diff', p.mode, p.path, p.sha ?? '', p.base ?? '', p.head ?? '', p.oldPath ?? ''] as const,
  log: (pid: string, f: LogFilters) => ['git', pid, 'log', f] as const,
  branches: (pid: string) => ['git', pid, 'branches'] as const,
  stashes: (pid: string) => ['git', pid, 'stashes'] as const,
  stash: (pid: string, index: number, sha: string) => ['git', pid, 'stash', index, sha] as const,
  conflict: (pid: string, path: string) => ['git', pid, 'conflict', path] as const,
  pushPreview: (pid: string) => ['git', pid, 'push-preview'] as const,
  compare: (pid: string, base: string, head: string) => ['git', pid, 'compare', base, head] as const,
  commit: (pid: string, sha: string) => ['git-commit', pid, sha] as const,
  changelists: (pid: string) => ['git', pid, 'changelists'] as const,
  shelves: (pid: string) => ['git', pid, 'shelf'] as const,
  shelf: (pid: string, id: string) => ['git', pid, 'shelf', id] as const,
  bisect: (pid: string) => ['git', pid, 'bisect'] as const,
  rebasePlan: (pid: string, from: string, onto: string) => ['git', pid, 'rebase-plan', from, onto] as const,
}

export function isNotRepo(e: unknown) {
  return e instanceof ApiError && e.code === 'not_a_repo'
}

export const gitApi = {
  post: <T = { ok: boolean }>(pid: string, p: string, body?: unknown) => api.post<T>(gitUrl(pid, p), body ?? {}),
  status: (pid: string) => api.get<GitStatus>(gitUrl(pid, 'status')),
  diff: (p: DiffPanelParams) =>
    api.get<GitFileDiff>(gitUrl(p.projectId, 'diff'), {
      path: p.path,
      mode: p.mode,
      sha: p.sha,
      base: p.base,
      head: p.head,
      oldPath: p.oldPath,
    }),
  lastMessage: (pid: string) => api.get<{ message: string }>(gitUrl(pid, 'last-commit-message')),
  outcome: (pid: string, p: string, body?: unknown) => api.post<OpOutcome>(gitUrl(pid, p), body ?? {}),
}

export function useGitStatus(pid: string | null) {
  return useQuery({
    queryKey: gk.status(pid ?? ''),
    queryFn: () => gitApi.status(pid!),
    enabled: !!pid,
    staleTime: 5_000,
    refetchOnWindowFocus: true,
    retry: (n, e) => !isNotRepo(e) && n < 1,
  })
}

export function useBranches(pid: string | null, enabled = true) {
  return useQuery({
    queryKey: gk.branches(pid ?? ''),
    queryFn: () => api.get<Branches>(gitUrl(pid!, 'branches')),
    enabled: !!pid && enabled,
    staleTime: 10_000,
    retry: (n, e) => !isNotRepo(e) && n < 1,
  })
}

export function useStashes(pid: string | null) {
  return useQuery({
    queryKey: gk.stashes(pid ?? ''),
    queryFn: () => api.get<StashEntry[]>(gitUrl(pid!, 'stashes')),
    enabled: !!pid,
    retry: false,
  })
}

export function useStashDetails(pid: string, s: StashEntry | null) {
  return useQuery({
    queryKey: gk.stash(pid, s?.index ?? -1, s?.sha ?? ''),
    queryFn: () => api.get<StashDetails>(gitUrl(pid, `stashes/${s!.index}`)),
    enabled: !!s,
  })
}

export function useFileDiff(p: DiffPanelParams) {
  return useQuery({
    queryKey: gk.diff(p),
    queryFn: () => gitApi.diff(p),
    // Commit and compare diffs never change; working/staged refresh on events.
    staleTime: p.mode === 'commit' ? Infinity : 2_000,
    refetchOnWindowFocus: p.mode === 'working' || p.mode === 'staged',
    retry: false,
  })
}

export function useCommitDetails(pid: string, sha: string | null) {
  return useQuery({
    queryKey: gk.commit(pid, sha ?? ''),
    queryFn: () => api.get<CommitDetails>(gitUrl(pid, `commits/${encodeURIComponent(sha!)}`)),
    enabled: !!sha,
    staleTime: Infinity,
    retry: false,
  })
}

export function useConflict(pid: string, path: string) {
  return useQuery({
    queryKey: gk.conflict(pid, path),
    queryFn: () => api.get<ConflictVersions>(gitUrl(pid, 'conflict'), { path }),
    retry: false,
  })
}

export function usePushPreview(pid: string, enabled: boolean, remote?: string, branch?: string) {
  return useQuery({
    queryKey: [...gk.pushPreview(pid), remote ?? '', branch ?? ''],
    queryFn: () => api.get<PushPreview>(gitUrl(pid, 'push-preview'), { remote, branch }),
    enabled,
    retry: false,
    staleTime: 0,
  })
}

export function useCompare(pid: string, base: string, head: string) {
  return useQuery({
    queryKey: gk.compare(pid, base, head),
    queryFn: () => api.get<Comparison>(gitUrl(pid, 'compare'), { base, head }),
    retry: false,
  })
}

export function useGitLog(pid: string, f: LogFilters) {
  return useInfiniteQuery({
    queryKey: gk.log(pid, f),
    initialPageParam: 0,
    queryFn: ({ pageParam, signal }) =>
      api.get<LogPage>(
        gitUrl(pid, 'log'),
        {
          ref: f.ref,
          all: f.all ? true : undefined,
          path: f.path,
          author: f.author,
          grep: f.grep,
          lines: f.lines,
          worktreeLines: f.lines && f.worktreeLines ? true : undefined,
          skip: pageParam,
          limit: LOG_PAGE,
        },
        signal,
      ),
    getNextPageParam: (last, pages) => (last.hasMore ? pages.reduce((n, p) => n + p.commits.length, 0) : undefined),
    staleTime: 15_000,
    retry: (n, e) => !isNotRepo(e) && n < 1,
  })
}

export function useChangelists(pid: string | null, enabled = true) {
  return useQuery({
    queryKey: gk.changelists(pid ?? ''),
    queryFn: () => api.get<Changelists>(gitUrl(pid!, 'changelists')),
    enabled: !!pid && enabled,
    staleTime: 5_000,
    retry: (n, e) => !isNotRepo(e) && n < 1,
  })
}

export function useShelves(pid: string | null) {
  return useQuery({
    queryKey: gk.shelves(pid ?? ''),
    queryFn: () => api.get<ShelfMeta[]>(gitUrl(pid!, 'shelf')),
    enabled: !!pid,
    staleTime: 30_000,
    retry: false,
  })
}

/** One shelf with its view commit (built on demand on the server). */
export function useShelf(pid: string, id: string | null) {
  return useQuery({
    queryKey: gk.shelf(pid, id ?? ''),
    queryFn: () => api.get<ShelfMeta>(gitUrl(pid, `shelf/${encodeURIComponent(id!)}`)),
    enabled: !!id,
    staleTime: 60_000,
    retry: false,
  })
}

export function useBisect(pid: string | null, enabled = true) {
  return useQuery({
    queryKey: gk.bisect(pid ?? ''),
    queryFn: () => api.get<BisectState>(gitUrl(pid!, 'bisect')),
    enabled: !!pid && enabled,
    staleTime: 5_000,
    retry: false,
  })
}

export function useRebasePlan(pid: string, from?: string, onto?: string) {
  return useQuery({
    queryKey: gk.rebasePlan(pid, from ?? '', onto ?? ''),
    queryFn: () => api.get<RebasePlan>(gitUrl(pid, 'rebase/plan'), { from, onto }),
    staleTime: 0,
    gcTime: 0,
    retry: false,
    refetchOnWindowFocus: false,
  })
}
