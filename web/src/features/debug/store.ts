// Client state of the debugger: live sessions (from `debug.session`), each session's
// console, the selected thread and frame, and the stack of the selected thread. The
// tool window can be hidden and shown again, so none of this lives in components.

import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import type { DebugSession, Frame, OutputLine } from './types'

const MAX_CONSOLE = 3000

export interface Selection {
  threadId?: number
  frameIndex: number
}

export interface StackState {
  epoch: number
  threadId: number
  frames: Frame[]
  error?: string
  loading: boolean
}

interface DebugStore {
  sessions: Record<string, DebugSession>
  /** Selected session tab per project. */
  active: Record<string, string | undefined>
  selection: Record<string, Selection>
  stacks: Record<string, StackState | undefined>
  output: Record<string, OutputLine[]>
  /** Console entries up to this sequence are hidden ("Clear console"). */
  cleared: Record<string, number>
  clearOutput: (sid: string) => void
  /** Watches that stopped the program when evaluated: evaluated on request only. */
  manualWatches: Record<string, string[]>
  markManual: (sid: string, expr: string) => void
  upsert: (s: DebugSession) => void
  remove: (id: string) => void
  setActive: (pid: string, sid: string | undefined) => void
  select: (sid: string, sel: Partial<Selection>) => void
  setStack: (sid: string, st: StackState | undefined) => void
  appendOutput: (sid: string, lines: OutputLine[]) => void
  setOutput: (sid: string, lines: OutputLine[]) => void
}

export const useDebug = create<DebugStore>()((set) => ({
  sessions: {},
  active: {},
  selection: {},
  stacks: {},
  output: {},
  cleared: {},
  clearOutput: (sid) =>
    set((st) => {
      const lines = st.output[sid] ?? []
      return { cleared: { ...st.cleared, [sid]: lines.length ? lines[lines.length - 1].seq : 0 } }
    }),
  manualWatches: {},
  markManual: (sid, expr) =>
    set((st) => {
      const cur = st.manualWatches[sid] ?? []
      return cur.includes(expr) ? st : { manualWatches: { ...st.manualWatches, [sid]: [...cur, expr] } }
    }),
  upsert: (s) =>
    set((st) => {
      const prev = st.sessions[s.id]
      // Events can arrive out of order with REST answers: never go back in epochs.
      if (prev && prev.stopEpoch > s.stopEpoch && prev.state !== 'terminated' && prev.state !== 'failed') return st
      return { sessions: { ...st.sessions, [s.id]: s } }
    }),
  remove: (id) =>
    set((st) => {
      const { [id]: _s, ...sessions } = st.sessions
      const { [id]: _o, ...output } = st.output
      const { [id]: _k, ...stacks } = st.stacks
      const active = { ...st.active }
      for (const [pid, sid] of Object.entries(active)) if (sid === id) active[pid] = undefined
      return { sessions, output, stacks, active }
    }),
  setActive: (pid, sid) => set((st) => ({ active: { ...st.active, [pid]: sid } })),
  select: (sid, sel) => set((st) => ({ selection: { ...st.selection, [sid]: { ...(st.selection[sid] ?? { frameIndex: 0 }), ...sel } } })),
  setStack: (sid, stack) => set((st) => ({ stacks: { ...st.stacks, [sid]: stack } })),
  appendOutput: (sid, lines) =>
    set((st) => {
      const cur = st.output[sid] ?? []
      const last = cur.length ? cur[cur.length - 1].seq : 0
      const fresh = lines.filter((l) => l.seq > last)
      if (!fresh.length) return st
      const next = cur.concat(fresh)
      return { output: { ...st.output, [sid]: next.length > MAX_CONSOLE ? next.slice(next.length - MAX_CONSOLE) : next } }
    }),
  setOutput: (sid, lines) => set((st) => ({ output: { ...st.output, [sid]: lines.slice(-MAX_CONSOLE) } })),
}))

export type DebugTab = 'frames' | 'console' | 'breakpoints' | 'peripherals'

export type PickerKind = 'debug' | 'attach'

/** The "Debug…" and "Attach to Process…" pickers. `config`: attach that launch
 *  configuration (it has no pid) to the process picked. */
export const usePicker = create<{
  kind: PickerKind | null
  pid: string | null
  config: string | null
  set: (kind: PickerKind | null, pid?: string | null, config?: string | null) => void
}>()((set) => ({
  kind: null,
  pid: null,
  config: null,
  set: (kind, pid, config = null) => set((s) => ({ kind, pid: pid === undefined ? s.pid : pid, config })),
}))

export function openDebugPicker(pid: string) {
  usePicker.getState().set('debug', pid)
}

export function openAttachPicker(pid: string, config?: string) {
  usePicker.getState().set('attach', pid, config ?? null)
}

/** Per-browser conveniences: the tool window tab, the picked configuration, pane sizes. */
interface DebugPrefs {
  tab: DebugTab
  config: Record<string, string>
  framesWidth: number
  setTab: (t: DebugTab) => void
  setConfig: (pid: string, name: string) => void
  setFramesWidth: (w: number) => void
}

export const useDebugPrefs = create<DebugPrefs>()(
  persist(
    (set) => ({
      tab: 'frames',
      config: {},
      framesWidth: 320,
      setTab: (tab) => set({ tab }),
      setConfig: (pid, name) => set((s) => ({ config: { ...s.config, [pid]: name } })),
      setFramesWidth: (framesWidth) => set({ framesWidth }),
    }),
    { name: 'wb.debug.v1' },
  ),
)

/** Sessions of a project, newest first. */
export function sessionsOf(all: Record<string, DebugSession>, pid: string | null): DebugSession[] {
  if (!pid) return []
  return Object.values(all)
    .filter((s) => s.projectId === pid)
    .sort((a, b) => b.startedAt - a.startedAt)
}

/** The session the project's tool window shows. */
export function activeSession(st: Pick<DebugStore, 'sessions' | 'active'>, pid: string | null): DebugSession | null {
  if (!pid) return null
  const sid = st.active[pid]
  const s = sid ? st.sessions[sid] : undefined
  if (s) return s
  const list = sessionsOf(st.sessions, pid)
  return list.find((x) => x.state === 'stopped') ?? list.find((x) => x.state !== 'terminated' && x.state !== 'failed') ?? list[0] ?? null
}
