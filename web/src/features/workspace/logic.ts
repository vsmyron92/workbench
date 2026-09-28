// Pure helpers for the Workspace feature (tested in logic.test.ts).
// Grouping and search semantics adapted from Mr. Mak Workspace (MIT): a pinned
// section, then category groups; search digs through the archive too, and a long
// list (search or archive shown) drops the buckets for one freshness-sorted run.

import type { ViewerKind, WorkspaceCard, WorkspaceStep } from './api'

export const HOME = 'home'
export const ALL = 'all'

export function cardPanelId(scope: string, cardId: string): string {
  return `card:${scope}:${cardId}`
}

export function matchesQuery(card: WorkspaceCard, query: string): boolean {
  const q = query.trim().toLowerCase()
  if (!q) return true
  return [card.title, card.description, card.category, card.id, card.scopeName].some((s) => s?.toLowerCase().includes(q))
}

/** What a list shows: matching cards; archived ones only while searching or when asked. */
export function visibleCards(cards: WorkspaceCard[], query: string, showArchived: boolean): WorkspaceCard[] {
  const searching = query.trim().length > 0
  return cards.filter((c) => matchesQuery(c, query) && (searching || showArchived || !c.archived))
}

export interface CardGroup {
  key: string
  label: string
  cards: WorkspaceCard[]
}

/** Freshest first (the server sorts too; this keeps merged lists honest). */
export function byFreshness(a: WorkspaceCard, b: WorkspaceCard): number {
  return b.touchedAt - a.touchedAt || (a.status === 'active' ? 0 : 1) - (b.status === 'active' ? 0 : 1) || a.title.localeCompare(b.title)
}

/**
 * Pinned first, then one group per category in order of each category's freshest
 * card. `flat` (searching, or the archive is open) gives one date-sorted group.
 */
export function groupCards(cards: WorkspaceCard[], flat: boolean): { pinned: WorkspaceCard[]; groups: CardGroup[] } {
  const pinned = cards.filter((c) => c.pinned).sort(byFreshness)
  const rest = cards.filter((c) => !c.pinned).sort(byFreshness)
  if (flat) return { pinned, groups: rest.length ? [{ key: '__date', label: 'By date', cards: rest }] : [] }
  const map = new Map<string, WorkspaceCard[]>()
  for (const c of rest) {
    const k = c.category || 'other'
    if (!map.has(k)) map.set(k, [])
    map.get(k)!.push(c)
  }
  return { pinned, groups: [...map.entries()].map(([key, list]) => ({ key, label: categoryLabel(key), cards: list })) }
}

export function categoryLabel(category: string): string {
  return (category || 'other').replace(/[-_]+/g, ' ')
}

/** The step to show: `requested` when valid, else the card's default, else the last. */
export function resolveStep(count: number, requested?: number | null, preferred?: number | null): number {
  const valid = (v: number | null | undefined): v is number => typeof v === 'number' && Number.isInteger(v) && v >= 0 && v < count
  if (valid(requested)) return requested
  if (valid(preferred)) return preferred
  return count - 1
}

/** `/view/<grant>/<folder>/` + a card-relative path, each segment encoded. */
export function fileUrl(base: string, path: string, version?: number): string {
  const enc = path
    .split('/')
    .filter((s) => s && s !== '.')
    .map(encodeURIComponent)
    .join('/')
  return `${base}${enc}${version ? `?v=${version}` : ''}`
}

/** Directory part of a card-relative path ('' for top-level files). */
export function dirOf(path: string): string {
  const i = path.lastIndexOf('/')
  return i < 0 ? '' : path.slice(0, i)
}

export function basename(path: string): string {
  const clean = path.replace(/\/+$/, '')
  return clean.slice(clean.lastIndexOf('/') + 1)
}

/**
 * Resolve a link or image reference found in a card file at `fromPath` to a
 * card-relative path. `null` for absolute URLs, fragments and references that
 * leave the card folder.
 */
export function resolveRelative(fromPath: string, ref: string): string | null {
  const r = ref.trim()
  if (!r || r.startsWith('#') || r.startsWith('/') || /^[a-z][a-z0-9+.-]*:/i.test(r)) return null
  const clean = r.split(/[?#]/)[0]
  let decoded: string
  try {
    decoded = decodeURIComponent(clean)
  } catch {
    decoded = clean
  }
  const parts = dirOf(fromPath).split('/').filter(Boolean)
  for (const seg of decoded.split('/')) {
    if (!seg || seg === '.') continue
    if (seg === '..') {
      if (!parts.length) return null
      parts.pop()
    } else parts.push(seg)
  }
  return parts.join('/')
}

/** A link with a scheme (`https:`, `mailto:`) or protocol-relative: not a card file. */
export function isExternalRef(ref: string): boolean {
  const r = ref.trim()
  return r.startsWith('//') || /^[a-z][a-z0-9+.-]*:/i.test(r)
}

/**
 * Where a link in a repository card's file points in its project, when it leaves
 * the card folder: relative to `workspace/<folder>/<fromPath>`, or `/…` from the
 * project root (as on GitHub). `null` for Workbench cards (their folders live in the
 * data dir), external links, and links that climb out of the project.
 */
export function repoLinkPath(card: Pick<WorkspaceCard, 'origin' | 'folder'>, fromPath: string, ref: string): string | null {
  if (card.origin !== 'repo' || isExternalRef(ref)) return null
  const r = ref.trim()
  if (!r || r.startsWith('#')) return null
  const path = r.startsWith('/') ? resolveRelative('x', r.replace(/^\/+/, '')) : resolveRelative(`workspace/${card.folder}/${fromPath}`, r)
  return path || null
}

/** GitHub-style heading anchor: `## Get started!` → `get-started`. */
export function headingSlug(text: string): string {
  return text
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s_-]/gu, '')
    .replace(/\s/g, '-')
}

/**
 * Whether Workbench should open a link a framed report asked for: http(s) only, and
 * never Workbench itself (the app would run with a URL the report chose, e.g. one
 * carrying `#wbk=`), including its other loopback names on the same port.
 */
export function externalLinkTarget(href: unknown, here: { origin: string; hostname: string; port: string; protocol: string }): string | null {
  if (typeof href !== 'string' || href.length > 8192) return null
  let u: URL
  try {
    u = new URL(href)
  } catch {
    return null
  }
  if (u.protocol !== 'http:' && u.protocol !== 'https:') return null
  if (u.origin === here.origin) return null
  const port = (url: { port: string; protocol: string }) => url.port || (url.protocol === 'https:' ? '443' : '80')
  const loopback = (h: string) => h === 'localhost' || h.endsWith('.localhost') || h === '[::1]' || /^127\.\d+\.\d+\.\d+$/.test(h) || h === '0.0.0.0'
  if (port(u) === port(here) && (u.hostname === here.hostname || loopback(u.hostname))) return null
  return u.href
}

const IMAGE = /\.(png|jpe?g|gif|webp|avif|svg|bmp|ico)$/i
const VIDEO = /\.(mp4|webm|mov|m4v|ogv)$/i

export function isImageName(name: string): boolean {
  return IMAGE.test(name)
}

export function isVideoName(name: string): boolean {
  return VIDEO.test(name)
}

export function kindLabel(kind: ViewerKind): string {
  switch (kind) {
    case 'html':
      return 'Report'
    case 'markdown':
      return 'Document'
    case 'compare3d':
      return '3D'
    case 'pdf':
      return 'PDF'
    default:
      return kind[0].toUpperCase() + kind.slice(1)
  }
}

/** Relative date for a card footer: today, yesterday, `3 d ago`, or a date. */
export function relativeDay(ms: number, now = Date.now()): string {
  if (!ms) return ''
  const day = (t: number) => {
    const d = new Date(t)
    return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime()
  }
  const days = Math.round((day(now) - day(ms)) / 86_400_000)
  if (days <= 0) {
    const min = Math.floor((now - ms) / 60_000)
    if (min < 1) return 'just now'
    if (min < 60) return `${min} min ago`
    return 'today'
  }
  if (days === 1) return 'yesterday'
  if (days < 30) return `${days} d ago`
  return new Date(ms).toLocaleDateString()
}

export function statusTone(status: string): 'success' | 'accent' | 'muted' {
  return status === 'active' ? 'success' : status === 'done' ? 'accent' : 'muted'
}

/** The picture of a card: its thumbnail through the card's grant. */
export function thumbUrl(card: WorkspaceCard): string | null {
  return card.thumb ? fileUrl(card.base, card.thumb) : null
}

export function stepVersion(step: WorkspaceStep | undefined): number | undefined {
  return step?.mtime ?? undefined
}

/** The 3D viewer's shading modes (compare3d/viewer.ts `ShadingMode`). */
export const SHADING_MODES = ['wire', 'solid', 'normals', 'pbr', 'albedo', 'normalMap', 'rough', 'metal'] as const
export type ShadingModeId = (typeof SHADING_MODES)[number]

/**
 * A manifest's `defaultMode`, from untrusted JSON. Mr. Mak also writes `quads`: our
 * wireframe draws quads whenever the file declares them. Anything else is dropped
 * (the test's kind picks the mode then).
 */
export function manifestMode(v: unknown): ShadingModeId | undefined {
  if (v === 'quads') return 'wire'
  return typeof v === 'string' && (SHADING_MODES as readonly string[]).includes(v) ? (v as ShadingModeId) : undefined
}

/** A one-model manifest, so a single .glb step opens in the 3D viewer. */
export function singleModelManifest(path: string, name: string) {
  return {
    title: name,
    tests: [{ id: 'model', name, kind: 'highpoly' as const, models: [{ file: basename(path), label: name }] }],
  }
}

/** Prompt for "Ask agent about this card" (pasted, not submitted: the user adds the question). */
export function askPrompt(card: WorkspaceCard, step?: WorkspaceStep): string {
  return [
    `About the Workspace card "${card.title}" (cardId ${card.id}, scope ${card.scope}):`,
    card.description ? `Description: ${card.description}` : '',
    `Its files are in ${card.folderPath}.`,
    card.steps.length ? `Steps: ${card.steps.map((s) => `${s.index}. ${s.name} (${s.path})`).join('; ')}.` : 'It has no steps yet.',
    step ? `I am looking at step ${step.index}, "${step.name}" (${step.path}).` : '',
    card.editable
      ? 'Change it with the workspace_* MCP tools (workspace_write_file, workspace_add_step, workspace_update_card).'
      : "It comes from the repository's workspace/workspace.json; its files are edited in the project.",
    'Question: ',
  ]
    .filter(Boolean)
    .join('\n')
}
