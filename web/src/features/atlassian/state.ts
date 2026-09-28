// Client state for the atlassian slice: the last status check (read synchronously by
// tool-window `when` predicates), per-browser preferences, and requests to open the
// slice's dialogs from commands.

import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import type { AtlassianStatus } from './api'
import { pushRecent } from './links'

interface StatusState {
  status: AtlassianStatus | null
  /** `not_configured`, `unauthorized`… when the status call failed. */
  errorCode: string | null
  errorMessage: string | null
  set: (s: Partial<StatusState>) => void
}

export const useAtlassianStatus = create<StatusState>()((set) => ({
  status: null,
  errorCode: null,
  errorMessage: null,
  set: (s) => set(s),
}))

export const jiraAvailable = () => !!useAtlassianStatus.getState().status?.jira
export const confluenceAvailable = () => !!useAtlassianStatus.getState().status?.confluence

export interface RecentPage {
  id: string
  title: string
  spaceKey: string | null
  at: number
}

export interface RecentIssue {
  id: string
  summary: string
  at: number
}

interface Prefs {
  /** Selected space per project ('' = no project). */
  spaceByProject: Record<string, string>
  /** Show the whole space instead of the project's root pages. */
  wholeSpace: Record<string, boolean>
  archived: boolean
  recent: RecentPage[]
  recentIssues: RecentIssue[]
  /** Expanded tree nodes (`<type>:<id>`), bounded. */
  expanded: string[]
  jiraFilter: Record<string, string>
  jiraJql: Record<string, string>
  commentsOpen: boolean
  /** The page panel's side pane (comments or attachments), or none. */
  sidePane: 'comments' | 'attachments' | null
  /** Jira tool window mode. */
  jiraMode: 'issues' | 'boards'
  /** Last sprint picked per board (`<pid>|<boardId>` → sprint id, 'backlog' or 'board'). */
  boardScope: Record<string, string>
  setSpace: (pid: string | null, spaceId: string) => void
  setWholeSpace: (pid: string | null, v: boolean) => void
  setArchived: (v: boolean) => void
  addRecent: (p: Omit<RecentPage, 'at'>) => void
  addRecentIssue: (i: Omit<RecentIssue, 'at'>) => void
  toggleExpanded: (key: string, open?: boolean) => void
  setJira: (pid: string | null, filter: string, jql: string) => void
  setCommentsOpen: (v: boolean) => void
  setSidePane: (v: 'comments' | 'attachments' | null) => void
  setJiraMode: (v: 'issues' | 'boards') => void
  setBoardScope: (key: string, scope: string) => void
}

const MAX_EXPANDED = 400

export const usePrefs = create<Prefs>()(
  persist(
    (set) => ({
      spaceByProject: {},
      wholeSpace: {},
      archived: false,
      recent: [],
      recentIssues: [],
      expanded: [],
      jiraFilter: {},
      jiraJql: {},
      commentsOpen: false,
      sidePane: null,
      jiraMode: 'issues',
      boardScope: {},
      setSpace: (pid, spaceId) => set((s) => ({ spaceByProject: { ...s.spaceByProject, [pid ?? '']: spaceId } })),
      setWholeSpace: (pid, v) => set((s) => ({ wholeSpace: { ...s.wholeSpace, [pid ?? '']: v } })),
      setArchived: (archived) => set({ archived }),
      addRecent: (p) => set((s) => ({ recent: pushRecent(s.recent, { ...p, at: Date.now() }) })),
      addRecentIssue: (i) => set((s) => ({ recentIssues: pushRecent(s.recentIssues, { ...i, at: Date.now() }) })),
      toggleExpanded: (key, open) =>
        set((s) => {
          const has = s.expanded.includes(key)
          const want = open ?? !has
          if (want === has) return s
          return { expanded: want ? [key, ...s.expanded].slice(0, MAX_EXPANDED) : s.expanded.filter((k) => k !== key) }
        }),
      setJira: (pid, filter, jql) =>
        set((s) => ({ jiraFilter: { ...s.jiraFilter, [pid ?? '']: filter }, jiraJql: { ...s.jiraJql, [pid ?? '']: jql } })),
      setCommentsOpen: (commentsOpen) => set({ commentsOpen }),
      setSidePane: (sidePane) => set({ sidePane, commentsOpen: sidePane === 'comments' }),
      setJiraMode: (jiraMode) => set({ jiraMode }),
      setBoardScope: (key, scope) =>
        set((s) => {
          const next = { ...s.boardScope, [key]: scope }
          // Bounded: the oldest entries go first.
          const keys = Object.keys(next)
          return { boardScope: keys.length > 100 ? Object.fromEntries(keys.slice(-100).map((k) => [k, next[k]])) : next }
        }),
    }),
    { name: 'wb.atlassian.v1' },
  ),
)

export interface PageOp {
  kind: 'move' | 'copy'
  page: { id: string; title: string }
  projectId: string | null
}

interface UiRequests {
  newPage: { spaceId?: string; parentId?: string; parentTitle?: string } | null
  createIssue: { projectKey?: string } | null
  pageOp: PageOp | null
  openPageOp: (op: PageOp | null) => void
  /** Incremented to ask the tool window to focus its search box. */
  focusSearch: number
  focusJql: number
  openNewPage: (r: UiRequests['newPage']) => void
  openCreateIssue: (r: UiRequests['createIssue']) => void
  requestSearchFocus: () => void
  requestJqlFocus: () => void
}

export const useAtlassianUi = create<UiRequests>()((set) => ({
  newPage: null,
  createIssue: null,
  pageOp: null,
  openPageOp: (pageOp) => set({ pageOp }),
  focusSearch: 0,
  focusJql: 0,
  openNewPage: (newPage) => set({ newPage }),
  openCreateIssue: (createIssue) => set({ createIssue }),
  requestSearchFocus: () => set((s) => ({ focusSearch: s.focusSearch + 1 })),
  requestJqlFocus: () => set((s) => ({ focusJql: s.focusJql + 1 })),
}))
