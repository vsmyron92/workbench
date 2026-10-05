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
  /** gdb, lldb, codelldb, debugpy, delve or generic. */
  adapterKind: string
  /** A remote target (embedded): the debug server and where gdb connected. */
  remote?: { server?: string; target?: string }
  /** The configuration names an SVD file: the Peripherals tab has a register map. */
  peripherals?: boolean
  /** The Live tab can read variables of the running program: through the debug server's Tcl port (`tcl`: the program is never
   *  stopped), or (`pausing`) by stopping it for a moment, which the user allows per session. */
  live?: boolean
  liveMode?: 'tcl' | 'pausing'

}

// ---------------------------------------------------------------- live watch

export type LiveKind = 'int' | 'uint' | 'float' | 'bool' | 'ptr' | 'enum' | 'bytes'

export interface LiveItem {
  id: number
  expression: string
  /** Absent: it could not be resolved (`error` says why). */
  address?: number
  size: number
  kind: LiveKind
  typeName: string
  /** A peripheral register: reading some of them changes the chip. */
  peripheral?: string
  error?: string
}

/** A number, or a string for what a JSON number cannot hold exactly (64-bit values, bytes). */
export type LiveRaw = number | string | boolean

export interface LiveSample {
  id: number
  /** Milliseconds since the epoch. */
  t: number
  v?: LiveRaw
  e?: string
}

export interface LiveSnapshot {
  items: LiveItem[]
  intervalMs: number
  last: Record<string, LiveSample>
  mode?: 'tcl' | 'pausing'
  /** Reading by pausing the program is allowed for this session. */
  pausing?: boolean
  /** How long a round keeps the program stopped, milliseconds (average), once any round has run. */
  pauseMs?: number | null
}

/** `GET …/live/history`: the readings the server kept, per item id, thinned if asked. `v` is null for a reading that failed
 *  or is no number; `exact` has the digits of the whole numbers a double cannot hold, by index. */
export interface LiveHistory {
  intervalMs: number
  items: LiveItem[]
  series: Record<string, { t: number[]; v: (number | null)[]; exact?: Record<string, string> }>
  now: number
}

/** `debug.live`: the list changed (`items`), the interval changed, or one round of readings. */
export interface LiveEvent {
  sessionId: string
  items?: LiveItem[]
  intervalMs?: number
  pausing?: boolean
  pauseMs?: number
  samples?: LiveSample[]
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
  /** A remote target (embedded): what Workbench starts and sends to it. */
  remote?: RemoteConfig
  problems: string[]
}

export interface RemoteConfig {
  server?: string
  serverLabel?: string
  serverAvailable: boolean
  /** The debug server's command line as it will run (`{port}` unexpanded). */
  commandLine?: string
  connect?: string
  init: string[]
  reset: string[]
  download: boolean
  /** `main`, `reset` or a gdb location. */
  stopAt: string
  /** `target extended-remote`: the stub runs the program, or `attach` names the target. */
  extended: boolean
  attach?: number
  /** Output channels: `name (port)`. */
  channels: string[]
  svd?: string
  /** The project works in its dev container: built there, debugged here. */
  inContainer: boolean
}

export type RegisterAccess = 'read-write' | 'read-only' | 'write-only'

export interface SvdField {
  name: string
  bitOffset: number
  bitWidth: number
  access: RegisterAccess
  description?: string | null
  values: { value: number; name: string; description?: string }[]
  /** Only with the register's value. */
  value?: number | null
  valueName?: string | null
}

export interface SvdRegister {
  name: string
  offset: number
  address: number
  size: number
  access: RegisterAccess
  resetValue?: string | null
  description?: string | null
  readAction: boolean
  /** `0x…`, when the register was read. */
  value?: string | null
  error?: string | null
  /** Why a register was not read: write-only, or reading it changes the chip. */
  skipped?: string | null
  fields: SvdField[]
}

export interface SvdPeripheralSummary {
  name: string
  base: number
  description?: string | null
  group?: string | null
  registers: number
}

export interface SvdList {
  device: string
  description?: string | null
  peripherals: SvdPeripheralSummary[]
}

export interface SvdPeripheral {
  name: string
  base: number
  description?: string | null
  read: boolean
  registers: SvdRegister[]
}

export interface ServerView {
  id: string
  label: string
  command: string
  args: string[]
  enabled: boolean
  builtin: boolean
  init: string[]
  reset: string[]
  download: boolean
  installHint: string
  availability: { available: boolean; path?: string; version?: string; problem?: string }
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
