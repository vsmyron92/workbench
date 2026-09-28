// Debugger actions shared by the tool window, the palette, the editor gutter and the
// keyboard: start, step, stop, breakpoints, navigation to frames, "ask agent".

import { ApiError } from '@/api/client'
import { askAgent } from '@/shell/agentBridge'
import { openPanel, showToolWindow, toast, toastError } from '@/shell/actions'
import { useUi } from '@/state/store'
import { applyBreakpoints, cachedBreakpoints, cachedConfigs, debugApi } from './api'
import { agentPrompt, fileBreakpoints, isLive, moveLines, patchLine, rememberedConfig, toggleLine } from './logic'
import { activeSession, openAttachPicker, openDebugPicker, useDebug, useDebugPrefs } from './store'
import type { DebugSession, Frame, LineBreakpoint } from './types'

export function currentProject(): string | null {
  return useUi.getState().projectId
}

export function currentSession(pid: string | null = currentProject()): DebugSession | null {
  return activeSession(useDebug.getState(), pid)
}

function showDebug(tab?: 'frames' | 'console' | 'breakpoints') {
  if (tab) useDebugPrefs.getState().setTab(tab)
  showToolWindow('debug', 'bottom')
}

/** Start launch configuration `name` (default: the one picked in this browser or
 *  started last; with neither, the Debug… picker opens instead of starting one the
 *  user did not choose). `processId`: the process an attach configuration without a
 *  pid attaches to (without it, the server answers `pid_required` and the Attach
 *  picker asks). */
export async function startDebug(pid: string, name?: string, stopOnEntry?: boolean, processId?: number): Promise<DebugSession | null> {
  const cached = cachedConfigs(pid)
  const picked = name ?? rememberedConfig(cached?.configs ?? [], useDebugPrefs.getState().config[pid], cached?.lastConfig)
  if (!picked) {
    openDebugPicker(pid)
    return null
  }
  useDebugPrefs.getState().setConfig(pid, picked)
  try {
    const s = await debugApi.start(pid, picked, stopOnEntry, processId)
    useDebug.getState().upsert(s)
    useDebug.getState().setActive(pid, s.id)
    showDebug(s.state === 'stopped' ? 'frames' : 'console')
    return s
  } catch (e) {
    if (e instanceof ApiError && e.code === 'pid_required') {
      openAttachPicker(pid, picked)
      return null
    }
    showDebug()
    toastError(e, `Could not debug ${picked}`)
    return null
  }
}

export async function attachTo(pid: string, body: { pid: number; adapter?: string; language?: string; program?: string }) {
  try {
    const s = await debugApi.attach(pid, body)
    useDebug.getState().upsert(s)
    useDebug.getState().setActive(pid, s.id)
    showDebug('frames')
    return s
  } catch (e) {
    toastError(e, `Could not attach to ${body.pid}`)
    return null
  }
}

export type StepAction = 'continue' | 'pause' | 'next' | 'stepIn' | 'stepOut'

/** Resume, pause or step the given (default: the current project's) session. */
export async function control(action: StepAction, s: DebugSession | null = currentSession()) {
  if (!s || !isLive(s)) return
  if (action === 'pause' ? s.state !== 'running' : s.state !== 'stopped') return
  const sel = useDebug.getState().selection[s.id]
  const threadId = sel?.threadId ?? s.stopped?.threadId
  try {
    useDebug.getState().upsert(await debugApi.control(s.projectId, s.id, action, threadId))
  } catch (e) {
    if (e instanceof ApiError && e.status === 409) return // raced with a stop or the end
    toastError(e)
  }
}

export async function stopSession(s: DebugSession | null = currentSession()) {
  if (!s) return
  try {
    useDebug.getState().upsert(await debugApi.stop(s.projectId, s.id))
  } catch (e) {
    toastError(e, 'Could not stop the session')
  }
}

export async function rerun(s: DebugSession | null = currentSession()) {
  if (!s) return
  if (!s.config) {
    toast('info', 'Only sessions of a launch configuration can be rerun')
    return
  }
  try {
    const n = await debugApi.restart(s.projectId, s.id)
    useDebug.getState().remove(s.id)
    useDebug.getState().upsert(n)
    useDebug.getState().setActive(s.projectId, n.id)
  } catch (e) {
    toastError(e, 'Could not rerun')
  }
}

export async function forget(s: DebugSession) {
  try {
    await debugApi.forget(s.projectId, s.id)
    useDebug.getState().remove(s.id)
  } catch (e) {
    toastError(e)
  }
}

export async function runToCursor(pid: string, path: string, line: number) {
  const s = currentSession(pid)
  if (!s || s.state !== 'stopped') {
    toast('info', 'Run to Cursor needs a suspended debug session')
    return
  }
  const threadId = useDebug.getState().selection[s.id]?.threadId ?? s.stopped?.threadId
  try {
    useDebug.getState().upsert(await debugApi.runTo(pid, s.id, path, line, threadId))
  } catch (e) {
    toastError(e)
  }
}

// ---------------------------------------------------------------- breakpoints

async function writeFile(pid: string, path: string, list: Partial<LineBreakpoint>[], optimistic?: LineBreakpoint[]) {
  const before = cachedBreakpoints(pid)
  if (before && optimistic) {
    applyBreakpoints(pid, { ...before, breakpoints: [...before.breakpoints.filter((b) => b.path !== path), ...optimistic] })
  }
  try {
    applyBreakpoints(pid, await debugApi.setFile(pid, path, list))
  } catch (e) {
    if (before) applyBreakpoints(pid, before)
    toastError(e, 'Could not save the breakpoint')
  }
}

function optimisticList(path: string, list: Partial<LineBreakpoint>[]): LineBreakpoint[] {
  return list.map((b, i) => ({ id: b.id ?? `new${i}${b.line}`, path, line: b.line ?? 1, enabled: b.enabled ?? true, condition: b.condition, hitCondition: b.hitCondition, logMessage: b.logMessage }))
}

export function breakpointAt(pid: string, path: string, line: number): LineBreakpoint | undefined {
  return cachedBreakpoints(pid)?.breakpoints.find((b) => b.path === path && b.line === line)
}

/** The project's breakpoints (loaded once if nothing showed them yet). */
async function current(pid: string): Promise<LineBreakpoint[] | null> {
  const c = cachedBreakpoints(pid)
  if (c) return c.breakpoints
  try {
    const v = await debugApi.breakpoints(pid)
    applyBreakpoints(pid, v)
    return v.breakpoints
  } catch (e) {
    toastError(e, 'Could not load breakpoints')
    return null
  }
}

export async function toggleBreakpoint(pid: string, path: string, line: number) {
  const all = await current(pid)
  if (!all) return
  const list = toggleLine(all, path, line)
  await writeFile(pid, path, list, optimisticList(path, list))
}

export async function updateBreakpoint(pid: string, path: string, line: number, patch: Partial<LineBreakpoint>) {
  const all = await current(pid)
  if (!all) return
  const list = patchLine(all, path, line, patch)
  await writeFile(pid, path, list, optimisticList(path, list))
}

export async function removeBreakpoint(pid: string, path: string, line: number) {
  const all = await current(pid)
  if (!all) return
  const list = fileBreakpoints(all, path)
    .filter((b) => b.line !== line)
    .map(({ status: _s, ...b }) => b)
  await writeFile(pid, path, list, optimisticList(path, list))
}

/** Breakpoints followed their lines through edits in the editor. */
export async function breakpointsMoved(pid: string, path: string, moved: Map<string, number>) {
  const list = moveLines(cachedBreakpoints(pid)?.breakpoints ?? [], path, moved)
  if (list) await writeFile(pid, path, list, optimisticList(path, list))
}

export async function setMuted(pid: string, muted: boolean) {
  try {
    applyBreakpoints(pid, await debugApi.mute(pid, muted))
  } catch (e) {
    toastError(e)
  }
}

export function viewBreakpoints() {
  showDebug('breakpoints')
}

// ---------------------------------------------------------------- watches

export async function setWatches(pid: string, next: string[]) {
  const before = cachedBreakpoints(pid)
  if (before) applyBreakpoints(pid, { ...before, watches: next })
  try {
    const r = await debugApi.setWatches(pid, next)
    const cur = cachedBreakpoints(pid)
    if (cur) applyBreakpoints(pid, { ...cur, watches: r.watches })
  } catch (e) {
    if (before) applyBreakpoints(pid, before)
    toastError(e, 'Could not save the watches')
  }
}

export async function addWatch(pid: string, expr: string) {
  const e = expr.trim()
  if (!e) return
  const cur = cachedBreakpoints(pid)?.watches ?? (await debugApi.breakpoints(pid).catch(() => null))?.watches ?? []
  if (!cur.includes(e)) await setWatches(pid, [...cur, e])
  showDebug('frames')
}

// ---------------------------------------------------------------- navigation

/** Show a frame's source: a project file in the editor; a file outside the project
 *  (a library header, a crate, the standard library) or source the debugger holds
 *  (`sourceReference`) in the read-only `debug.source` panel of the session. */
export function openFrame(s: Pick<DebugSession, 'id' | 'projectId'>, f: Frame, focus = true) {
  const src = f.source
  const at = { line: Math.max(1, f.line), column: Math.max(1, f.column || 1), t: Date.now() }
  if (src?.path && src.inProject) {
    openPanel({
      kind: 'editor',
      id: `editor:${s.projectId}:${src.path}`,
      title: src.path.split('/').pop() ?? src.path,
      params: { projectId: s.projectId, path: src.path, ...at },
      focus,
    })
    return
  }
  const view = sourcePanel(s, f)
  if (!view) {
    if (focus) toast('info', `${f.name} has no source`)
    return
  }
  openPanel({ ...view, params: { ...view.params, ...at }, focus })
}

/** The `debug.source` panel that shows a frame outside the project (null: no source). */
export function sourcePanel(s: Pick<DebugSession, 'id' | 'projectId'>, f: Frame): { kind: string; id: string; title: string; params: Record<string, unknown> } | null {
  const src = f.source
  // DAP: a `sourceReference` means "ask the debugger", even with a path (debugpy
  // names exec'd code `<generated>`).
  if (src?.sourceReference) {
    const name = src.name ?? src.path ?? `source ${src.sourceReference}`
    return {
      kind: 'debug.source',
      id: `debug.source:${s.projectId}:${s.id}:ref${src.sourceReference}`,
      title: name.split('/').pop() ?? name,
      params: { projectId: s.projectId, sessionId: s.id, sourceReference: src.sourceReference, name },
    }
  }
  if (src?.path?.startsWith('/') && !src.inProject) {
    return {
      kind: 'debug.source',
      id: `debug.source:${s.projectId}:${src.path}`,
      title: src.path.split('/').pop() ?? src.path,
      params: { projectId: s.projectId, sessionId: s.id, path: src.path, name: src.name ?? undefined },
    }
  }
  return null
}

export async function askAgentAboutStop(s: DebugSession) {
  const st = useDebug.getState()
  const stack = st.stacks[s.id]
  const frameIndex = st.selection[s.id]?.frameIndex ?? 0
  const frames = stack?.frames ?? []
  let locals: import('./types').Variable[] = []
  const frame = frames[frameIndex]
  if (frame && s.state === 'stopped') {
    try {
      const { scopes } = await debugApi.scopes(s.projectId, s.id, frame.id)
      for (const sc of scopes.filter((x) => !x.expensive && x.presentationHint !== 'registers' && x.name.toLowerCase() !== 'registers').slice(0, 3)) {
        const { variables } = await debugApi.variables(s.projectId, s.id, sc.variablesReference)
        locals = locals.concat(variables)
      }
    } catch {
      // Ask anyway, with what we have.
    }
  }
  const prompt = agentPrompt({ session: s, frames, frameIndex, locals, console: st.output[s.id] ?? [] })
  void askAgent({ projectId: s.projectId, prompt, submit: false })
}
