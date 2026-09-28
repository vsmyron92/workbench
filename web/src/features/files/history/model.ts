// Pure logic of the Local History views: how entries read, day groups, panel ids,
// and reverting the changed lines inside a selection.

import { diffLines, rollbackBlock, splitLines } from '../lineDiff'
import type { HistoryEntry, Untracked } from './api'

/** What a row says ("Saved in Workbench", "External (agent) edit · …"). */
export function describeEntry(e: Pick<HistoryEntry, 'kind' | 'label' | 'who'>): string {
  switch (e.kind) {
    case 'save':
      return e.label || 'Saved in Workbench'
    case 'disk':
      return 'Changed on disk'
    case 'agent':
      return e.who ? `External (agent) edit · ${e.who}` : 'External (agent) edit'
    case 'base':
      return e.label || 'Opened in Workbench'
    case 'deleted':
      return 'Deleted'
    default:
      return e.label ?? ''
  }
}

export const isLabel = (e: Pick<HistoryEntry, 'kind'>) => e.kind === 'label' || e.kind === 'auto'
export const hasContent = (e: Pick<HistoryEntry, 'kind'>) => e.kind === 'save' || e.kind === 'disk' || e.kind === 'agent' || e.kind === 'base'

export const UNTRACKED_TEXT: Record<Untracked, string> = {
  sensitive: 'This file matches a sensitive pattern, so Local History never records it.',
  git: 'Files inside .git are not recorded.',
  ignored: 'This file is ignored (.gitignore or a build folder), so Local History does not record it.',
  binary: 'Binary files are not recorded.',
  tooLarge: 'Files larger than 2 MB are not recorded.',
}

export interface DayGroup<T> {
  key: string
  label: string
  entries: T[]
}

function dayKey(d: Date): string {
  return `${d.getFullYear()}-${d.getMonth() + 1}-${d.getDate()}`
}

/** Entries (newest first) grouped by local day: Today, Yesterday, then dates. */
export function groupByDay<T extends { ts: number }>(entries: T[], now = Date.now()): DayGroup<T>[] {
  const today = new Date(now)
  const yesterday = new Date(now)
  yesterday.setDate(yesterday.getDate() - 1)
  const out: DayGroup<T>[] = []
  for (const e of entries) {
    const d = new Date(e.ts)
    const key = dayKey(d)
    let g = out[out.length - 1]
    if (!g || g.key !== key) {
      const label =
        key === dayKey(today)
          ? 'Today'
          : key === dayKey(yesterday)
            ? 'Yesterday'
            : d.toLocaleDateString(undefined, { weekday: 'long', month: 'short', day: 'numeric', year: d.getFullYear() === today.getFullYear() ? undefined : 'numeric' })
      g = { key, label, entries: [] }
      out.push(g)
    }
    g.entries.push(e)
  }
  return out
}

/** `14:05:09` in local time. */
export function clockTime(ts: number): string {
  const d = new Date(ts)
  const two = (n: number) => String(n).padStart(2, '0')
  return `${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}`
}

/** Panel id: `localHistory:<projectId>:<path>`; folders end with `/` (the project: `/`). */
export function historyPanelId(projectId: string, path: string, dir: boolean): string {
  return `localHistory:${projectId}:${path}${dir ? '/' : ''}`
}

export function historyTitle(path: string, dir: boolean): string {
  if (dir && !path) return 'Recent Changes'
  const name = path.slice(path.lastIndexOf('/') + 1)
  return `${name}${dir ? '/' : ''} (Local History)`
}

/**
 * `current` with the changes that touch lines `from`–`to` (1-based, inclusive, in
 * `current`) reverted to `revision`. A deletion right after `to` or before `from`
 * counts when the selection is a single line next to it. Returns null when no
 * change touches the selection.
 */
export function revertLines(current: string, revision: string, from: number, to: number): string | null {
  const eol = current.includes('\r\n') ? '\r\n' : '\n'
  const cur = splitLines(current)
  const base = splitLines(revision)
  const blocks = diffLines(base, cur).filter((b) =>
    b.kind === 'deleted' ? b.start >= from - 1 && b.start <= to : b.start <= to && b.end >= from,
  )
  if (!blocks.length) return null
  let lines = cur
  // Bottom-up, so earlier blocks keep their line numbers.
  for (const b of [...blocks].sort((a, c) => c.start - a.start)) lines = rollbackBlock(lines, base, b)
  return lines.join(eol)
}

/**
 * Which row is selected after the entries changed: the same one if it is still
 * listed, else the newest version, skipping those identical to `currentHash` (the
 * file on disk now: comparing with it would show nothing).
 */
export function keepSelection(entries: HistoryEntry[], selected: number | null, currentHash?: string | null): number | null {
  if (selected !== null && entries.some((e) => e.id === selected)) return selected
  const versions = entries.filter((e) => !isLabel(e))
  return (versions.find((e) => !currentHash || e.hash !== currentHash) ?? versions[0])?.id ?? null
}

/** `14:05:09` today, `Sep 26 14:05` on other days. */
export function shortWhen(ts: number, now = Date.now()): string {
  const d = new Date(ts)
  const today = new Date(now)
  if (d.toDateString() === today.toDateString()) return clockTime(ts)
  return `${d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: d.getFullYear() === today.getFullYear() ? undefined : 'numeric' })} ${clockTime(ts).slice(0, 5)}`
}
