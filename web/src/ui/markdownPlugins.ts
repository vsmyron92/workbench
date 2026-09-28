// Pure helpers behind the Markdown renderer (ui/Markdown.tsx), kept free of React
// so they can be unit-tested.

/** A HAST node, as much of it as these helpers need. */
export interface HastNode {
  type: string
  tagName?: string
  value?: string
  properties?: Record<string, unknown>
  children?: HastNode[]
}

/**
 * Split a leading YAML front-matter block (`---` … `---` at the very start) from
 * the body. Renderers show it as a collapsible block instead of a stray rule and
 * a paragraph of `key: value` lines.
 */
export function splitFrontmatter(text: string): { frontmatter: string | null; body: string } {
  const m = /^﻿?---[ \t]*\r?\n([\s\S]*?)\r?\n(?:---|\.\.\.)[ \t]*(?:\r?\n|$)/.exec(text)
  if (!m) return { frontmatter: null, body: text }
  return { frontmatter: m[1], body: text.slice(m[0].length) }
}

/** Concatenated text of a HAST subtree (for copy buttons and labels). */
export function hastText(node: HastNode | undefined): string {
  if (!node) return ''
  if (node.type === 'text') return node.value ?? ''
  return (node.children ?? []).map(hastText).join('')
}

export const ALERT_KINDS = ['note', 'tip', 'important', 'warning', 'caution'] as const
export type AlertKind = (typeof ALERT_KINDS)[number]

const ALERT_MARK = /^\s*\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\][ \t]*(?:\r?\n)?/i

/**
 * GitHub's alert blockquotes: `> [!NOTE]` (and TIP, IMPORTANT, WARNING, CAUTION)
 * on the first line turns the blockquote into a callout. Runs after sanitizing,
 * so the class it adds is ours, not the document's.
 */
export function rehypeAlerts() {
  return (tree: HastNode) => {
    walk(tree, (node) => {
      if (node.type !== 'element' || node.tagName !== 'blockquote') return
      const first = (node.children ?? []).find((c) => c.type === 'element')
      if (!first || first.tagName !== 'p') return
      const text = (first.children ?? [])[0]
      if (!text || text.type !== 'text' || typeof text.value !== 'string') return
      const m = ALERT_MARK.exec(text.value)
      if (!m) return
      const kind = m[1].toLowerCase() as AlertKind
      text.value = text.value.slice(m[0].length)
      // Drop the soft break GitHub leaves after the marker line.
      if (!text.value && first.children && first.children[1]?.type === 'element' && first.children[1].tagName === 'br') {
        first.children.splice(1, 1)
      }
      node.properties = { ...node.properties, className: ['wb-alert', `wb-alert-${kind}`], dataAlert: kind }
    })
  }
}

function walk(node: HastNode, fn: (n: HastNode) => void) {
  fn(node)
  for (const c of node.children ?? []) walk(c, fn)
}

/** Title shown on an alert callout. */
export function alertTitle(kind: string): string {
  return kind.charAt(0).toUpperCase() + kind.slice(1)
}

/**
 * Ids an in-document `#anchor` may resolve to: as written, a heading slug
 * (`md-`, from rehype-slug) or an id or name from the document itself, which
 * the sanitizer prefixed with `user-content-` (footnotes, `<a name>`).
 */
export function anchorIds(anchor: string): string[] {
  let a = anchor.replace(/^#/, '')
  try {
    a = decodeURIComponent(a)
  } catch {
    /* keep it as written */
  }
  if (!a) return []
  return [...new Set([a, `md-${a}`, `md-${a.toLowerCase()}`, `user-content-${a}`])]
}

/** The element under `root` an in-document `#anchor` points at, if any. */
export function findAnchor(root: ParentNode, anchor: string): Element | null {
  for (const id of anchorIds(anchor)) {
    const q = CSS.escape(id)
    const el = root.querySelector(`[id="${q}"], a[name="${q}"]`)
    if (el) return el
  }
  return null
}

/** `language-rust` (possibly among other classes) → `rust`. */
export function codeLanguage(className: unknown): string | null {
  const list = Array.isArray(className) ? className : typeof className === 'string' ? className.split(/\s+/) : []
  for (const c of list) {
    if (typeof c === 'string' && c.startsWith('language-')) return c.slice('language-'.length) || null
  }
  return null
}
