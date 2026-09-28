// Shapes of the debug slice's REST answers and events (server/src/debug).

export type SessionState = 'starting' | 'running' | 'stopped' | 'terminated' | 'failed'

export interface StopInfo {
  reason: string
  description?: string
  text?: string
  threadId?: number
  allThreadsStopped: boolean
  /** Our breakpoint ids the adapter says were hit. */
  hitBreakpointIds?: string[]
  /** The program stopped while evaluating this expression (a watch calling a function). */
  duringEvaluation?: string
  at: number
}

export interface ExceptionFilter {
  filter: string
  label: string
  description?: string
  default?: boolean
}

export interface Capabilities {
  supportsFunctionBreakpoints?: boolean
  supportsConditionalBreakpoints?: boolean
  supportsHitConditionalBreakpoints?: boolean
  supportsLogPoints?: boolean
  supportsSetVariable?: boolean
  supportsSetExpression?: boolean
  supportsCompletionsRequest?: boolean
  completionTriggerCharacters?: string[]
  supportsTerminateRequest?: boolean
  supportsEvaluateForHovers?: boolean
  exceptionBreakpointFilters?: ExceptionFilter[]
}

export interface DebugSession {
  id: string
  projectId: string
  name: string
  config?: string
  adapter: string
  adapterLabel: string
  request: 'launch' | 'attach'
  state: SessionState
  phase?: string
  error?: string
  stopped?: StopInfo
  /** Frame ids and variable references belong to one epoch. */
  stopEpoch: number
  threads: { id: number; name: string }[]
  exitCode?: number
  process?: { pid?: number; name: string }
  capabilities: Capabilities
  startedAt: number
  endedAt?: number
  inContainer: boolean
  parentId?: string
  prelaunchTerminalId?: string
  debuggeeTerminalId?: string
  outputSeq: number
  /** The user stopped it (Stop, Rerun): "Stopped" or "Detached", not an exit code. */
  stopRequested?: boolean
}

export interface OutputLine {
  seq: number
  category: string
  text: string
  at: number
  path?: string
  line?: number
}

export interface SourceRef {
  name?: string | null
  /** Project-relative when `inProject`, else absolute. */
  path?: string
  inProject: boolean
  sourceReference?: number | null
}

export interface Frame {
  id: number
  name: string
  line: number
  column: number
  source?: SourceRef | null
  presentationHint?: string | null
}

export interface Scope {
  name: string
  variablesReference: number
  expensive: boolean
  presentationHint?: string | null
  namedVariables?: number | null
  indexedVariables?: number | null
}

export interface Variable {
  name: string | null
  value: string
  type?: string | null
  variablesReference: number
  namedVariables?: number | null
  indexedVariables?: number | null
  evaluateName?: string | null
  presentationHint?: { kind?: string; attributes?: string[] } | null
}

export interface BpStatus {
  verified: boolean
  line?: number
  message?: string
}

export interface LineBreakpoint {
  id: string
  path: string
  line: number
  enabled: boolean
  condition?: string
  hitCondition?: string
  logMessage?: string
  status?: BpStatus | null
}

export interface FunctionBreakpoint {
  id: string
  name: string
  enabled: boolean
  condition?: string
  status?: BpStatus | null
}

export interface BreakpointsView {
  breakpoints: LineBreakpoint[]
  functionBreakpoints: FunctionBreakpoint[]
  exceptionFilters: { adapter: string; label: string; filters: ExceptionFilter[]; enabled: string[] }[]
  muted: boolean
  watches: string[]
  lastConfig?: string | null
  live: boolean
}

export interface LaunchConfig {
  name: string
  origin: 'config' | 'cargo' | 'cmake' | 'python' | 'go'
  source?: string
  request: 'launch' | 'attach'
  adapter?: string
  adapterLabel?: string
  adapterAvailable: boolean
  language: string
  program?: string
  module?: string
  args: string[]
  cwd?: string
  preLaunch?: string
  stopOnEntry: boolean
  problems: string[]
}

export interface AdapterView {
  id: string
  kind: string
  label: string
  command: string
  args: string[]
  languages: string[]
  transport: 'stdio' | 'tcp'
  enabled: boolean
  builtin: boolean
  adapterId: string
  installHint: string
  availability: { available: boolean; path?: string; version?: string; problem?: string }
}

export interface ProcessInfo {
  pid: number
  ppid: number
  name: string
  command: string
  startedAt?: number
  language: string
}

export interface ProcessList {
  processes: ProcessInfo[]
  truncated: boolean
  ptraceScope?: number
  ptraceHint?: string
}

export interface CompletionItem {
  label: string
  text?: string
  start?: number
  length?: number
  type?: string
}
