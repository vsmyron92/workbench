// Inline styles from page storage that the editor may show. Storage is written by
// anyone who can edit the page, and the editor renders it inside Workbench's own
// origin, so a `<span style>` must not be able to position, size or layer anything
// (`position:fixed; inset:0; z-index:…` would cover the whole app) or fetch anything
// (`url(…)`). Only text colour and emphasis get through. The original attribute stays
// in the document (`xattrs`) and is written back unchanged; this filters display only.

/** Properties a span may set in the editor (text colour and emphasis, nothing layout-related). */
const ALLOWED = new Set([
  'color',
  'background-color',
  'text-decoration',
  'text-decoration-line',
  'text-decoration-color',
  'text-decoration-style',
  'font-weight',
  'font-style',
])

/** Keywords, numbers, `#hex` and colour functions only: no quotes, escapes, comments or `!`. */
const SAFE_VALUE = /^[a-z0-9#.,%\s()-]+$/i
const COLOR_FUNCTIONS = new Set(['rgb', 'rgba', 'hsl', 'hsla'])

function safeValue(v: string): boolean {
  if (!SAFE_VALUE.test(v)) return false
  // Every function call must be a colour function (no url(), image-set(), var()…).
  for (const m of v.matchAll(/([a-z-]*)\s*\(/gi)) {
    if (!COLOR_FUNCTIONS.has(m[1].toLowerCase())) return false
  }
  return true
}

/**
 * The displayable part of a storage `style` attribute: allowed declarations only,
 * normalized to `prop: value` pairs joined by `; `. Null when nothing is left.
 */
export function safeInlineStyle(style: string | null | undefined): string | null {
  if (!style) return null
  const kept: string[] = []
  for (const decl of style.split(';')) {
    const i = decl.indexOf(':')
    if (i < 0) continue
    const prop = decl.slice(0, i).trim().toLowerCase()
    const value = decl.slice(i + 1).trim()
    if (!ALLOWED.has(prop) || !value || !safeValue(value)) continue
    kept.push(`${prop}: ${value}`)
  }
  return kept.length ? kept.join('; ') : null
}
