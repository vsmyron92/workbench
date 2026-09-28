// Pure helpers of the Find in Files view: grouping hits into rows and locating
// the highlighted ranges inside a (possibly windowed) preview line.

import type { SearchHit } from './api'

export type SearchRow =
  | { kind: 'file'; path: string; count: number; collapsed: boolean }
  | { kind: 'line'; path: string; line: number; hits: SearchHit[] }

/** Hits sorted by path/line/column → a file row per file and a row per matching line. */
export function groupRows(matches: SearchHit[], collapsed: ReadonlySet<string>): { rows: SearchRow[]; files: number } {
  const rows: SearchRow[] = []
  let files = 0
  for (let i = 0; i < matches.length; ) {
    const path = matches[i].path
    let j = i
    while (j < matches.length && matches[j].path === path) j++
    files++
    const isCollapsed = collapsed.has(path)
    rows.push({ kind: 'file', path, count: j - i, collapsed: isCollapsed })
    if (!isCollapsed) {
      for (let k = i; k < j; ) {
        const line = matches[k].line
        let l = k
        while (l < j && matches[l].line === line) l++
        rows.push({ kind: 'line', path, line, hits: matches.slice(k, l) })
        k = l
      }
    }
    i = j
  }
  return { rows, files }
}

export interface PreviewParts {
  /** The line prefix was cut (long line): show an ellipsis first. */
  cut: boolean
  /** Alternating plain / highlighted segments, starting with plain. */
  segments: { text: string; hit: boolean }[]
}

/**
 * Split the first hit's preview into plain and highlighted segments for every hit
 * on that line. Columns are UTF-16 based (JS string indices); leading indentation
 * is dropped.
 */
export function previewParts(hits: SearchHit[]): PreviewParts {
  const first = hits[0]
  const text = first.preview
  const lead = text.length - text.trimStart().length
  const ranges = hits
    .map((h) => [h.column - 1 - first.previewOffset, h.endColumn - 1 - first.previewOffset] as const)
    .filter(([s, e]) => s >= 0 && e <= text.length && e > s)
  const segments: { text: string; hit: boolean }[] = []
  let pos = Math.min(lead, ranges[0]?.[0] ?? lead)
  for (const [s, e] of ranges) {
    if (s < pos) continue
    segments.push({ text: text.slice(pos, s), hit: false })
    segments.push({ text: text.slice(s, e), hit: true })
    pos = e
  }
  segments.push({ text: text.slice(pos), hit: false })
  return { cut: first.previewOffset > 0, segments: segments.filter((x) => x.text.length > 0) }
}
