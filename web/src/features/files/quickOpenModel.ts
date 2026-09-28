// Pure parts of "Go to File": the ranked item list and which item is selected.

export interface QuickOpenItem {
  path: string
  positions: number[]
  recent: boolean
}

export interface FoundFile {
  path: string
  score: number
  positions: number[]
}

/** Recent files when there is no query; otherwise the server's ranking, with recent
 *  files floated up when they are nearly as good a match as the best one. */
export function rankItems(q: string, results: FoundFile[], recent: string[]): QuickOpenItem[] {
  if (!q) return recent.slice(0, 30).map((path) => ({ path, positions: [], recent: true }))
  const recentRank = new Map(recent.map((p, i) => [p, i]))
  const best = results[0]?.score ?? 0
  return results
    .map((r, i) => ({ r, i, boost: recentRank.has(r.path) && r.score >= best * 0.7 ? 1 : 0 }))
    .sort((a, b) => b.boost - a.boost || a.i - b.i)
    .map(({ r }) => ({ path: r.path, positions: r.positions, recent: recentRank.has(r.path) }))
}

/**
 * The item to select after the list changed. cmdk only moves the selection to the
 * first item when the input changes, which happens before the server's results for
 * it arrive; when the list is then replaced, the selection is left on a value that
 * is no longer rendered, and Enter does nothing. So: the first item when a new
 * query's results have just arrived (`fresh`) or when the selected one is gone,
 * otherwise keep what the user picked (arrow keys, pointer).
 */
export function nextSelection(selected: string, values: string[], fresh: boolean): string {
  if (!fresh && values.includes(selected)) return selected
  return values[0] ?? ''
}
