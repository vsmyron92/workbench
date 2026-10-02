// REST access for the debug feature (/api/projects/{pid}/debug/**) and the shared
// query client, so palette commands and the editor integration can read caches.

import { useQuery, type QueryClient } from '@tanstack/react-query'
import { api } from '@/api/client'
import type { AdapterView, BreakpointsView, CompletionItem, DebugSession, Frame, FunctionBreakpoint, LaunchConfig, LineBreakpoint, OutputLine, ProcessList, Scope, ServerView, SvdList, SvdPeripheral, SvdRegister, Variable } from './types'

const enc = encodeURIComponent
export const base = (pid: string) => `/api/projects/${enc(pid)}/debug`
const sbase = (pid: string, sid: string) => `${base(pid)}/sessions/${enc(sid)}`

export const debugKeys = {
  configs: (pid: string) => ['debug', 'configs', pid] as const,
  adapters: (pid: string) => ['debug', 'adapters', pid] as const,
  servers: (pid: string) => ['debug', 'servers', pid] as const,
  breakpoints: (pid: string) => ['debug', 'breakpoints', pid] as const,
  sessions: (pid: string) => ['debug', 'sessions', pid] as const,
  processes: (pid: string) => ['debug', 'processes', pid] as const,
  /** Everything read from a suspended session: refetched per stop epoch. */
  session: (sid: string) => ['debug', 'session', sid] as const,
}

let client: QueryClient | null = null
export function setDebugQueryClient(qc: QueryClient | null) {
  client = qc
}
export function queryClient() {
  return client
}

export function cachedBreakpoints(pid: string | null): BreakpointsView | undefined {
  return pid ? client?.getQueryData<BreakpointsView>(debugKeys.breakpoints(pid)) : undefined
}
export function cachedConfigs(pid: string | null): { configs: LaunchConfig[]; lastConfig?: string | null } | undefined {
  return pid ? client?.getQueryData(debugKeys.configs(pid)) : undefined
}

export function useConfigs(pid: string | null) {
  return useQuery({
    queryKey: debugKeys.configs(pid ?? ''),
    queryFn: ({ signal }) => api.get<{ configs: LaunchConfig[]; lastConfig?: string | null }>(`${base(pid!)}/configs`, undefined, signal),
    enabled: !!pid,
    staleTime: 30_000,
  })
}

export function useAdapters(pid: string | null, enabled = true) {
  return useQuery({
    queryKey: debugKeys.adapters(pid ?? ''),
    queryFn: ({ signal }) => api.get<{ adapters: AdapterView[]; warnings: string[] }>(`${base(pid!)}/adapters`, undefined, signal),
    enabled: !!pid && enabled,
    staleTime: 30_000,
  })
}

export function useServers(pid: string | null, enabled = true) {
  return useQuery({
    queryKey: debugKeys.servers(pid ?? ''),
    queryFn: ({ signal }) => api.get<{ servers: ServerView[]; warnings: string[] }>(`${base(pid!)}/servers`, undefined, signal),
    enabled: !!pid && enabled,
    staleTime: 30_000,
  })
}

export function useBreakpoints(pid: string | null) {
  return useQuery({
    queryKey: debugKeys.breakpoints(pid ?? ''),
    queryFn: ({ signal }) => api.get<BreakpointsView>(`${base(pid!)}/breakpoints`, undefined, signal),
    enabled: !!pid,
    staleTime: 60_000,
  })
}

export function useProcesses(pid: string | null, enabled: boolean) {
  return useQuery({
    queryKey: debugKeys.processes(pid ?? ''),
    queryFn: ({ signal }) => api.get<ProcessList>(`${base(pid!)}/processes`, undefined, signal),
    enabled: !!pid && enabled,
    staleTime: 0,
  })
}

export const debugApi = {
  sessions: (pid: string, signal?: AbortSignal) => api.get<DebugSession[]>(`${base(pid)}/sessions`, undefined, signal),
  /** `processId`: the process an attach configuration without a pid attaches to. */
  start: (pid: string, config: string, stopOnEntry?: boolean, processId?: number) =>
    api.post<DebugSession>(`${base(pid)}/sessions`, { config, stopOnEntry, pid: processId }),
  attach: (pid: string, body: { pid: number; adapter?: string; language?: string; program?: string }) => api.post<DebugSession>(`${base(pid)}/sessions/attach`, body),
  stop: (pid: string, sid: string) => api.post<DebugSession>(`${sbase(pid, sid)}/stop`),
  restart: (pid: string, sid: string) => api.post<DebugSession>(`${sbase(pid, sid)}/restart`),
  forget: (pid: string, sid: string) => api.del<{ ok: boolean }>(sbase(pid, sid)),
  control: (pid: string, sid: string, action: string, threadId?: number) => api.post<DebugSession>(`${sbase(pid, sid)}/control`, { action, threadId }),
  runTo: (pid: string, sid: string, path: string, line: number, threadId?: number) => api.post<DebugSession>(`${sbase(pid, sid)}/run-to`, { path, line, threadId }),
  stack: (pid: string, sid: string, threadId: number, levels = 60, signal?: AbortSignal) =>
    api.get<{ frames: Frame[]; totalFrames?: number | null }>(`${sbase(pid, sid)}/stack`, { threadId, levels }, signal),
  scopes: (pid: string, sid: string, frameId: number, signal?: AbortSignal) => api.get<{ scopes: Scope[] }>(`${sbase(pid, sid)}/scopes`, { frameId }, signal),
  variables: (pid: string, sid: string, ref: number, page?: { start: number; count: number }, signal?: AbortSignal) =>
    api.get<{ variables: Variable[]; truncated: boolean }>(`${sbase(pid, sid)}/variables`, { ref, ...page }, signal),
  evaluate: (pid: string, sid: string, expression: string, context: 'watch' | 'repl' | 'hover' | 'clipboard', frameId?: number) =>
    api.post<Variable>(`${sbase(pid, sid)}/evaluate`, { expression, context, frameId }),
  setVariable: (pid: string, sid: string, variablesReference: number, name: string, value: string) =>
    api.post<Variable>(`${sbase(pid, sid)}/set-variable`, { variablesReference, name, value }),
  /** The chip's register map (the configuration's `svd`). */
  svd: (pid: string, sid: string, signal?: AbortSignal) => api.get<SvdList>(`${sbase(pid, sid)}/svd`, undefined, signal),
  /** A peripheral's registers; `read` reads them (the program must be suspended), `force` names
   *  registers that are read although reading them changes the chip. */
  svdPeripheral: (pid: string, sid: string, name: string, read: boolean, force?: string[], signal?: AbortSignal) =>
    api.get<SvdPeripheral>(`${sbase(pid, sid)}/svd/${enc(name)}`, { read, registers: force?.length ? force.join(',') : undefined }, signal),
  /** Write a register, or one field (a number, or the field's value name). */
  svdWrite: (pid: string, sid: string, peripheral: string, register: string, body: { value: string | number; field?: string }) =>
    api.put<SvdRegister>(`${sbase(pid, sid)}/svd/${enc(peripheral)}/${enc(register)}`, body),
  completions: (pid: string, sid: string, text: string, column: number, frameId?: number) =>
    api.post<{ targets: CompletionItem[] }>(`${sbase(pid, sid)}/completions`, { text, column, frameId }),
  output: (pid: string, sid: string, after = 0) => api.get<{ lines: OutputLine[]; dropped: boolean; seq: number }>(`${sbase(pid, sid)}/output`, { after }),
  source: (pid: string, sid: string, ref: number) => api.get<{ content: string; mimeType?: string }>(`${sbase(pid, sid)}/source`, { ref }),
  /** A file outside the project that the session's frames or output named. */
  file: (pid: string, sid: string, path: string) => api.get<{ path: string; content: string }>(`${sbase(pid, sid)}/file`, { path }),
  breakpoints: (pid: string) => api.get<BreakpointsView>(`${base(pid)}/breakpoints`),
  setFile: (pid: string, path: string, breakpoints: Partial<LineBreakpoint>[]) => api.put<BreakpointsView>(`${base(pid)}/breakpoints/file`, { path, breakpoints }),
  setFunctions: (pid: string, breakpoints: Partial<FunctionBreakpoint>[]) => api.put<BreakpointsView>(`${base(pid)}/breakpoints/functions`, { breakpoints }),
  setExceptions: (pid: string, adapter: string, filters: string[]) => api.put<BreakpointsView>(`${base(pid)}/breakpoints/exceptions`, { adapter, filters }),
  mute: (pid: string, muted: boolean) => api.put<BreakpointsView>(`${base(pid)}/breakpoints/mute`, { muted }),
  clear: (pid: string) => api.post<BreakpointsView>(`${base(pid)}/breakpoints/clear`),
  setWatches: (pid: string, expressions: string[]) => api.put<{ watches: string[] }>(`${base(pid)}/watches`, { expressions }),
}

const turns = new Map<string, Promise<unknown>>()

/** Run `fn` once the previous call with the same `key` has settled. A session's
 *  watches are evaluated one at a time: a watch that calls a function can stop the
 *  program, and that stop must be blamed on it (and not run into by the next ones). */
export function inTurn<T>(key: string, fn: () => Promise<T>): Promise<T> {
  const prev = turns.get(key) ?? Promise.resolve()
  const run = prev.then(fn, fn)
  const settled = run.then(
    () => undefined,
    () => undefined,
  )
  turns.set(key, settled)
  void settled.then(() => {
    if (turns.get(key) === settled) turns.delete(key)
  })
  return run
}

/** Store a fresh breakpoints view (answers of every breakpoint write). */
export function applyBreakpoints(pid: string, v: BreakpointsView) {
  client?.setQueryData(debugKeys.breakpoints(pid), v)
}
