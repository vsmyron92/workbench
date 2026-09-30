// Types of the apps slice REST API (server/src/apps). camelCase, as serialized by serde.

import type { ExitInfo, RunKind } from '@/api/types'

export type RunState = 'stopped' | 'starting' | 'running' | 'ready' | 'failed' | 'exited'

export interface ResultItem {
  name: string
  status: 'passed' | 'failed' | 'skipped'
  detail?: string
}

export interface TestResult {
  passed: number
  failed: number
  items?: ResultItem[]
  truncated?: boolean
}

/** Live part of a run (also the payload of `run.state`, plus `name`). */
export interface RunLive {
  state: RunState
  terminalId?: string
  port?: number
  url?: string
  startedAt?: number
  readyAt?: number
  exit?: ExitInfo
  result?: TestResult
  error?: string
  /** Exited because it was cut short (its terminal closed or its process killed), not because it finished. Absent when false. */
  terminated?: boolean
  phase?: string
  /** The process runs in the project's dev container (in GET …/runs: or a start would put it there). */
  inContainer?: boolean
  /** How this computer reaches its port there: the published 127.0.0.1 port, or the container's address. */
  reach?: 'published' | 'container-ip' | 'host-network'
}

export interface RunConfigView {
  kind: RunKind
  command: string
  cwd: string
  port?: number
  freePort: boolean
  ready?: { log?: string | null; http?: string | null; timeoutS: number }
  dependsOn: string[]
  preview?: string
  group?: string
  source?: string
  resultPattern?: string
  env: Record<string, string>
  hasStop: boolean
  hasStatus: boolean
}

export interface RunView extends RunLive {
  name: string
  config: RunConfigView
  portInUse: boolean
  problems: string[]
  /** It may deploy, release or reach a remote host: starting it asks first; agents cannot start it. */
  needsConfirm?: boolean
  /** Pinned to the host although the project runs its runs in its dev container. */
  hostPinned?: boolean
}

export type HealthStatus = 'up' | 'down' | 'degraded' | 'unknown'

export interface Sample {
  t: number
  ok: boolean
  ms: number | null
}

export interface HealthView {
  status: HealthStatus
  httpStatus?: number
  latencyMs?: number
  checkedAt?: number
  error?: string
  history: Sample[]
}

export interface VersionInfo {
  sha?: string
  raw?: string
  checkedAt: number
  source: 'http' | 'command'
  error?: string
}

export type EnvKind = 'production' | 'staging' | 'preview' | 'development'
export type Confirm = 'none' | 'click' | 'typed'

export interface EnvConfigView {
  host: string | null
  /** `root@host` or `local`; null when the host is not configured. */
  target: string | null
  health: { url: string; expectStatus: number; jsonPointer?: string | null; equals?: string | null; intervalS: number; timeoutMs: number; viaHost: boolean } | null
  version: { http?: string | null; jsonPointer?: string | null; command: boolean } | null
  auth: { user: string; secret: string; except: string[] } | null
  logs: { name: string; command: string }[]
  commands: { name: string; command: string; confirm: boolean }[]
  deploy: { command: string; local: boolean; confirm: Confirm; requireGreenPipeline: boolean; onlyRef?: string | null; after?: string | null } | null
}

export interface EnvView {
  name: string
  kind: EnvKind
  url: string
  config: EnvConfigView
  health: HealthView
  version: VersionInfo | null
  preview: { mode: 'direct' | 'proxy'; reason?: string }
  deploying?: string
}

/** `env.health` event payload. */
export interface EnvHealthEvent {
  env: string
  status?: HealthStatus
  httpStatus?: number | null
  latencyMs?: number | null
  checkedAt?: number | null
  error?: string | null
  version?: string | null
  versionInfo?: VersionInfo
  sample?: Sample | null
  previewChanged?: boolean
  deploying?: string
}

export type GateStatus = 'pass' | 'fail' | 'warn' | 'skip'

export interface Gate {
  id: 'ref' | 'pipeline' | 'after'
  label: string
  status: GateStatus
  detail: string
  url?: string
}

export interface DeployPlan {
  env: string
  kind: EnvKind
  sha: string
  sha8: string
  branch?: string
  subject?: string
  target: string
  command: string
  confirm: Confirm
  gates: Gate[]
  ok: boolean
  deploying?: string
}

export interface ProxyUrl {
  url: string | null
  port?: number
  reason?: string
}

/** Params of the `app` panel. */
export interface AppPanelParams {
  projectId: string
  env?: string
  run?: string
  url: string
}
