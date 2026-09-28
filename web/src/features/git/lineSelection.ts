// Pure helpers of line selection in the diff viewer (unit-tested in p3.test.ts):
// which changed lines an editor selection covers, keys for checkbox state, and the
// line references the server takes.

import type { DiffLine, LineRef } from './types'

/** `a:12` (added line 12 of the modified side) or `d:5` (deleted line 5 of the original side). */
export const lineKey = (l: { kind: 'add' | 'del'; line: number }) => `${l.kind === 'add' ? 'a' : 'd'}:${l.line}`

export function parseLineKey(k: string): LineRef | null {
  const m = /^([ad]):(\d+)$/.exec(k)
  return m ? { kind: m[1] === 'a' ? 'add' : 'del', line: Number(m[2]) } : null
}

export function refsOf(keys: Iterable<string>): LineRef[] {
  const out: LineRef[] = []
  for (const k of keys) {
    const r = parseLineKey(k)
    if (r) out.push(r)
  }
  return out
}

export interface LineRange {
  start: number
  end: number
}

/**
 * Changed lines covered by editor selections: added lines inside a range of the
 * modified side, deleted lines inside a range of the original side, and deletions
 * that sit at a selected line of the modified side (a replaced block's old lines).
 */
export function linesInRanges(lines: DiffLine[], modified: LineRange[], original: LineRange[]): string[] {
  const inAny = (rs: LineRange[], n: number) => rs.some((r) => n >= r.start && n <= r.end)
  const out: string[] = []
  for (const l of lines) {
    if (l.kind === 'add' ? inAny(modified, l.line) : inAny(original, l.line) || inAny(modified, l.at)) out.push(lineKey(l))
  }
  return out
}

export function hunkKeys(lines: DiffLine[], hunk: number): string[] {
  return lines.filter((l) => l.hunk === hunk).map(lineKey)
}

/** Toggle a group of keys: all selected → none; otherwise all. */
export function toggleKeys(sel: ReadonlySet<string>, keys: string[]): Set<string> {
  const next = new Set(sel)
  if (keys.every((k) => next.has(k))) keys.forEach((k) => next.delete(k))
  else keys.forEach((k) => next.add(k))
  return next
}

/** Keep only keys that are still changed lines (after the diff was reloaded). */
export function prune(sel: ReadonlySet<string>, lines: DiffLine[] | undefined): Set<string> {
  const live = new Set((lines ?? []).map(lineKey))
  return new Set([...sel].filter((k) => live.has(k)))
}

export const countLabel = (n: number) => `${n} line${n === 1 ? '' : 's'}`
