// Types shared across features. Feature-specific types live in their feature
// folder (features/<slice>/types.ts). Keep these in sync with the Rust structs
// named in the comments (serde camelCase).

/** projects.rs ProjectSummary */
export interface ProjectSummary {
  id: string
  name: string
  root: string
  rootAbs: string
  tags: string[]
  docs: string[]
  branch: string | null
  gitlab: { host: string; path: string } | null
  github: { host: string; path: string } | null
  hasConfluence: boolean
  hasJira: boolean
  runs: number
  envs: string[]
  warnings: string[]
  /** The dev container: null without a devcontainer.json or container (devcontainer slice). */
  devcontainer: {
    configs: string[]
    state: 'none' | 'stopped' | 'running' | 'building' | 'error'
    /** Running, and new shells and runs go into it. */
    inContainer: boolean
  } | null
  /** The git repositories, the default one first (docs/ARCHITECTURE.md "Repositories of a project"); `[]` for a folder in none. */
  repos?: RepoSummary[]
}

/** projects.rs RepoSummary: one git repository of a project. */
export interface RepoSummary {
  /** `.` for the repository holding the project root, else its project-relative directory (`services/api`). */
  id: string
  name: string
  /** The working tree relative to the project root; empty for `.`. */
  path: string
  default: boolean
  remote: string | null
  gitlab: { host: string; path: string } | null
  github: { host: string; path: string } | null
}

/** GET /api/projects/{pid} */
export interface ProjectDetail {
  summary: ProjectSummary
  /** config/project.rs ProjectFile (snake_case keys, as in TOML) */
  config: ProjectConfig
  remote: { url: string; host: string; path: string } | null
}

export interface ProjectConfig {
  schema: number
  project: { id: string; name: string; root: string; tags?: string[]; docs?: string[]; sensitive?: string[] }
  repo?: { remote: string; default_branch?: string; gitlab?: { host: string; path: string; project_id?: number; token: string; registry?: string } }
  run?: RunConfig[]
  env?: EnvironmentConfig[]
  hosts?: Record<string, { host: string; user: string; port: number; identity_file?: string }>
  links?: {
    confluence?: { site: string; space: string; root_pages?: number[]; pinned?: Record<string, number>; archived?: boolean }
    jira?: { site: string; project_keys: string[]; jql?: string }
    url?: { name: string; url: string }[]
  }
  agent?: {
    model?: string
    effort?: string
    permission_mode?: string
    remote_control?: boolean
    add_dirs?: string[]
    env?: Record<string, string>
    starter?: { name: string; command: string }[]
  }
  secrets?: Record<string, unknown>
  toolchains?: Record<string, string>
}

export type RunKind = 'server' | 'task' | 'test' | 'build' | 'service' | 'editor'

export interface RunConfig {
  name: string
  kind: RunKind
  command: string
  cwd?: string
  env?: Record<string, string>
  port?: number
  free_port?: boolean
  ready?: { log?: string; http?: string; timeout_s: number }
  depends_on?: string[]
  preview?: string
  group?: string
  source?: string
}

export interface EnvironmentConfig {
  name: string
  kind: 'production' | 'staging' | 'preview' | 'development'
  url: string
  host?: string
  health?: { url: string; expect_status: number; json_pointer?: string; equals?: string; interval_s: number }
  logs?: { name: string; command: string }[]
  deploy?: { command: string; confirm: 'none' | 'click' | 'typed'; require_green_pipeline?: boolean; only_ref?: string; after?: string }
  command?: { name: string; command: string; confirm?: boolean }[]
}

/** terminals/mod.rs */
export type TerminalKind = 'agent' | 'shell' | 'run' | 'command'
export type TerminalStatus = 'starting' | 'running' | 'exited'
export type AgentState = 'starting' | 'idle' | 'working' | 'needs_permission' | 'needs_input' | 'error' | 'exited'

export interface ExitInfo {
  code: number | null
  signal: string | null
  at: number
  /** Ended from outside (closed or killed from Workbench; on Unix a hang-up, terminate, kill or interrupt signal), not by its own exit. Absent when false. */
  terminated?: boolean
}

/** terminals/providers.rs ProviderKind: which kind of agent CLI runs a session. */
export type AgentProvider = 'claude' | 'codex' | 'kimi' | 'gemini' | 'aider' | 'custom'

/**
 * terminals/permission.rs PendingPermission: the Claude Code permission request a session
 * waits on, answerable with `POST /api/agents/{terminalId}/permission {id, decision:
 * 'allow' | 'deny', message?, interrupt?, scope?: 'once' | 'session'}` (devices only; 409
 * once it is no longer pending).
 */
export interface PendingPermission {
  id: string
  /** The tool: `Bash`, `Edit`, `mcp__server__tool`… */
  tool: string
  /** What it asks for on one line, masked and cut ("Permission to run `npm test`"). Never enough to approve on. */
  summary: string
  /** When it was asked (ms). */
  since: number
  /** What "Allow for this session" allows (Claude's suggested rule, whole); null: only once. */
  sessionRule?: string | null
  /**
   * The whole request as text on its real lines (the command, the URL, the edit, an MCP
   * tool's arguments as JSON), at most 16 KB, masked only for known secrets and
   * credential shapes; invisible characters shown as `⟨U+…⟩`.
   */
  detail?: string
  /** `detail` and `sessionRule` are whole: nothing cut or masked. One-tap Allow only then. */
  complete?: boolean
}

export interface AgentInfo {
  /** The CLI's conversation id; '' while unknown (Codex/Kimi before discovery, Aider, custom CLIs). */
  sessionId: string
  provider: AgentProvider
  /** `[agents.providers.<id>]` of the session (`null`: claude, a record from before providers). */
  providerId: string | null
  state: AgentState
  unread: boolean
  model: string | null
  effort: string | null
  permissionMode: string | null
  remoteControl: boolean
  remoteUrl: string | null
  title: string | null
  lastMessage: string | null
  attention: string | null
  contextPct: number | null
  costUsd: number | null
  lastEventAt: number
  /** The permission request Workbench can answer (Claude Code), or null. */
  pendingPermission: PendingPermission | null
}

export interface TerminalInfo {
  id: string
  kind: TerminalKind
  title: string
  projectId: string | null
  cwd: string
  argv: string[]
  status: TerminalStatus
  exit: ExitInfo | null
  createdAt: number
  lastOutputAt: number
  cols: number
  rows: number
  open: boolean
  pinned: boolean
  color: string | null
  order: number
  agent: AgentInfo | null
  meta: Record<string, unknown>
  /** Processes still running in the terminal's session after its main process exited. */
  lingering?: number
}

/** events.rs Event */
export interface WbEvent<T = unknown> {
  type: string
  projectId?: string
  data: T
  ts: number
}
