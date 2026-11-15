// The live-data bridge of Workspace reports: what a sandboxed report may ask Workbench for.
//
// A report runs in an opaque origin and cannot reach `/api`. It posts `workbench:query` messages to the page
// that frames it (the prelude of server/src/workspace/view.rs defines `workbench.query`), and this module
// answers them from the user's own session, after the user allowed the card (see `useReportBridge`).
//
// What holds it:
//   * a fixed list of kinds, each a read of one project's data (the card's own project, never a name from the
//     report) through an existing route, so nothing here can change anything;
//   * every parameter is checked and bounded, every answer is rebuilt field by field (no paths of this
//     computer, no tokens, no terminal text, no message bodies) and capped in size;
//   * a failure says what happened without the server's detail beyond its own message.

import type { TerminalInfo } from '@/api/types'

export type BridgeKind = 'sessions' | 'git.repos' | 'ci.runs' | 'data.series' | 'data.view' | 'data.stats'

export interface KindInfo {
  id: BridgeKind
  /** What the user is told a report may read when they allow it. */
  label: string
}

export const KINDS: KindInfo[] = [
  { id: 'sessions', label: 'agent sessions of this project (state, model, cost)' },
  { id: 'git.repos', label: 'git branches and changes' },
  { id: 'ci.runs', label: 'recent CI runs' },
  { id: 'data.series', label: 'data viewer series and their readings' },
]

export type BridgeCode = 'bad_request' | 'unavailable' | 'denied' | 'busy' | 'too_large' | 'failed'

export class BridgeError extends Error {
  code: BridgeCode
  constructor(code: BridgeCode, message: string) {
    super(message)
    this.code = code
  }
}

/** The largest answer handed to a report, as JSON text. */
export const MAX_RESULT_BYTES = 1_500_000
/** The largest request a report may send, as JSON text. */
const MAX_PARAMS_BYTES = 4096
const MAX_SERIES = 16
const MAX_BUCKETS = 2000
const MAX_RUNS = 30

export interface BridgeRequest {
  id: number
  kind: string
  params: Record<string, unknown>
}

/** A `workbench:query` message of a report, or `null` when the message is something else. */
export function parseRequest(raw: unknown): BridgeRequest | { id: number | null; invalid: string } | null {
  if (!raw || typeof raw !== 'object') return null
  const m = raw as { type?: unknown; id?: unknown; kind?: unknown; params?: unknown }
  if (m.type !== 'workbench:query') return null
  const id = typeof m.id === 'number' && Number.isSafeInteger(m.id) && m.id > 0 ? m.id : null
  if (id === null) return { id: null, invalid: 'id: a positive whole number' }
  if (typeof m.kind !== 'string' || m.kind.length === 0 || m.kind.length > 40) return { id, invalid: 'kind: the name of what to read' }
  const params = m.params === undefined ? {} : m.params
  if (!params || typeof params !== 'object' || Array.isArray(params)) return { id, invalid: 'params: an object' }
  let size = 0
  try {
    size = JSON.stringify(params).length
  } catch {
    return { id, invalid: 'params: plain JSON' }
  }
  if (size > MAX_PARAMS_BYTES) return { id, invalid: `params: at most ${MAX_PARAMS_BYTES} characters` }
  return { id, kind: m.kind, params: params as Record<string, unknown> }
}

export interface BridgeIo {
  get<T>(path: string, query?: Record<string, string | number | undefined>): Promise<T>
  post<T>(path: string, body: unknown): Promise<T>
}

type Params = Record<string, unknown>

const bad = (message: string) => new BridgeError('bad_request', message)

function text(p: Params, key: string, max: number): string | undefined {
  const v = p[key]
  if (v === undefined || v === null) return undefined
  // eslint-disable-next-line no-control-regex
  if (typeof v !== 'string' || v.length === 0 || v.length > max || /[\u0000-\u001f\u007f]/.test(v)) throw bad(`${key}: a short text`)
  return v
}

function count(p: Params, key: string, max: number, fallback: number): number {
  const v = p[key]
  if (v === undefined || v === null) return fallback
  if (typeof v !== 'number' || !Number.isInteger(v) || v < 1) throw bad(`${key}: a whole number from 1 to ${max}`)
  return Math.min(v, max)
}

function seconds(p: Params, key: string): number | undefined {
  const v = p[key]
  if (v === undefined || v === null) return undefined
  if (typeof v !== 'number' || !Number.isFinite(v)) throw bad(`${key}: a number of seconds`)
  return v
}

function seriesNames(p: Params): string[] {
  const v = p.series
  if (!Array.isArray(v) || v.length === 0 || v.length > MAX_SERIES) throw bad(`series: 1 to ${MAX_SERIES} series names`)
  return v.map((n, i) => {
    const name = text({ n }, 'n', 200)
    if (name === undefined) throw bad(`series[${i}]: a series name`)
    return name
  })
}

/** A repository id as the git routes name them. */
function repoId(p: Params): string | undefined {
  const v = text(p, 'repo', 64)
  if (v !== undefined && !/^[A-Za-z0-9][A-Za-z0-9_.-]*$/.test(v)) throw bad('repo: a repository id of this project')
  return v
}

const num = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null)
const str = (v: unknown, max = 300): string | null => (typeof v === 'string' ? v.slice(0, max) : null)
const rec = (v: unknown): Record<string, unknown> => (v && typeof v === 'object' ? (v as Record<string, unknown>) : {})

// ---------------------------------------------------------------------------------------------- shaping

export function shapeSessions(list: TerminalInfo[], projectId: string) {
  return {
    sessions: list
      .filter((t) => t.projectId === projectId && t.agent)
      .map((t) => ({
        id: t.id,
        title: t.title.slice(0, 200),
        provider: t.agent!.provider,
        state: t.agent!.state,
        unread: t.agent!.unread,
        model: t.agent!.model,
        effort: t.agent!.effort,
        contextPct: t.agent!.contextPct,
        costUsd: t.agent!.costUsd,
        lastEventAt: t.agent!.lastEventAt,
        running: t.status === 'running',
      })),
  }
}

export function shapeRepos(list: unknown) {
  const rows = Array.isArray(list) ? list : []
  const forge = (v: unknown) => {
    const f = rec(v)
    return typeof f.host === 'string' && typeof f.path === 'string' ? { host: f.host.slice(0, 200), path: f.path.slice(0, 300) } : null
  }
  return {
    repos: rows.slice(0, 50).map((r) => {
      const o = rec(r)
      const err = rec(o.error)
      return {
        id: str(o.id, 64),
        name: str(o.name, 200),
        default: o.default === true,
        branch: str(o.branch, 200),
        head: str(o.head, 40),
        upstream: str(o.upstream, 200),
        ahead: num(o.ahead),
        behind: num(o.behind),
        changed: num(o.changed),
        conflicts: num(o.conflicts),
        state: str(o.state, 40),
        github: forge(o.github),
        gitlab: forge(o.gitlab),
        error: typeof err.code === 'string' ? err.code.slice(0, 80) : null,
      }
    }),
  }
}

export function shapeSeries(snapshot: unknown) {
  const rows = Array.isArray(rec(snapshot).series) ? (rec(snapshot).series as unknown[]) : []
  return {
    series: rows.slice(0, 500).map((r) => {
      const o = rec(r)
      return {
        name: str(o.name, 200),
        kind: str(o.kind, 20),
        unit: str(o.unit, 40),
        label: str(o.label, 200),
        group: str(o.group, 200),
        count: num(o.count),
        tStart: num(o.tStart),
        tEnd: num(o.tEnd),
        last: num(o.last),
        wall: o.wall === true,
        updatedAt: num(o.updatedAt),
      }
    }),
  }
}

/** A GitHub run or a GitLab pipeline as one shape. */
export function shapeRuns(provider: 'github' | 'gitlab', page: unknown) {
  const items = Array.isArray(rec(page).items) ? (rec(page).items as unknown[]) : []
  const link = (v: unknown) => (typeof v === 'string' && v.startsWith('https://') ? v.slice(0, 500) : null)
  return {
    provider,
    runs: items.slice(0, MAX_RUNS).map((r) => {
      const o = rec(r)
      return provider === 'github'
        ? {
            id: num(o.id),
            title: str(o.displayTitle) ?? str(o.name),
            status: str(o.status, 40),
            conclusion: str(o.conclusion, 40),
            branch: str(o.headBranch, 200),
            sha: str(o.headSha, 40),
            event: str(o.event, 60),
            createdAt: str(o.createdAt, 40),
            durationSec: num(o.duration),
            url: link(o.htmlUrl),
          }
        : {
            id: num(o.id),
            title: str(o.commitTitle) ?? str(o.name),
            status: str(o.status, 40),
            conclusion: null,
            branch: str(o.ref, 200),
            sha: str(o.sha, 40),
            event: str(o.source, 60),
            createdAt: str(o.createdAt, 40),
            durationSec: num(o.duration),
            url: link(o.webUrl),
          }
    }),
  }
}

/** Numbers and short labels only: what a plot draws. Anything else is dropped. */
function numbersOnly(v: unknown, depth = 0): unknown {
  if (depth > 6) return null
  if (typeof v === 'number') return Number.isFinite(v) ? v : null
  if (typeof v === 'string') return v.slice(0, 200)
  if (typeof v === 'boolean' || v === null) return v
  if (Array.isArray(v)) return v.slice(0, 20_000).map((x) => numbersOnly(x, depth + 1))
  if (typeof v === 'object') {
    const out: Record<string, unknown> = {}
    for (const [k, x] of Object.entries(v as Record<string, unknown>).slice(0, 200)) out[k.slice(0, 200)] = numbersOnly(x, depth + 1)
    return out
  }
  return null
}

// ---------------------------------------------------------------------------------------------- running

export interface BridgeCtx {
  /** The card's own project: the only one a report can read. */
  projectId: string
}

const enc = encodeURIComponent

/** Answer one request. Throws `BridgeError`. */
export async function runQuery(kind: string, p: Params, ctx: BridgeCtx, io: BridgeIo): Promise<unknown> {
  const base = `/api/projects/${enc(ctx.projectId)}`
  let out: unknown
  switch (kind) {
    case 'sessions':
      out = shapeSessions(await io.get<TerminalInfo[]>('/api/terminals'), ctx.projectId)
      break
    case 'git.repos':
      out = shapeRepos(await io.get<unknown>(`${base}/git/repos`))
      break
    case 'data.series':
      out = shapeSeries(await io.get<unknown>(`${base}/data`))
      break
    case 'data.view': {
      const buckets = count(p, 'buckets', MAX_BUCKETS, 400)
      const body = { series: seriesNames(p), from: seconds(p, 'from'), to: seconds(p, 'to'), buckets }
      out = { views: numbersOnly(rec(await io.post<unknown>(`${base}/data/view`, body)).views) }
      break
    }
    case 'data.stats': {
      const body = { series: seriesNames(p), from: seconds(p, 'from'), to: seconds(p, 'to') }
      out = { stats: numbersOnly(rec(await io.post<unknown>(`${base}/data/stats`, body)).stats) }
      break
    }
    case 'ci.runs': {
      const limit = count(p, 'limit', MAX_RUNS, 10)
      const repo = repoId(p)
      const branch = text(p, 'branch', 200)
      let provider = text(p, 'provider', 10)
      if (provider !== undefined && provider !== 'github' && provider !== 'gitlab') throw bad('provider: "github" or "gitlab"')
      if (!provider) {
        // The repository's own forge.
        const repos = shapeRepos(await io.get<unknown>(`${base}/git/repos`)).repos
        const r = repos.find((x) => (repo ? x.id === repo : x.default)) ?? repos[0]
        provider = r?.github ? 'github' : r?.gitlab ? 'gitlab' : undefined
        if (!provider) throw new BridgeError('unavailable', 'this project has no GitHub or GitLab remote')
      }
      if (provider === 'github') {
        out = shapeRuns('github', await io.get<unknown>(`${base}/github/actions/runs`, { repo, branch, perPage: limit }))
      } else {
        out = shapeRuns('gitlab', await io.get<unknown>(`${base}/gitlab/pipelines`, { repo, ref: branch, perPage: limit }))
      }
      break
    }
    default:
      throw bad(`kind: one of ${[...KINDS.map((k) => k.id), 'data.view', 'data.stats'].join(', ')}`)
  }
  let size = 0
  try {
    size = JSON.stringify(out).length
  } catch {
    throw new BridgeError('failed', 'the answer could not be read')
  }
  if (size > MAX_RESULT_BYTES) throw new BridgeError('too_large', 'the answer is too large: ask for fewer series or buckets')
  return out
}
