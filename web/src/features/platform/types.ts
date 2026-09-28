// Types for the platform slice's REST API (server/src/platform). Config types
// mirror config.toml (snake_case keys, as serde writes them).

export type SecretRef =
  | { file: string }
  | { env: string }
  | { keyring: string }
  | { dotenv: { path: string; key: string } }
  | { command: string[] }

/** config/global.rs GlobalConfig */
export interface GlobalConfig {
  server: {
    bind: string
    allowed_hosts: string[]
    public_url?: string | null
    tls?: { cert: string; key: string } | null
  }
  projects: { roots: string[]; include: string[]; exclude: string[] }
  agents: {
    command: string
    model?: string | null
    effort?: string | null
    permission_mode?: string | null
    remote_control: boolean
    restore_on_start: boolean
    statusline: boolean
  }
  gitlab?: { host: string; token: string } | null
  /** `token` empty: public repositories only, read-only. */
  github?: { host: string; token: string } | null
  atlassian?: { site: string; email: string; token: string } | null
  notify?: { desktop: boolean; command?: string | null }
  /** Web Push: the VAPID contact and extra push service hosts. */
  push?: { subject?: string | null; extra_endpoint_hosts?: string[] }
  extra_roots: string[]
  secrets: Record<string, SecretRef>
}

/** GET /api/settings */
export interface SettingsInfo {
  config: GlobalConfig
  hash: string
  paths: { configDir: string; configFile: string; dataDir: string; projectsDir: string }
  version: string
  bind: string
  startedAt: number
  restartRequired: string[]
  tlsActive: boolean
  notifySend: boolean
  mcpEndpoint: string
}

/** settings.rs ApplyResult */
export interface ApplyResult {
  ok: boolean
  hash: string
  restartRequired: string[]
  warnings: string[]
  projects: number
}

/** settings.rs Diagnostic (POST /api/settings/validate) */
export interface Diagnostic {
  ok: boolean
  message?: string
  line?: number
  column?: number
  endLine?: number
  endColumn?: number
  warnings: string[]
}

export interface RawConfig {
  text: string
  hash: string
  path: string
  exists: boolean
}

/** settings.rs SecretRow (secrets.rs SecretStatus + usage) */
export interface SecretRow {
  name: string
  source: 'file' | 'env' | 'keyring' | 'dotenv' | 'command'
  location: string
  resolved: boolean
  error?: string
  warnings: string[]
  projectId?: string
  scope: 'global' | 'project'
  usedBy: string[]
  fixable: boolean
  mode?: string
}

export interface MissingSecret {
  name: string
  projectId: string | null
  usedBy: string[]
}

export interface SecretsInfo {
  secrets: SecretRow[]
  missing: MissingSecret[]
}

/** GET /api/settings/projects/{pid} */
export interface ProjectSettings {
  projectId: string
  name: string
  root: string
  repoPath: string
  overlayPath: string
  detectedToml: string
  repoHash: string
  repoFile: string | null
  overlayHash: string
  overlayFile: string | null
  mergedToml: string
  warnings: string[]
}

export interface LayerSaveResult {
  ok: boolean
  hash: string
  warnings: string[]
  projectWarnings: string[]
}

/** auth.rs DeviceInfo */
export interface DeviceInfo {
  id: string
  name: string
  createdAt: number
  lastSeenAt: number
  remote: boolean
  userAgent: string
  current: boolean
}

export interface RemoteAddress {
  interface: string
  address: string
  family: 4 | 6
  kind: 'lan' | 'tailscale' | 'virtual'
  url: string
  listening: boolean
  hostAllowed: boolean
}

/** GET /api/platform/remote */
export interface RemoteInfo {
  bind: string
  configuredBind: string
  port: number
  loopbackOnly: boolean
  exposed: boolean
  addresses: RemoteAddress[]
  allowedHosts: string[]
  publicUrl: string | null
  tls: { configured: boolean; active: boolean; cert?: string; key?: string; certExists?: boolean; keyExists?: boolean }
  devices: DeviceInfo[]
  remoteDevices: number
  restartRequired: string[]
  requestHost: string
  secure: boolean
}

export interface PairCandidate {
  url: string
  kind: 'public' | 'request' | 'tailscale' | 'lan' | 'loopback'
  label: string
  reachable: boolean
  hostAllowed: boolean
}

/** POST /api/platform/pair */
export interface PairResult {
  code: string
  url: string
  expiresAt: number
  ttlMs: number
  qrSvg: string
  baseUrl: string
  candidates: PairCandidate[]
  warning: string | null
}

/** activity.rs McpCallRecord (event `mcp.call`) */
export interface McpCall {
  id: number
  at: number
  terminalId: string | null
  projectId: string | null
  session: string | null
  tool: string
  ok: boolean
  mutating: boolean
  ms: number
  summary: string
  error: string | null
}

/** activity.rs ActivityRecord (event `platform.activity`) */
export interface ActivityEvent {
  id: number
  at: number
  kind: 'attention' | 'env' | 'deploy' | 'pipeline' | 'notify' | string
  level: 'info' | 'success' | 'warning' | 'error'
  projectId: string | null
  terminalId: string | null
  title: string
  message: string
}

export interface ActivityFeed {
  calls: McpCall[]
  events: ActivityEvent[]
}

export interface ToolSummary {
  name: string
  description: string
  mutating: boolean
}

export interface ClaudeMcpServer {
  name: string
  scope: 'user' | 'local' | 'project' | 'plugin' | 'managed'
  source: string
  plugin?: string
  transport: string
  endpoint?: string
  command?: string
  args: number
  envNames: string[]
  headerNames: string[]
  status: 'enabled' | 'disabled' | 'needs-approval' | 'denied'
  reason?: string
}

/** GET /api/platform/mcp-servers */
export interface McpOverview {
  servers: ClaudeMcpServer[]
  files: { path: string; status: 'read' | 'missing' | 'error'; error?: string }[]
  accountConnectorsUsed: boolean
  workbench: { name: string; endpoint: string; note: string; tools: ToolSummary[] }
  notes: string[]
}

export interface NotifyOutcome {
  desktop: 'sent' | 'disabled' | 'unavailable' | 'rate-limited'
  command: 'ran' | 'none' | 'rate-limited'
  push?: 'queued' | 'none' | 'off' | 'full' | 'rate-limited' | 'n/a'
}

/** push/mod.rs Topics: what a device wants pushed. */
export interface PushTopics {
  attention: boolean
  done: boolean
  env: boolean
  deploy: boolean
  pipeline: boolean
  notify: boolean
}

/** push/mod.rs SubInfo */
export interface PushSubscriptionInfo {
  id: string
  sessionId: string
  device: string
  /** "Google (FCM)", "Apple", "Mozilla", "Microsoft (WNS)" or the host. */
  service: string
  createdAt: number
  topics: PushTopics
  quietWhenActive: boolean
  lastOkAt: number | null
  lastError: string | null
  lastErrorAt: number | null
  current: boolean
  endpointHash: string
}

/** GET /api/push */
export interface PushInfo {
  publicKey: string
  subscriptions: PushSubscriptionInfo[]
  sessionId: string | null
}

/** POST /api/push/test */
export interface PushDelivery {
  subscriptionId: string
  device: string
  outcome: 'sent' | 'gone' | 'failed' | 'skipped' | 'superseded'
  status?: number
  error?: string
}
