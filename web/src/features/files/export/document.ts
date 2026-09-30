// Pure parts of "Export as HTML…": the standalone document around the rendered
// Markdown, theme tokens resolved to literal values (the export is a document of
// its own: nothing of the app's stylesheet or variables comes with it), and the
// table of contents.

import { basename, samePath, segments } from '../paths'

/** Largest image embedded as a data URI, and all images together. */
export const IMAGE_CAP = 5 * 1024 * 1024
export const TOTAL_IMAGE_CAP = 25 * 1024 * 1024

export type ExportTheme = 'light' | 'dark'

/** File names embedded as images (anything else a Markdown image names stays a link). */
const IMAGE_NAME = /\.(png|jpe?g|gif|webp|avif|bmp|ico|svg)$/i

/**
 * Whether the export may fetch `path` (project-relative or absolute, as resolved
 * from the document) to embed it. Repository Markdown is untrusted: `![](../.git/config)`
 * must not copy a remote URL's token into the file, so only image files outside
 * `.git` qualify (on a Windows server also `C:\p\.git\…` and `.GIT`, the same folder there).
 */
export function embeddableImagePath(path: string): boolean {
  return IMAGE_NAME.test(path) && !segments(path).some((s) => samePath(s, '.git'))
}

/** Why an image was not embedded (the text of the export's warning). */
export class ImageSkip extends Error {}

/**
 * Fetch a relative image for embedding: only image files (by name, and served as
 * `image/*`), within the caps (`total`: bytes embedded so far). Throws
 * `ImageSkip` (or the fetch's own `AbortError`).
 */
export async function fetchEmbeddableImage(
  path: string,
  url: string,
  total: number,
  fetcher: (url: string, init: RequestInit) => Promise<Response>,
  signal?: AbortSignal,
): Promise<Blob> {
  if (!embeddableImagePath(path)) throw new ImageSkip('not an image, linked instead')
  const res = await fetcher(url, { credentials: 'same-origin', signal })
  if (!res.ok) {
    throw new ImageSkip(res.status === 403 ? 'marked sensitive, not embedded' : `could not be embedded (HTTP ${res.status}), linked instead`)
  }
  const type = (res.headers.get('content-type') ?? '').toLowerCase()
  if (!type.startsWith('image/')) throw new ImageSkip('not an image, linked instead')
  const size = Number(res.headers.get('content-length') ?? '0')
  if (size > IMAGE_CAP) throw new ImageSkip(`over ${IMAGE_CAP / 1024 / 1024} MB, linked instead`)
  const blob = await res.blob()
  if (blob.type && !blob.type.toLowerCase().startsWith('image/')) throw new ImageSkip('not an image, linked instead')
  if (blob.size > IMAGE_CAP) throw new ImageSkip(`over ${IMAGE_CAP / 1024 / 1024} MB, linked instead`)
  if (total + blob.size > TOTAL_IMAGE_CAP) throw new ImageSkip(`over ${TOTAL_IMAGE_CAP / 1024 / 1024} MB of images in all, linked instead`)
  return blob
}

export interface TocItem {
  id: string
  text: string
  level: number
}

export function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!)
}

/** `docs/README.md` → `README.html`. */
export function exportFileName(path: string): string {
  const name = basename(path) || 'document'
  const dot = name.lastIndexOf('.')
  return `${dot > 0 ? name.slice(0, dot) : name}.html`
}

/** The export's path next to the source file. */
export function exportPath(path: string): string {
  return path.slice(0, path.length - basename(path).length) + exportFileName(path)
}

/** Index of the `)` closing the `(` at `open`, or -1. */
function closing(s: string, open: number): number {
  let depth = 0
  for (let i = open; i < s.length; i++) {
    if (s[i] === '(') depth++
    else if (s[i] === ')' && --depth === 0) return i
  }
  return -1
}

/**
 * Replace `var(--name)` / `var(--name, fallback)` with the value `lookup` gives
 * (resolved in turn). Variables it does not know stay (their fallback resolved),
 * so a document's own custom properties (`--alert` set per callout) keep working.
 */
export function resolveVars(css: string, lookup: (name: string) => string | undefined, depth = 0): string {
  if (depth > 8) return css
  let out = ''
  let i = 0
  for (;;) {
    const at = css.indexOf('var(', i)
    if (at < 0) return out + css.slice(i)
    const end = closing(css, at + 3)
    if (end < 0) return out + css.slice(i)
    out += css.slice(i, at)
    const inner = css.slice(at + 4, end)
    const comma = inner.indexOf(',')
    const name = (comma < 0 ? inner : inner.slice(0, comma)).trim()
    const fallback = comma < 0 ? null : inner.slice(comma + 1).trim()
    const value = lookup(name)?.trim()
    if (value) out += resolveVars(value, lookup, depth + 1)
    else if (fallback !== null) out += `var(${name}, ${resolveVars(fallback, lookup, depth + 1)})`
    else out += css.slice(at, end + 1)
    i = end + 1
  }
}

/** Every `--name` a stylesheet refers to. */
export function varNames(css: string): string[] {
  return [...new Set(Array.from(css.matchAll(/var\(\s*(--[\w-]+)/g), (m) => m[1]))]
}

export function tocHtml(items: TocItem[]): string {
  if (items.length < 2) return ''
  const links = items
    .map((h) => `<a class="wb-md-toc-item l${h.level}" href="#${escapeHtml(h.id)}">${escapeHtml(h.text)}</a>`)
    .join('\n')
  return `<nav class="wb-md-toc" aria-label="Contents">\n<div class="wb-md-toc-title">Contents</div>\n${links}\n</nav>`
}

/**
 * The page chrome of the export, written against the app's tokens (resolved to
 * literals with the rest). On narrow screens the contents move above the text
 * instead of disappearing, and printing drops them.
 */
export const BASE_CSS = `
*, *::before, *::after { box-sizing: border-box; }
html { -webkit-text-size-adjust: 100%; }
body { margin: 0; background: var(--bg); color: var(--fg); font-family: var(--font-ui); font-size: var(--fs); line-height: 1.4; -webkit-font-smoothing: antialiased; }
a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; }
code, kbd, pre { font-family: var(--font-mono); font-size: 0.95em; }
::selection { background: var(--accent-bg); }
.wb-md-toc a { text-decoration: none; }
.wb-export-foot { margin-top: 48px; padding-top: 12px; border-top: 1px solid var(--border); color: var(--fg-subtle); font-size: 12px; }
@container (max-width: 1180px) {
  .wb-md-reader-grid .wb-md-toc { display: block; position: static; grid-column: 2; grid-row: 1; max-width: none; max-height: none; margin-bottom: 28px; padding: 10px 14px; border: 1px solid var(--border); border-radius: var(--radius); background: var(--bg-panel); }
  .wb-md-reader-grid .wb-md-page { grid-row: 2; }
}
@media print {
  body { background: none; }
  .wb-md-toc { display: none !important; }
  .wb-md-reader-grid { display: block; padding: 0; }
  .wb-md-anchor { display: none; }
  pre, blockquote, table, img, .wb-md-mermaid { break-inside: avoid; }
}
`

/**
 * The file. Its CSP lets nothing run and loads nothing but images: its own (data
 * URIs), those the Markdown links to on the web, and relative ones next to the file
 * (`'self'`; opened from disk they are `file:` URLs, which `'self'` does not cover
 * in every browser, hence `file:` too).
 */
export const EXPORT_CSP = "default-src 'none'; img-src 'self' file: data: https: http:; style-src 'unsafe-inline'; font-src data:"

/**
 * The document around the rendered Markdown (`EXPORT_CSP`, no script).
 */
export function documentHtml(o: { title: string; theme: ExportTheme; css: string; body: string; toc: string; source: string; exportedAt: Date }): string {
  const when = o.exportedAt.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
  return `<!doctype html>
<html lang="en" data-theme="${o.theme}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="${EXPORT_CSP}">
<meta name="color-scheme" content="${o.theme}">
<meta name="generator" content="Workbench">
<title>${escapeHtml(o.title)}</title>
<style>
${o.css}
</style>
</head>
<body>
<div class="wb-md-view wb-md-reader">
<div class="wb-md-reader-grid">
<main class="wb-md-page">
<article class="wb-prose">
${o.body}
</article>
<footer class="wb-export-foot">Exported from ${escapeHtml(o.source)} · ${escapeHtml(when)}</footer>
</main>
${o.toc}
</div>
</div>
</body>
</html>
`
}
