// Pure helpers of the apps feature (unit-tested in logic.test.ts).

import type { RunKind } from '@/api/types'
import type { EnvHealthEvent, EnvKind, EnvView, HealthStatus, RunLive, RunState, RunView, Sample } from './types'

export const HISTORY = 60

export type Tone = 'success' | 'warning' | 'danger' | 'accent' | 'muted'

export function healthTone(s: HealthStatus | undefined): Tone {
  switch (s) {
    case 'up':
      return 'success'
    case 'degraded':
      return 'warning'
    case 'down':
      return 'danger'
    default:
      return 'muted'
  }
}

export function runTone(r: Pick<RunView, 'state' | 'error' | 'result'>): Tone {
  switch (r.state) {
    case 'ready':
      return r.error ? 'warning' : 'success'
    case 'running':
      return r.error ? 'warning' : 'accent'
    case 'starting':
      return 'accent'
    case 'failed':
      return 'danger'
    case 'exited':
      return r.result && r.result.failed > 0 ? 'danger' : r.result ? 'success' : 'muted'
    default:
      return 'muted'
  }
}

export function isActive(s: RunState | undefined): boolean {
  return s === 'starting' || s === 'running' || s === 'ready'
}

/** Short state text for chips: "ready :5173", "12 passed", "waiting for api". */
export function runStateLabel(r: Pick<RunView, 'state' | 'phase' | 'port' | 'result' | 'exit' | 'config'>): string {
  const res = r.result
  const counts = res ? `${res.passed} passed${res.failed ? `, ${res.failed} failed` : ''}` : null
  switch (r.state) {
    case 'starting':
      return r.phase ?? 'starting…'
    case 'ready':
      return r.port ? `ready :${r.port}` : counts ?? 'ready'
    case 'running':
      return counts ?? (r.port && r.config.kind === 'server' ? `running :${r.port}` : 'running')
    case 'failed':
      return counts ?? (r.exit?.code != null ? `failed (${r.exit.code})` : 'failed')
    case 'exited':
      // Closed or killed from outside: its exit code (portable-pty's 1 with a signal) says nothing.
      if (r.exit?.terminated) return counts ?? 'terminated'
      return counts ?? (r.exit?.code != null ? `exit ${r.exit.code}` : 'finished')
    default:
      return ''
  }
}

const GROUP_LABEL: Record<string, string> = {
  dev: 'Development',
  test: 'Tests',
  build: 'Builds',
  unity: 'Unity',
  tasks: 'Tasks',
  deploy: 'Deploy & release',
  suggested: 'Suggested from docs',
}
const KIND_GROUP: Record<RunKind, string> = {
  server: 'dev',
  service: 'dev',
  test: 'test',
  build: 'build',
  editor: 'tools',
  task: 'tasks',
}
const GROUP_ORDER = ['dev', 'test', 'build', 'unity', 'tools', 'tasks', 'deploy']

export function groupLabel(g: string): string {
  return GROUP_LABEL[g] ?? (g === 'tools' ? 'Tools' : g.charAt(0).toUpperCase() + g.slice(1))
}

/** Runs grouped by `group` (else by kind), in a stable order; "suggested" last. */
export function groupRuns<T extends Pick<RunView, 'config'>>(runs: T[]): { id: string; label: string; runs: T[] }[] {
  const map = new Map<string, T[]>()
  for (const r of runs) {
    const g = r.config.group ?? KIND_GROUP[r.config.kind] ?? 'tasks'
    if (!map.has(g)) map.set(g, [])
    map.get(g)!.push(r)
  }
  const rank = (g: string) => (g === 'suggested' ? 1000 : GROUP_ORDER.indexOf(g) === -1 ? 500 : GROUP_ORDER.indexOf(g))
  return [...map.entries()].sort((a, b) => rank(a[0]) - rank(b[0]) || a[0].localeCompare(b[0])).map(([id, rs]) => ({ id, label: groupLabel(id), runs: rs }))
}

/** The config a fresh project selects in the top bar: first server, else first everyday run (no suggestion, no deploy). */
export function defaultRun<T extends Pick<RunView, 'name' | 'config'>>(runs: T[]): string | null {
  const real = runs.filter((r) => r.config.group !== 'suggested' && r.config.group !== 'deploy')
  return (real.find((r) => r.config.kind === 'server') ?? real[0] ?? runs[0])?.name ?? null
}

/** Apply a `run.state` payload to a run (fields absent from the event are cleared). */
export function applyRunEvent<T extends RunView>(run: T, ev: RunLive): T {
  return {
    ...run,
    state: ev.state,
    terminalId: ev.terminalId,
    port: ev.port,
    url: ev.url,
    startedAt: ev.startedAt,
    readyAt: ev.readyAt,
    exit: ev.exit,
    result: ev.result,
    error: ev.error,
    phase: ev.phase,
  }
}

/** Apply an `env.health` payload; returns `null` when a refetch is needed instead. */
export function applyEnvEvent(env: EnvView, ev: EnvHealthEvent): EnvView | null {
  if (ev.previewChanged || ev.deploying !== undefined || ev.versionInfo) return null
  if (!ev.status) return env
  const history = ev.sample ? [...env.health.history, ev.sample].slice(-HISTORY) : env.health.history
  return {
    ...env,
    health: {
      status: ev.status,
      httpStatus: ev.httpStatus ?? undefined,
      latencyMs: ev.latencyMs ?? undefined,
      checkedAt: ev.checkedAt ?? undefined,
      error: ev.error ?? undefined,
      history,
    },
    version: ev.version && env.version?.sha !== ev.version ? { ...(env.version ?? { checkedAt: ev.checkedAt ?? Date.now(), source: 'http' }), sha: ev.version } : env.version,
  }
}

export interface Spark {
  /** Polyline points of latency (successful samples). */
  line: string
  /** x positions of failed samples. */
  fails: number[]
  max: number
}

/** Sparkline geometry for `samples` in a `w`×`h` box (latency, failures as ticks). */
export function sparkline(samples: Sample[], w: number, h: number): Spark {
  const n = Math.max(samples.length, 2)
  const step = w / (Math.max(HISTORY, n) - 1)
  const offset = w - (samples.length - 1) * step
  const oks = samples.map((s) => (s.ok && s.ms != null ? s.ms : null))
  const max = Math.max(50, ...oks.filter((v): v is number => v != null))
  const pts: string[] = []
  const fails: number[] = []
  samples.forEach((s, i) => {
    const x = +(offset + i * step).toFixed(1)
    if (s.ok && s.ms != null) pts.push(`${x},${+(h - 1 - (s.ms / max) * (h - 3)).toFixed(1)}`)
    else fails.push(x)
  })
  return { line: pts.join(' '), fails, max }
}

export function envShortLabel(e: Pick<EnvView, 'name' | 'kind'>): string {
  if (e.kind === 'production' && e.name === 'production') return 'prod'
  return e.name.length > 12 ? e.name.slice(0, 11) + '…' : e.name
}

const KIND_RANK: Record<EnvKind, number> = { production: 0, staging: 1, preview: 2, development: 3 }

/** Environments for the top bar pills: production and staging first, at most `max`. */
export function pillEnvs<T extends Pick<EnvView, 'kind' | 'name'>>(envs: T[], max = 3): T[] {
  return [...envs].sort((a, b) => KIND_RANK[a.kind] - KIND_RANK[b.kind]).slice(0, max)
}

export function formatLatency(ms: number | undefined | null): string {
  if (ms == null) return ''
  return ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(1)} s`
}

/** Uptime of the recorded samples, e.g. "58/60". */
export function uptime(samples: Sample[]): string {
  return samples.length ? `${samples.filter((s) => s.ok).length}/${samples.length}` : ''
}

export function appPanelId(projectId: string, key: { env?: string; run?: string }): string {
  return `app:${projectId}:${key.env ?? key.run ?? ''}`
}

export function isLoopbackHost(host: string): boolean {
  const h = host.replace(/^\[|\]$/g, '').toLowerCase()
  return h === 'localhost' || h === '::1' || h.startsWith('127.') || h === '0.0.0.0'
}

/** URL a run's preview should show. */
export function runUrl(r: Pick<RunView, 'url' | 'port' | 'config'>): string | null {
  return r.url ?? r.config.preview ?? (r.config.kind === 'server' && (r.port ?? r.config.port) ? `http://localhost:${r.port ?? r.config.port}/` : null)
}

/** `new URL` that never throws. */
export function parseUrl(s: string): URL | null {
  try {
    return new URL(s)
  } catch {
    return null
  }
}

/** Path + query + hash of `url` when it is on `origin`'s origin, else null. */
export function pathOnOrigin(url: string, origin: string): string | null {
  const u = parseUrl(url)
  const o = parseUrl(origin)
  if (!u || !o || u.origin !== o.origin) return null
  return u.pathname + u.search + u.hash
}

export const DEVICES = {
  desktop: { label: 'Desktop', width: null as number | null },
  tablet: { label: 'Tablet (820 px)', width: 820 },
  phone: { label: 'Phone (390 px)', width: 390 },
} as const
export type Device = keyof typeof DEVICES

/** Short commit id for display. */
export function shortSha(sha: string | undefined | null): string {
  return sha ? sha.slice(0, 8) : ''
}
