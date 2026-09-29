// The user documentation: markdown files in ./pages, bundled into the web build so Help
// works offline and on a phone. File names order the pages (`01-getting-started.md`); the
// part after the number is the page's slug, and its first `# ` heading is its title.
// Pages link to each other by slug, `[text](agents)` or `[text](agents#heading-id)`: a relative
// link, which the Markdown renderer hands to `onLinkClick` (an unknown scheme would be stripped).

export interface HelpPage {
  slug: string
  title: string
  text: string
  /** Lower-cased title + text, for search. */
  haystack: string
}

const files = import.meta.glob('./pages/*.md', { query: '?raw', import: 'default', eager: true }) as Record<string, string>

export function parsePages(entries: Record<string, string>): HelpPage[] {
  return Object.entries(entries)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([path, text]) => {
      const slug = (path.split('/').pop() ?? path).replace(/\.md$/, '').replace(/^\d+-/, '')
      const title = /^#\s+(.+)$/m.exec(text)?.[1].trim() ?? slug
      return { slug, title, text, haystack: `${title}\n${text}`.toLowerCase() }
    })
}

export const PAGES: HelpPage[] = parsePages(files)
export const FIRST_PAGE = PAGES[0]?.slug ?? ''

export function pageBySlug(slug: string | undefined, pages: HelpPage[] = PAGES): HelpPage | undefined {
  return pages.find((p) => p.slug === slug)
}

/** `remote-access#troubleshooting` → its slug and heading id; anything else is not a Help link. */
export function parseHelpLink(href: string): { slug: string; anchor?: string } | null {
  const m = /^([a-z0-9-]+)(?:#(.+))?$/.exec(href)
  return m ? { slug: m[1], anchor: m[2] } : null
}

export interface HelpHit {
  page: HelpPage
  /** A line of the page that matched, for context. */
  snippet: string
  score: number
}

const words = (q: string) => q.toLowerCase().split(/\s+/).filter(Boolean)

/** Pages containing every word of the query, best first: title hits outrank body hits. */
export function searchPages(query: string, pages: HelpPage[] = PAGES): HelpHit[] {
  const terms = words(query)
  if (!terms.length) return []
  const hits: HelpHit[] = []
  for (const page of pages) {
    if (!terms.every((t) => page.haystack.includes(t))) continue
    const title = page.title.toLowerCase()
    let score = 0
    for (const t of terms) {
      if (title.includes(t)) score += 10
      score += Math.min(page.haystack.split(t).length - 1, 5)
    }
    const line = page.text.split('\n').find((l) => terms.some((t) => l.toLowerCase().includes(t))) ?? ''
    hits.push({ page, score, snippet: line.replace(/[#>*`|]/g, '').trim().slice(0, 140) })
  }
  return hits.sort((a, b) => b.score - a.score)
}
