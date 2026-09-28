// Shapes of server/src/devcontainer (serde camelCase).

export type DcState = 'none' | 'stopped' | 'running' | 'building' | 'error'

/** ProjectSummary.devcontainer */
export interface DcSummary {
  configs: string[]
  state: DcState
  inContainer: boolean
}

export type RiskLevel = 'danger' | 'warning' | 'info'

export interface Risk {
  level: RiskLevel
  item: string
  message: string
}

export type Cmd = { kind: 'shell'; value: string } | { kind: 'exec'; value: string[] }

export interface Lifecycle {
  commands: [string | null, Cmd][]
}

export interface Mount {
  type: string
  source: string
  target: string
  readonly: boolean
  spec: string
}

export interface PortSpec {
  host?: string
  hostPort?: number
  port: number
  label?: string
}

export type Source =
  | { kind: 'image'; image: string }
  | { kind: 'dockerfile'; dockerfile: string; context: string; args: Record<string, string>; target?: string; cacheFrom: string[]; options: string[] }
  | { kind: 'compose'; files: string[]; service: string; runServices: string[] }
  | { kind: 'none' }

export interface DevConfig {
  path: string
  name?: string
  source: Source
  workspaceFolder: string
  workspaceMount?: Mount
  mounts: Mount[]
  runArgs: string[]
  containerEnv: Record<string, string>
  remoteEnv: Record<string, string | null>
  remoteUser?: string
  containerUser?: string
  updateRemoteUserUid: boolean
  forwardPorts: PortSpec[]
  appPorts: PortSpec[]
  overrideCommand: boolean
  shutdownAction: string
  init: boolean
  privileged: boolean
  capAdd: string[]
  securityOpt: string[]
  features: Record<string, unknown>
  localEnv: string[]
  notes: string[]
}

export interface Hook {
  key: string
  when: string
  commands: string[]
}

export interface Plan {
  config: DevConfig
  engine: 'docker' | 'cli' | null
  engineNote: string
  problems: string[]
  risks: Risk[]
  hooks: Hook[]
  ports: number[]
  files: string[]
  hash: string
}

export interface Engines {
  docker: string | null
  dockerError?: string
  compose: string | null
  cli: string | null
  preference: string
}

export interface ContainerView {
  id: string
  name: string
  status: string
  image: string
  ports: { port: number; proto: string; hostIp?: string; hostPort?: number }[]
  ip?: string
  created: string
  managed: boolean
  configFile?: string
  compose?: string
}

export interface PortView {
  port: number
  label: string | null
  url: string
  via: 'published' | 'container-ip' | 'host-network'
  hostPort: number | null
}

/** GET /api/projects/{pid}/devcontainer */
export interface DcView {
  projectId: string
  configs: string[]
  config: string | null
  plan: Plan | null
  planError: string | null
  engines: Engines
  state: DcState
  container: ContainerView | null
  containers: number
  approved: boolean
  useContainer: boolean
  inContainer: boolean
  operation: { kind: string; terminalId: string | null; startedAt: number } | null
  error: string | null
  hostRuns: string[]
  remoteUser: string | null
  workspaceFolder: string | null
  ports: PortView[]
  /** Agent CLI name → its path in the container, or null. */
  agents: Record<string, string | null>
  bridge: string | null
}

/** GET …/devcontainer/scaffold */
export interface Proposal {
  path: string
  content: string
  stacks: string[]
  notes: string[]
  exists: boolean
}
