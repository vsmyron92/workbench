// Anchoring an inline comment: Confluence finds the commented text by the selection
// itself, how often it occurs in the page and which occurrence is meant
// (`textSelectionMatchCount` / `textSelectionMatchIndex`). Occurrences are counted
// without overlaps from the start of the page text, as the server does.

/** Start offsets of the non-overlapping occurrences of `needle` in `hay`. */
export function occurrences(hay: string, needle: string): number[] {
  if (!needle) return []
  const out: number[] = []
  let from = 0
  for (;;) {
    const i = hay.indexOf(needle, from)
    if (i < 0) return out
    out.push(i)
    from = i + needle.length
  }
}

export interface Anchor {
  selection: string
  matchIndex: number
  matchCount: number
}

export const MAX_SELECTION = 1000

/**
 * Where the selection `raw`, starting at `offset` in the page text `text`, sits among
 * the occurrences of that text. Surrounding whitespace is dropped. Returns a reason
 * when the selection cannot anchor a comment.
 */
export function anchorOf(text: string, raw: string, offset: number): Anchor | { error: string } {
  const lead = raw.length - raw.trimStart().length
  const selection = raw.trim()
  const start = offset + lead
  if (!selection) return { error: 'Select some text first' }
  if (/[\r\n]/.test(selection)) return { error: 'Select text within one paragraph' }
  if (selection.length > MAX_SELECTION) return { error: `Select at most ${MAX_SELECTION} characters` }
  if (text.slice(start, start + selection.length) !== selection) return { error: 'The selection does not match the page text' }
  const hits = occurrences(text, selection)
  const matchIndex = hits.indexOf(start)
  // The selected occurrence overlaps an earlier one ("aa" in "aaa"): no index names it.
  if (matchIndex < 0) return { error: 'Select a longer passage: this one overlaps another occurrence of the same text' }
  return { selection, matchIndex, matchCount: hits.length }
}
