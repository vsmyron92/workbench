// Types and queries for the atlassian slice (mirrors server/src/atlassian/*.rs, camelCase).

import { useInfiniteQuery, useQuery, useQueryClient, type QueryClient } from '@tanstack/react-query'
import { api } from '@/api/client'
import { useEvent } from '@/api/events'

// ---------------------------------------------------------------- status

export interface AtlassianUser {
  accountId: string
  displayName: string
  email: string | null
}

export interface AtlassianStatus {
  configured: boolean
  site: string
  user: AtlassianUser | null
  confluence: boolean
  jira: boolean
  jiraTitle: string | null
  authFailed: boolean
  error: string | null
  checkedAt: number
  /** Where the server's config.toml is (`~`-contracted), for setup help. */
  configFile?: string
}

// ---------------------------------------------------------------- confluence

export interface Space {
  id: string
  key: string
  name: string
  type: string
  status: string
  homepageId: string | null
}

export type ContentType = 'page' | 'folder' | 'whiteboard' | 'database' | 'embed'

export interface TreeNode {
  id: string
  title: string
  type: ContentType | string
  status: string
  hasChildren: boolean | null
  position: number | null
  parentId: string | null
  spaceId: string | null
}

export interface ChildrenOut {
  children: TreeNode[]
  truncated: boolean
}

export interface PageVersion {
  number: number
  message: string
  minorEdit: boolean
  createdAt: string
  authorId: string
  authorName: string | null
}

export interface Crumb {
  id: string
  title: string
  type: string
}

export interface Page {
  id: string
  title: string
  status: string
  spaceId: string
  spaceKey: string | null
  spaceName: string | null
  parentId: string | null
  version: PageVersion
  webUrl: string
  editUrl: string | null
  ancestors: Crumb[]
  labels: string[]
  html: string
  storage: string
  hasInlineCommentMarkers: boolean
  /** Display names of the accounts the storage mentions (accountId → name). */
  users: Record<string, string>
  inlineMarkerRefs: string[]
  historical: boolean
}

export interface VersionsPage {
  results: PageVersion[]
  nextCursor: string | null
}

export interface VersionBody {
  number: number
  title: string
  storage: string
  createdAt: string
  message: string
  authorName: string | null
}

export interface Comment {
  id: string
  kind: 'footer' | 'inline'
  authorId: string
  authorName: string | null
  createdAt: string
  version: number
  html: string
  webUrl: string | null
  selection: string | null
  markerRef: string | null
  resolutionStatus: string | null
  /** Who resolved or reopened it last (name), and when. */
  resolvedBy: string | null
  resolvedAt: string | null
  /** The body as markdown for editing (mentions as [@Name](mention:id)). */
  markdown: string
  /** Saving the markdown back would lose formatting (macros, page links…). */
  editLossy: boolean
  replies: Comment[]
}

export interface Comments {
  footer: Comment[]
  inline: Comment[]
  truncated: boolean
}

export interface SearchHit {
  id: string
  type: string
  title: string
  excerpt: string
  spaceKey: string | null
  spaceName: string | null
  status: string
  lastModified: string | null
  webUrl: string | null
}

export interface SearchOut {
  cql: string
  results: SearchHit[]
  nextCursor: string | null
  totalSize: number | null
}

export interface UpdateIn {
  title?: string
  storage?: string
  markdown?: string
  /** The version the edit started from; required (the server refuses updates without it). */
  version: number
  message?: string
  force?: boolean
  minorEdit?: boolean
}

export interface UpdateOut {
  id: string
  title: string
  version: number
  webUrl: string
  unchanged: boolean
}

export interface CreateIn {
  spaceId?: string
  spaceKey?: string
  parentId?: string
  title: string
  storage?: string
  markdown?: string
}

export interface CreatedOut {
  id: string
  title: string
  spaceId: string
  version: number
  webUrl: string
}

export interface Attachment {
  id: string
  title: string
  mediaType: string
  fileSize: number | null
  comment: string
  createdAt: string | null
  version: number
  authorId: string
  authorName: string | null
  /** Workbench's proxy URL for the bytes. */
  downloadUrl: string
  isImage: boolean
}

export interface AttachmentsOut {
  attachments: Attachment[]
  truncated: boolean
}

export interface UserHit {
  accountId: string
  displayName: string
  email: string | null
}

export interface InlineCreated {
  id: string
  markerRef: string | null
  matchCount: number
  matchIndex: number
}

export interface CommentWritten {
  id: string
  pageId: string | null
  version: number
  resolutionStatus: string | null
}

// ---------------------------------------------------------------- jira

export interface JiraUser {
  accountId: string
  displayName: string
}

export interface JiraStatus {
  id: string | null
  name: string
  /** new | indeterminate | done */
  category: string
}

export interface Named {
  id: string | null
  name: string
}

export interface IssueSummary {
  key: string
  id: string
  summary: string
  status: JiraStatus | null
  assignee: JiraUser | null
  priority: Named | null
  issueType: Named | null
  updated: string | null
  labels: string[]
  projectKey: string | null
}

export interface JiraSearchOut {
  issues: IssueSummary[]
  nextPageToken: string | null
  isLast: boolean
}

export interface JiraTransition {
  id: string
  name: string
  to: JiraStatus | null
  hasScreen: boolean
}

export interface JiraComment {
  id: string
  author: JiraUser | null
  created: string | null
  updated: string | null
  html: string
  markdown: string
}

export interface Issue extends IssueSummary {
  reporter: JiraUser | null
  created: string | null
  resolution: string | null
  due: string | null
  projectName: string | null
  parent: { key: string; summary: string | null } | null
  descriptionHtml: string
  descriptionMarkdown: string
  descriptionLossy: boolean
  transitions: JiraTransition[]
  editable: { summary: boolean; description: boolean; labels: boolean; priority: boolean; assignee: boolean; priorities: Named[] }
  comments: JiraComment[]
  commentsTotal: number
  webUrl: string
}

export interface Board {
  id: number
  name: string
  /** scrum | kanban | simple */
  type: string
  projectKey: string | null
  projectName: string | null
}

export interface BoardColumn {
  name: string
  statusIds: string[]
  min: number | null
  max: number | null
}

export interface QuickFilter {
  id: number
  name: string
  jql: string
  description: string | null
}

export interface BoardDetail extends Board {
  columns: BoardColumn[]
  quickFilters: QuickFilter[]
  hasSprints: boolean
  webUrl: string
}

export interface Sprint {
  id: number
  name: string
  /** active | future | closed */
  state: string
  goal: string | null
  startDate: string | null
  endDate: string | null
  completeDate: string | null
}

export interface BoardIssues {
  issues: IssueSummary[]
  total: number
  truncated: boolean
}

/** Which issues of a board to show: its active sprint's, a sprint's, the backlog or all. */
export type BoardScope = { kind: 'board' } | { kind: 'sprint'; sprintId: number } | { kind: 'backlog' }

export interface JiraProject {
  id: string
  key: string
  name: string
}

export interface IssueType {
  id: string
  name: string
  subtask: boolean
  description: string | null
}

// ---------------------------------------------------------------- query keys and hooks

type Pid = string | null | undefined

export const qk = {
  status: (pid: Pid) => ['atlassian', 'status', pid ?? null] as const,
  spaces: (pid: Pid) => ['confluence', 'spaces', pid ?? null] as const,
  roots: (pid: Pid, spaceId: string, status: string) => ['confluence', 'roots', pid ?? null, spaceId, status] as const,
  children: (pid: Pid, id: string, type: string, archived: boolean) => ['confluence', 'children', pid ?? null, id, type, archived] as const,
  byIds: (pid: Pid, ids: string[]) => ['confluence', 'byIds', pid ?? null, ids.join(',')] as const,
  page: (pid: Pid, id: string) => ['confluence', 'page', pid ?? null, id] as const,
  comments: (pid: Pid, id: string) => ['confluence', 'comments', pid ?? null, id] as const,
  versions: (pid: Pid, id: string) => ['confluence', 'versions', pid ?? null, id] as const,
  version: (pid: Pid, id: string, n: number) => ['confluence', 'version', pid ?? null, id, n] as const,
  search: (pid: Pid, q: string, space: string, archived: boolean) => ['confluence', 'search', pid ?? null, q, space, archived] as const,
  jiraSearch: (pid: Pid, jql: string) => ['jira', 'search', pid ?? null, jql] as const,
  issue: (pid: Pid, key: string) => ['jira', 'issue', pid ?? null, key] as const,
  jiraProjects: (pid: Pid) => ['jira', 'projects', pid ?? null] as const,
  issueTypes: (pid: Pid, project: string) => ['jira', 'types', pid ?? null, project] as const,
  attachments: (pid: Pid, id: string) => ['confluence', 'attachments', pid ?? null, id] as const,
  watch: (pid: Pid, id: string) => ['confluence', 'watch', pid ?? null, id] as const,
  users: (pid: Pid, q: string) => ['confluence', 'users', pid ?? null, q] as const,
  boards: (pid: Pid) => ['jira', 'boards', pid ?? null] as const,
  board: (pid: Pid, id: number) => ['jira', 'board', pid ?? null, id] as const,
  sprints: (pid: Pid, id: number) => ['jira', 'sprints', pid ?? null, id] as const,
  boardIssues: (pid: Pid, id: number, scope: string, jql: string) => ['jira', 'boardIssues', pid ?? null, id, scope, jql] as const,
  transitions: (pid: Pid, key: string) => ['jira', 'transitions', pid ?? null, key] as const,
}

const pq = (pid: Pid) => ({ projectId: pid ?? undefined })

export const confluenceApi = {
  status: (pid: Pid, refresh = false) => api.get<AtlassianStatus>('/api/atlassian/status', { ...pq(pid), refresh: refresh ? 1 : undefined }),
  spaces: (pid: Pid) => api.get<Space[]>('/api/confluence/spaces', pq(pid)),
  roots: (pid: Pid, spaceId: string, status: string) =>
    api.get<ChildrenOut>(`/api/confluence/spaces/${encodeURIComponent(spaceId)}/pages`, { ...pq(pid), status }),
  children: (pid: Pid, id: string, type: string, archived: boolean) =>
    api.get<ChildrenOut>(`/api/confluence/pages/${encodeURIComponent(id)}/children`, { ...pq(pid), type, archived }),
  byIds: (pid: Pid, ids: string[]) => api.get<TreeNode[]>('/api/confluence/pages', { ...pq(pid), ids: ids.join(',') }),
  page: (pid: Pid, id: string) => api.get<Page>(`/api/confluence/pages/${encodeURIComponent(id)}`, pq(pid)),
  update: (pid: Pid, id: string, body: UpdateIn) => api.put<UpdateOut>(`/api/confluence/pages/${encodeURIComponent(id)}`, body, pq(pid)),
  create: (pid: Pid, body: CreateIn) => api.post<CreatedOut>('/api/confluence/pages', body, pq(pid)),
  versions: (pid: Pid, id: string, cursor?: string | null) =>
    api.get<VersionsPage>(`/api/confluence/pages/${encodeURIComponent(id)}/versions`, { ...pq(pid), cursor: cursor ?? undefined, limit: 50 }),
  version: (pid: Pid, id: string, n: number) => api.get<VersionBody>(`/api/confluence/pages/${encodeURIComponent(id)}/versions/${n}`, pq(pid)),
  comments: (pid: Pid, id: string) => api.get<Comments>(`/api/confluence/pages/${encodeURIComponent(id)}/comments`, pq(pid)),
  addComment: (pid: Pid, id: string, body: { markdown: string; parentCommentId?: string; parentKind?: string }) =>
    api.post<{ id: string }>(`/api/confluence/pages/${encodeURIComponent(id)}/comments`, body, pq(pid)),
  search: (pid: Pid, q: string, space: string, archived: boolean, cursor?: string | null) =>
    api.get<SearchOut>('/api/confluence/search', { ...pq(pid), q, space, archived, cursor: cursor ?? undefined, limit: 25 }),
  markdown: (markdown: string) => api.post<{ storage: string }>('/api/confluence/markdown', { markdown }),
  searchCql: (pid: Pid, cql: string, limit = 25) => api.get<SearchOut>('/api/confluence/search', { ...pq(pid), cql, limit }),
  addInlineComment: (pid: Pid, id: string, body: { markdown: string; selection: string; matchIndex: number; matchCount: number }) =>
    api.post<InlineCreated>(`/api/confluence/pages/${encodeURIComponent(id)}/inline-comments`, body, pq(pid)),
  updateComment: (pid: Pid, kind: 'inline' | 'footer', id: string, body: { markdown?: string; version?: number; resolved?: boolean }) =>
    api.put<CommentWritten>(`/api/confluence/comments/${kind}/${encodeURIComponent(id)}`, body, pq(pid)),
  deleteComment: (pid: Pid, kind: 'inline' | 'footer', id: string) =>
    api.del<{ ok: boolean }>(`/api/confluence/comments/${kind}/${encodeURIComponent(id)}`, pq(pid)),
  attachments: (pid: Pid, id: string) => api.get<AttachmentsOut>(`/api/confluence/pages/${encodeURIComponent(id)}/attachments`, pq(pid)),
  upload: (
    pid: Pid,
    id: string,
    file: Blob,
    opts: { name: string; comment?: string; replace?: boolean; onProgress?: (sent: number, total: number) => void; signal?: AbortSignal },
  ) =>
    api.upload<Attachment>(
      `/api/confluence/pages/${encodeURIComponent(id)}/attachments`,
      file,
      { ...pq(pid), name: opts.name, comment: opts.comment, replace: opts.replace ? true : undefined },
      opts.onProgress,
      opts.signal,
    ),
  deleteAttachment: (pid: Pid, att: string) => api.del<{ ok: boolean; pageId: string | null }>(`/api/confluence/attachments/${encodeURIComponent(att)}`, pq(pid)),
  addLabels: (pid: Pid, id: string, names: string[]) =>
    api.post<{ labels: string[] }>(`/api/confluence/pages/${encodeURIComponent(id)}/labels`, { names }, pq(pid)),
  removeLabel: (pid: Pid, id: string, name: string) =>
    api.del<{ ok: boolean }>(`/api/confluence/pages/${encodeURIComponent(id)}/labels/${encodeURIComponent(name)}`, pq(pid)),
  move: (pid: Pid, id: string, position: 'before' | 'after' | 'append', targetId: string) =>
    api.post<{ ok: boolean }>(`/api/confluence/pages/${encodeURIComponent(id)}/move`, { position, targetId }, pq(pid)),
  copy: (pid: Pid, id: string, body: { title?: string; parentId?: string; spaceKey?: string; copyAttachments?: boolean; copyLabels?: boolean }) =>
    api.post<CreatedOut>(`/api/confluence/pages/${encodeURIComponent(id)}/copy`, body, pq(pid)),
  trash: (pid: Pid, id: string) =>
    api.del<{ ok: boolean; title: string; spaceId: string; parentId: string | null }>(`/api/confluence/pages/${encodeURIComponent(id)}`, pq(pid)),
  restore: (pid: Pid, id: string) => api.post<{ ok: boolean; title: string }>(`/api/confluence/pages/${encodeURIComponent(id)}/restore`, {}, pq(pid)),
  watching: (pid: Pid, id: string) => api.get<{ watching: boolean }>(`/api/confluence/pages/${encodeURIComponent(id)}/watch`, pq(pid)),
  setWatching: (pid: Pid, id: string, watching: boolean) =>
    api.put<{ watching: boolean }>(`/api/confluence/pages/${encodeURIComponent(id)}/watch`, { watching }, pq(pid)),
  users: (pid: Pid, q: string, signal?: AbortSignal) => api.get<UserHit[]>('/api/confluence/users', { ...pq(pid), q, limit: 8 }, signal),
}

export const jiraApi = {
  search: (pid: Pid, jql: string, nextPageToken?: string | null) =>
    api.post<JiraSearchOut>('/api/jira/search', { jql, nextPageToken: nextPageToken ?? undefined, maxResults: 50 }, pq(pid)),
  issue: (pid: Pid, key: string) => api.get<Issue>(`/api/jira/issues/${encodeURIComponent(key)}`, pq(pid)),
  update: (pid: Pid, key: string, body: { summary?: string; description?: string; labels?: string[]; priorityId?: string }) =>
    api.put<{ ok: boolean }>(`/api/jira/issues/${encodeURIComponent(key)}`, body, pq(pid)),
  transition: (pid: Pid, key: string, transitionId: string) =>
    api.post<{ ok: boolean }>(`/api/jira/issues/${encodeURIComponent(key)}/transitions`, { transitionId }, pq(pid)),
  comment: (pid: Pid, key: string, markdown: string) =>
    api.post<{ id: string }>(`/api/jira/issues/${encodeURIComponent(key)}/comments`, { markdown }, pq(pid)),
  assign: (pid: Pid, key: string, accountId: string | null) =>
    api.put<{ ok: boolean }>(`/api/jira/issues/${encodeURIComponent(key)}/assignee`, { accountId }, pq(pid)),
  projects: (pid: Pid) => api.get<JiraProject[]>('/api/jira/projects', pq(pid)),
  issueTypes: (pid: Pid, project: string) => api.get<IssueType[]>(`/api/jira/createmeta/${encodeURIComponent(project)}`, pq(pid)),
  create: (
    pid: Pid,
    body: { projectKey: string; issueTypeId?: string; summary: string; description?: string; labels?: string[]; parentKey?: string },
  ) => api.post<{ id: string; key: string; webUrl: string }>('/api/jira/issues', body, pq(pid)),
  transitions: (pid: Pid, key: string) => api.get<JiraTransition[]>(`/api/jira/issues/${encodeURIComponent(key)}/transitions`, pq(pid)),
  boards: (pid: Pid) => api.get<{ boards: Board[]; truncated: boolean }>('/api/jira/boards', pq(pid)),
  board: (pid: Pid, id: number) => api.get<BoardDetail>(`/api/jira/boards/${id}`, pq(pid)),
  sprints: (pid: Pid, id: number, state = 'active,future,closed') => api.get<Sprint[]>(`/api/jira/boards/${id}/sprints`, { ...pq(pid), state }),
  boardIssues: (pid: Pid, id: number, scope: BoardScope, jql?: string) =>
    api.get<BoardIssues>(`/api/jira/boards/${id}/issues`, {
      ...pq(pid),
      sprintId: scope.kind === 'sprint' ? scope.sprintId : undefined,
      backlog: scope.kind === 'backlog' ? true : undefined,
      jql: jql || undefined,
    }),
}

/** Re-check the status on the server, skipping its cache, and store the answer. */
export function refreshAtlassianStatus(qc: QueryClient, pid: Pid): Promise<AtlassianStatus | undefined> {
  return qc.fetchQuery({ queryKey: qk.status(pid), queryFn: () => confluenceApi.status(pid, true), staleTime: 0 }).catch(() => undefined)
}

/** The status says Atlassian cannot be used as configured (not set up, or credentials rejected). */
export function needsSetup(s: AtlassianStatus): boolean {
  return !s.configured || s.authFailed
}

export function useAtlassianStatusQuery(pid: Pid, enabled = true) {
  return useQuery({
    queryKey: qk.status(pid),
    queryFn: () => confluenceApi.status(pid),
    staleTime: 5 * 60_000,
    refetchInterval: 10 * 60_000,
    retry: false,
    enabled,
  })
}

export function useSpaces(pid: Pid, enabled = true) {
  return useQuery({ queryKey: qk.spaces(pid), queryFn: () => confluenceApi.spaces(pid), staleTime: 10 * 60_000, enabled, retry: false })
}

export function usePage(pid: Pid, id: string) {
  return useQuery({ queryKey: qk.page(pid, id), queryFn: () => confluenceApi.page(pid, id), staleTime: 60_000, retry: false, enabled: !!id })
}

export function useComments(pid: Pid, id: string, enabled: boolean) {
  return useQuery({ queryKey: qk.comments(pid, id), queryFn: () => confluenceApi.comments(pid, id), enabled, staleTime: 60_000, retry: false })
}

/** The comment threads without their replies: enough to colour a page's highlights. */
export function useCommentStates(pid: Pid, id: string, enabled: boolean) {
  return useQuery({
    queryKey: [...qk.comments(pid, id), 'states'],
    queryFn: () => api.get<Comments>(`/api/confluence/pages/${encodeURIComponent(id)}/comments`, { ...pq(pid), replies: false }),
    enabled,
    staleTime: 60_000,
    retry: false,
  })
}

export function useVersions(pid: Pid, id: string, enabled: boolean) {
  return useInfiniteQuery({
    queryKey: qk.versions(pid, id),
    queryFn: ({ pageParam }) => confluenceApi.versions(pid, id, pageParam),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.nextCursor,
    enabled,
    staleTime: 60_000,
  })
}

export function useVersionBody(pid: Pid, id: string, n: number | null) {
  return useQuery({
    queryKey: qk.version(pid, id, n ?? 0),
    queryFn: () => confluenceApi.version(pid, id, n!),
    enabled: n !== null && n > 0,
    // A version's content never changes.
    staleTime: Infinity,
    gcTime: 10 * 60_000,
  })
}

export function useAttachments(pid: Pid, id: string, enabled = true) {
  return useQuery({ queryKey: qk.attachments(pid, id), queryFn: () => confluenceApi.attachments(pid, id), enabled: enabled && !!id, staleTime: 60_000, retry: false })
}

export function useWatching(pid: Pid, id: string, enabled = true) {
  return useQuery({ queryKey: qk.watch(pid, id), queryFn: () => confluenceApi.watching(pid, id), enabled: enabled && !!id, staleTime: 5 * 60_000, retry: false })
}

export function useBoards(pid: Pid, enabled = true) {
  return useQuery({ queryKey: qk.boards(pid), queryFn: () => jiraApi.boards(pid), enabled, staleTime: 5 * 60_000, retry: false })
}

export function useBoard(pid: Pid, id: number) {
  return useQuery({ queryKey: qk.board(pid, id), queryFn: () => jiraApi.board(pid, id), enabled: id > 0, staleTime: 5 * 60_000, retry: false })
}

export function useSprints(pid: Pid, id: number, enabled: boolean) {
  return useQuery({ queryKey: qk.sprints(pid, id), queryFn: () => jiraApi.sprints(pid, id), enabled: enabled && id > 0, staleTime: 60_000, retry: false })
}

export function useIssue(pid: Pid, key: string) {
  return useQuery({ queryKey: qk.issue(pid, key), queryFn: () => jiraApi.issue(pid, key), staleTime: 30_000, retry: false, enabled: !!key })
}

/** Keep Confluence and Jira caches fresh when pages or issues change (here, in other tabs, or by agents). */
export function useAtlassianInvalidation() {
  const qc = useQueryClient()
  useEvent<{ pageId?: string }>('confluence.page', (ev) => {
    const id = ev.data?.pageId
    qc.invalidateQueries({
      predicate: (q) => {
        const k = q.queryKey as unknown[]
        if (k[0] !== 'confluence') return false
        if (k[1] === 'children' || k[1] === 'roots' || k[1] === 'search' || k[1] === 'byIds') return true
        return id !== undefined && (k[1] === 'page' || k[1] === 'comments' || k[1] === 'versions' || k[1] === 'attachments') && k[3] === id
      },
    })
  })
  useEvent<{ key?: string }>('jira.issue', (ev) => {
    const key = ev.data?.key
    qc.invalidateQueries({
      predicate: (q) => {
        const k = q.queryKey as unknown[]
        return k[0] === 'jira' && (k[1] === 'search' || k[1] === 'boardIssues' || ((k[1] === 'issue' || k[1] === 'transitions') && k[3] === key))
      },
    })
  })
  // Settings (site, email, token secret) or a project's [links] changed: check the status
  // again (the server keys its cache by the credentials, so a fixed token is seen at once)
  // and retry whatever failed, e.g. with "not set up" or rejected credentials.
  const onConfigChanged = () =>
    qc.invalidateQueries({
      predicate: (q) => {
        const k = q.queryKey as unknown[]
        if (k[0] === 'atlassian' && k[1] === 'status') return true
        return (k[0] === 'confluence' || k[0] === 'jira') && q.state.status === 'error'
      },
    })
  useEvent('settings.changed', onConfigChanged)
  useEvent('projects.changed', onConfigChanged)
}
