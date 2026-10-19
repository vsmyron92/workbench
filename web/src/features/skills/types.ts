// Wire types of the skills slice (server/src/skills): what the agent CLIs can load as skills here,
// and the optional skills Workbench offers. See docs/ARCHITECTURE.md ("Agent skills").

/** The agent CLIs that read skills. */
export type Cli = 'claude' | 'codex' | 'gemini' | 'kimi'

export type Scope = 'project' | 'nested' | 'user' | 'plugin' | 'synced' | 'managed' | 'system' | 'extra' | 'builtin'

export type SkillKind = 'skill' | 'command' | 'builtin'

/** `active`; `name-only` (listed without its description); `disabled` (a setting or a plugin is off); `shadowed` (another skill of the name wins). */
export type SkillStatus = 'active' | 'name-only' | 'disabled' | 'shadowed'

export interface Issue {
  level: 'error' | 'warn' | 'info'
  code: string
  message: string
}

export interface SkillFlags {
  /** Only the user starts it. */
  manualOnly: boolean
  /** Only the model uses it. */
  modelOnly: boolean
  /** Tools it pre-approves while it runs. */
  preapproves: string[]
  hooks: boolean
  /** The sub-agent it runs in (`context: fork`). */
  forks: string | null
  /** Commands run when it is invoked (Claude Code's shell injection). */
  shell: boolean
  scripts: boolean
  paths: string[]
  model: string | null
  argumentHint: string | null
}

export interface SkillInfo {
  id: string
  name: string
  /** What the agent lists it as: `plugin:name`, `anthropic-skills:name`, `sub/dir:name`. */
  qualified: string
  kind: SkillKind
  scope: Scope
  /** The plugin, or the sub-folder of a nested skill. */
  scopeLabel?: string
  description: string
  /** The description is the first line of the text: the file has none. */
  descriptionFromText: boolean
  whenToUse?: string
  agents: Cli[]
  /** Ids of the accounts that read it. */
  accounts: string[]
  /** Its folder (a command's file), with `~`. Empty for built-in skills. */
  path: string
  /** Its file relative to the project, when it lies inside it. */
  projectFile?: string
  invoke: { agent: Cli; text: string }[]
  /** What its listing line costs every session (tokens, estimated). */
  tokens: number
  bytes: number
  lines: number
  /** Files in its folder besides `SKILL.md`. */
  files: number
  flags: SkillFlags
  status: SkillStatus
  statusReason?: string
  /** Whether the newest session's listing had it (absent: nothing to compare with). */
  inSession?: boolean
  /** The Workbench skill this was installed as. */
  workbench?: string
  issues: Issue[]
}

export interface SessionNote {
  count: number
  chars: number
  tokens: number
  atMs: number | null
  version: string | null
}

/** An agent account whose CLI reads skills. */
export interface AgentReport {
  id: string
  cli: Cli
  label: string
  installed: boolean
  enabled: boolean
  /** Its sessions get Workbench's MCP tools. */
  mcp: boolean
  /** The account's folder, with `~`. */
  folder: string
  /** Skills its CLI can load here. */
  skills: number
  /** What their listing lines cost every session (tokens, estimated from the files). */
  listingTokens: number
  /** What the newest session in this project received (Claude Code). */
  session?: SessionNote
}

/** `available`: not installed; `installed`: and current; `update`: from an older Workbench; `modified`: edited since; `foreign`: someone else's folder of that name. */
export type InstallState = 'available' | 'installed' | 'update' | 'modified' | 'foreign'

export interface CatalogEntry {
  name: string
  description: string
  files: number
  bytes: number
  lines: number
  installs: { account: string; state: InstallState; path: string }[]
}

/** GET /api/projects/{pid}/skills */
export interface SkillsReport {
  projectId: string
  generatedAt: number
  agents: AgentReport[]
  skills: SkillInfo[]
  catalog: CatalogEntry[]
  notes: string[]
}

export interface SkillFileEntry {
  path: string
  size: number
  dir: boolean
}

/** GET /api/projects/{pid}/skills/{id}: a found skill (`skill`) or one of Workbench's (`catalog`, id `wb:<name>`). */
export interface SkillDetail {
  skill?: SkillInfo
  catalog?: CatalogEntry | null
  /** The skill's `SKILL.md`; null for a built-in skill (it has no file). */
  text: string | null
  truncated: boolean
  files: SkillFileEntry[]
  filesTruncated: boolean
}

/** GET …/skills/{id}/file?path= */
export interface SkillFile {
  path: string
  text: string | null
  truncated: boolean
  binary: boolean
}
