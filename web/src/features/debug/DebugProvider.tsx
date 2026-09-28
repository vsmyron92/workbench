// Mounted once: keeps the debugger's client state in step with the server
// (`debug.session`, `debug.output`, `debug.breakpoints`, resync), loads the stack of
// every suspended session (the editor shows its execution point), brings the stop
// into view, owns the CLion stepping keys while a session runs, and hosts the dialogs.

import { useEffect, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { subscribe, useEvent } from '@/api/events'
import { isMobileShell, showToolWindow } from '@/shell/actions'
import { useUi } from '@/state/store'
import { control, currentSession, openFrame, stopSession } from './actions'
import { applyBreakpoints, debugApi, debugKeys, setDebugQueryClient } from './api'
import { BreakpointDialogHost } from './BreakpointDialog'
import { installEditorIntegration } from './editor'
import { isLive, revealFrame } from './logic'
import { PickerHost } from './Pickers'
import { useDebug } from './store'
import type { BreakpointsView, DebugSession, OutputLine } from './types'

/** Load the stack of the selected thread of a suspended session (once per epoch). */
async function loadStack(s: DebugSession, reveal: boolean) {
  const st = useDebug.getState()
  const sel = st.selection[s.id]
  const threadId = sel?.threadId ?? s.stopped?.threadId ?? s.threads[0]?.id
  if (threadId == null) return
  const cur = st.stacks[s.id]
  if (cur && cur.epoch === s.stopEpoch && cur.threadId === threadId && (cur.loading || cur.frames.length || cur.error)) return
  st.setStack(s.id, { epoch: s.stopEpoch, threadId, frames: [], loading: true })
  try {
    const r = await debugApi.stack(s.projectId, s.id, threadId)
    const now = useDebug.getState().sessions[s.id]
    if (!now || now.stopEpoch !== s.stopEpoch) return // resumed meanwhile
    useDebug.getState().setStack(s.id, { epoch: s.stopEpoch, threadId, frames: r.frames, loading: false })
    const frameIndex = revealFrame(r.frames, s.stopped?.reason)
    useDebug.getState().select(s.id, { threadId, frameIndex })
    // A stop brings its source to the front (a background tab would hide it). Not on
    // a phone: debugging is desktop-only, and the phone has no editor tab.
    if (reveal && s.projectId === useUi.getState().projectId && !isMobileShell()) {
      const f = r.frames[frameIndex]
      if (f) openFrame(s, f, true)
      showToolWindow('debug', 'bottom')
    }
  } catch (e) {
    const now = useDebug.getState().sessions[s.id]
    if (now?.stopEpoch === s.stopEpoch) useDebug.getState().setStack(s.id, { epoch: s.stopEpoch, threadId, frames: [], loading: false, error: (e as Error).message })
  }
}

async function loadOutput(s: DebugSession) {
  const have = useDebug.getState().output[s.id]
  const after = have?.length ? have[have.length - 1].seq : 0
  if (after >= s.outputSeq && have) return
  try {
    const r = await debugApi.output(s.projectId, s.id, after)
    if (!have || r.dropped) useDebug.getState().setOutput(s.id, [...(r.dropped ? [] : (have ?? [])), ...r.lines])
    else useDebug.getState().appendOutput(s.id, r.lines)
  } catch {
    // The session may be gone; the next event retries.
  }
}

async function loadSessions(pid: string) {
  try {
    const list = await debugApi.sessions(pid)
    const st = useDebug.getState()
    for (const old of Object.values(st.sessions)) if (old.projectId === pid && !list.some((s) => s.id === old.id)) st.remove(old.id)
    for (const s of list) {
      st.upsert(s)
      void loadOutput(s)
    }
  } catch {
    // Offline or project gone: events bring the state back.
  }
}

/** The CLion stepping keys, taken before the editor (Monaco binds F8, Shift+F8 and
 *  Ctrl+F2 to other things) while the current project has a live session. Terminals
 *  keep every key. */
function onKey(e: KeyboardEvent) {
  if (e.defaultPrevented || !e.key.startsWith('F')) return
  const target = e.target as HTMLElement | null
  if (target?.closest?.('.xterm')) return
  // A widget that handles one of these keys itself says so (`data-wb-keys`): the git
  // diff viewer's F7 is Next Difference, as in CLion's diff viewer.
  const owner = target?.closest?.('[data-wb-keys]')
  if (owner && (owner.getAttribute('data-wb-keys') ?? '').split(' ').includes(e.key)) return
  const s = currentSession()
  if (!s || !isLive(s)) return
  const mod = e.ctrlKey || e.metaKey
  let run: (() => void) | null = null
  if (e.key === 'F9' && !mod && !e.shiftKey && !e.altKey) run = () => void control('continue', s)
  else if (e.key === 'F8' && !mod && !e.altKey) run = () => void control(e.shiftKey ? 'stepOut' : 'next', s)
  else if (e.key === 'F7' && !mod && !e.shiftKey && !e.altKey) run = () => void control('stepIn', s)
  else if (e.key === 'F2' && mod && !e.shiftKey && !e.altKey) run = () => void stopSession(s)
  if (!run) return
  e.preventDefault()
  e.stopPropagation()
  run()
}

export function DebugProvider({ children }: { children?: ReactNode }) {
  const qc = useQueryClient()
  const pid = useUi((s) => s.projectId)

  useEffect(() => {
    setDebugQueryClient(qc)
    installEditorIntegration()
    window.addEventListener('keydown', onKey, true)
    return () => {
      window.removeEventListener('keydown', onKey, true)
      setDebugQueryClient(null)
    }
  }, [qc])

  // Sessions of the current project (others arrive through events).
  useEffect(() => {
    if (pid) void loadSessions(pid)
  }, [pid])

  // Stacks of suspended sessions follow their epochs.
  useEffect(
    () =>
      useDebug.subscribe((st, prev) => {
        if (st.sessions === prev.sessions && st.selection === prev.selection) return
        for (const s of Object.values(st.sessions)) {
          if (s.state !== 'stopped') continue
          const stack = st.stacks[s.id]
          const sel = st.selection[s.id]
          const threadChanged = !!stack && sel?.threadId != null && sel.threadId !== stack.threadId
          if (!stack || stack.epoch !== s.stopEpoch || threadChanged) void loadStack(s, !stack || stack.epoch !== s.stopEpoch)
        }
      }),
    [],
  )

  useEvent<DebugSession | { id: string; removed: true }>('debug.session', (ev) => {
    const d = ev.data
    if ('removed' in d) {
      useDebug.getState().remove(d.id)
      return
    }
    const st = useDebug.getState()
    const prev = st.sessions[d.id]
    // A watch that stopped the program (it called a function with a breakpoint)
    // would stop it again at every evaluation: from now on it waits for a click.
    if (d.stopped?.duringEvaluation) st.markManual(d.id, d.stopped.duringEvaluation)
    // A new stop selects its thread before the stack loads (the store's listener
    // loads the stack of the selected thread).
    if (d.state === 'stopped' && d.stopped?.threadId != null && prev?.stopEpoch !== d.stopEpoch) {
      st.select(d.id, { threadId: d.stopped.threadId, frameIndex: 0 })
      st.setActive(d.projectId, d.id)
    }
    if (d.state !== 'stopped' && st.stacks[d.id]) st.setStack(d.id, undefined)
    st.upsert(d)
    // Output a flood kept out of events (the server flushes output before state, so
    // a sequence ahead of ours means lines were skipped): fetch them.
    const have = st.output[d.id]
    if (prev && have && d.outputSeq > (have.length ? have[have.length - 1].seq : 0)) void loadOutput(d)
    if (!prev) {
      // A new session of this project becomes the one the tool window shows.
      const cur = st.active[d.projectId]
      if (!cur || !isLive(useDebug.getState().sessions[cur])) st.setActive(d.projectId, d.id)
      void loadOutput(d)
    }
  })

  useEvent<{ sessionId: string; lines: OutputLine[] }>('debug.output', (ev) => {
    const { sessionId, lines } = ev.data
    const have = useDebug.getState().output[sessionId]
    const last = have?.length ? have[have.length - 1].seq : 0
    if (lines.length && lines[0].seq > last + 1) {
      // Missed some (a flood, a reconnect): fetch the gap.
      const s = useDebug.getState().sessions[sessionId]
      if (s) void loadOutput({ ...s, outputSeq: lines[lines.length - 1].seq })
      return
    }
    useDebug.getState().appendOutput(sessionId, lines)
  })

  useEvent<BreakpointsView>('debug.breakpoints', (ev) => {
    if (ev.projectId) applyBreakpoints(ev.projectId, ev.data)
  })

  useEffect(
    () =>
      subscribe('resync', () => {
        const p = useUi.getState().projectId
        if (p) {
          void loadSessions(p)
          void qc.invalidateQueries({ queryKey: debugKeys.breakpoints(p) })
        }
        // Frame ids may be stale after a gap: reload the stacks of suspended sessions.
        for (const s of Object.values(useDebug.getState().sessions)) {
          if (s.state !== 'stopped') continue
          useDebug.getState().setStack(s.id, undefined)
          void loadStack(s, false)
        }
      }),
    [qc],
  )

  return (
    <>
      {children}
      <BreakpointDialogHost />
      <PickerHost />
    </>
  )
}
