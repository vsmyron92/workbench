// Client state of the git UI: commit message drafts (persisted per browser),
// running remote operations, the branches popover and git dialogs.

import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import type { Changelist, GitOpEvent, RebaseAction, ShelfMeta } from './types'

// ---------------------------------------------------------------- view preferences

export type CommitTab = 'changes' | 'stash' | 'shelf'
export type GroupBy = 'staging' | 'changelists'

interface PrefsState {
  groupBy: GroupBy
  tab: CommitTab
  setGroupBy: (g: GroupBy) => void
  setTab: (t: CommitTab) => void
}

/** Commit window view: staging area (Staged / Unstaged) or CLion changelists; the open tab. */
export const useGitPrefs = create<PrefsState>()(
  persist(
    (set) => ({
      groupBy: 'staging',
      tab: 'changes',
      setGroupBy: (groupBy) => set({ groupBy }),
      setTab: (tab) => set({ tab }),
    }),
    { name: 'wb.git.prefs.v1' },
  ),
)

// ---------------------------------------------------------------- commit inclusion (changelist view)

export interface PartialSelection {
  /** Fingerprint of the HEAD → working tree diff the lines were picked in. */
  fingerprint: string
  /** Line keys (`a:12`, `d:5`, see lineSelection.ts). */
  keys: string[]
}

export interface Inclusion {
  /** Files ticked for the next commit; null = the default (the active changelist). */
  included: string[] | null
  /** Files of which only some lines are included. */
  partial: Record<string, PartialSelection>
}

const emptyInclusion: Inclusion = { included: null, partial: {} }

interface InclusionState {
  byProject: Record<string, Inclusion>
  setIncluded: (pid: string, paths: string[]) => void
  setPartial: (pid: string, path: string, sel: PartialSelection | null) => void
  reset: (pid: string) => void
}

/** CLion's partial commit: which files (and lines) the changelist view commits. */
export const useInclusion = create<InclusionState>()(
  persist(
    (set) => ({
      byProject: {},
      setIncluded: (pid, paths) =>
        set((s) => ({ byProject: { ...s.byProject, [pid]: { ...(s.byProject[pid] ?? emptyInclusion), included: paths } } })),
      setPartial: (pid, path, sel) =>
        set((s) => {
          const cur = s.byProject[pid] ?? emptyInclusion
          const partial = { ...cur.partial }
          if (sel && sel.keys.length) partial[path] = sel
          else delete partial[path]
          return { byProject: { ...s.byProject, [pid]: { ...cur, partial } } }
        }),
      reset: (pid) =>
        set((s) => {
          const byProject = { ...s.byProject }
          delete byProject[pid]
          return { byProject }
        }),
    }),
    { name: 'wb.git.inclusion.v1' },
  ),
)

export function useProjectInclusion(pid: string | null): Inclusion {
  return useInclusion((s) => (pid ? s.byProject[pid] : undefined)) ?? emptyInclusion
}

// ---------------------------------------------------------------- commit drafts

export interface CommitDraft {
  message: string
  amend: boolean
  signoff: boolean
  /** The message loaded for amend (cleared again when amend is turned off). */
  amendLoaded?: string
}

const emptyDraft: CommitDraft = { message: '', amend: false, signoff: false }

interface DraftState {
  drafts: Record<string, CommitDraft>
  /** Incremented to ask the commit window to focus its message box. */
  focusTick: number
  update: (pid: string, patch: Partial<CommitDraft>) => void
  reset: (pid: string) => void
  focus: () => void
}

export const useDrafts = create<DraftState>()(
  persist(
    (set) => ({
      drafts: {},
      focusTick: 0,
      update: (pid, patch) => set((s) => ({ drafts: { ...s.drafts, [pid]: { ...(s.drafts[pid] ?? emptyDraft), ...patch } } })),
      reset: (pid) => set((s) => ({ drafts: { ...s.drafts, [pid]: { ...emptyDraft, signoff: s.drafts[pid]?.signoff ?? false } } })),
      focus: () => set((s) => ({ focusTick: s.focusTick + 1 })),
    }),
    { name: 'wb.git.drafts.v1', partialize: (s) => ({ drafts: s.drafts }) },
  ),
)

export function useDraft(pid: string | null): CommitDraft {
  return useDrafts((s) => (pid ? s.drafts[pid] : undefined)) ?? emptyDraft
}

// ---------------------------------------------------------------- remote operations

export interface OpState {
  opId: string
  projectId: string | null
  op: string
  title: string
  lines: string[]
  lastLine: string
  done: boolean
  ok?: boolean
  message?: string
  /** Finished with files in conflict (the card stays up and offers to resolve). */
  conflicts?: boolean
  /** A rebase stopped (edit step, conflicts): the card offers Continue / Abort. */
  stopped?: boolean
  startedAt: number
  /** Log expanded in the progress card. */
  showLog?: boolean
}

interface OpsStore {
  ops: Record<string, OpState>
  start: (o: { opId: string; projectId: string; op: string; title: string }) => void
  event: (projectId: string | null, e: GitOpEvent) => void
  fail: (opId: string, message: string) => void
  toggleLog: (opId: string) => void
  dismiss: (opId: string) => void
}

const MAX_LINES = 300

export const useOps = create<OpsStore>()((set, get) => ({
  ops: {},
  start: (o) =>
    set((s) => ({ ops: { ...s.ops, [o.opId]: { ...o, lines: [], lastLine: 'Starting…', done: false, startedAt: Date.now() } } })),
  event: (projectId, e) => {
    const cur: OpState = get().ops[e.opId] ?? {
      opId: e.opId,
      projectId,
      op: e.op,
      title: e.title ?? opTitle(e.op),
      lines: [],
      lastLine: '',
      done: false,
      startedAt: Date.now(),
    }
    if (cur.done && !e.done) return
    const next: OpState = { ...cur, title: e.title ?? cur.title }
    if (e.line !== undefined) {
      next.lastLine = e.line
      next.lines = [...cur.lines, e.line].slice(-MAX_LINES)
    }
    if (e.done) {
      next.done = true
      next.ok = e.ok
      next.message = e.message
      next.conflicts = !!e.conflicts
      next.stopped = !!e.stopped
      if (e.ok && !e.conflicts) window.setTimeout(() => get().dismiss(e.opId), 4000)
    }
    set((s) => ({ ops: { ...s.ops, [e.opId]: next } }))
  },
  fail: (opId, message) =>
    set((s) => (s.ops[opId] ? { ops: { ...s.ops, [opId]: { ...s.ops[opId], done: true, ok: false, message } } } : s)),
  toggleLog: (opId) => set((s) => (s.ops[opId] ? { ops: { ...s.ops, [opId]: { ...s.ops[opId], showLog: !s.ops[opId].showLog } } } : s)),
  dismiss: (opId) =>
    set((s) => {
      const ops = { ...s.ops }
      delete ops[opId]
      return { ops }
    }),
}))

export function opTitle(op: string) {
  return { fetch: 'Fetch', pull: 'Update', push: 'Push', 'delete-remote-branch': 'Delete remote branch', rebase: 'Rebase' }[op] ?? op
}

/** The running op of a project (for the status bar). */
export function useRunningOp(pid: string | null): OpState | null {
  return useOps((s) => Object.values(s.ops).find((o) => !o.done && (!pid || o.projectId === pid)) ?? null)
}

// ---------------------------------------------------------------- popover & dialogs

export type GitDialog =
  | { kind: 'push'; projectId: string }
  | { kind: 'stash'; projectId: string }
  | { kind: 'newBranch'; projectId: string; startPoint?: string; startLabel?: string }
  | { kind: 'reset'; projectId: string; sha: string; subject: string }
  | { kind: 'compare'; projectId: string; base: string; head: string }
  | { kind: 'log'; title: string; lines: string[] }
  /** Interactive rebase from a commit (inclusive) or onto a branch; `focus` preselects a row. */
  | { kind: 'rebase'; projectId: string; from?: string; onto?: string; focus?: string; preset?: RebaseAction }
  | { kind: 'bisectStart'; projectId: string; good?: string; bad?: string }
  | { kind: 'shelve'; projectId: string; paths: string[]; changelist?: string; name?: string }
  | { kind: 'unshelve'; projectId: string; shelf: ShelfMeta; paths?: string[] }
  | { kind: 'pickBranch'; projectId: string; title: string; confirmLabel?: string; onPick: (ref: string) => void }
  | { kind: 'changelist'; projectId: string; list?: Changelist; paths?: string[] }

interface UiStore {
  popover: { projectId: string; anchor: DOMRect | null; from: 'topbar' | 'statusbar' } | null
  dialog: GitDialog | null
  openPopover: (projectId: string, anchor: DOMRect | null, from: 'topbar' | 'statusbar') => void
  closePopover: () => void
  openDialog: (d: GitDialog) => void
  closeDialog: () => void
}

export const useGitUi = create<UiStore>()((set) => ({
  popover: null,
  dialog: null,
  openPopover: (projectId, anchor, from) => set({ popover: { projectId, anchor, from } }),
  closePopover: () => set({ popover: null }),
  openDialog: (d) => set({ dialog: d, popover: null }),
  closeDialog: () => set({ dialog: null }),
}))
