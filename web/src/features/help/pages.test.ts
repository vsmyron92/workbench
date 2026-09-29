import { describe, expect, it } from 'vitest'
import { PAGES, parseHelpLink, parsePages, searchPages } from './pages'

describe('help pages', () => {
  it('orders pages by file name and takes the slug and title from them', () => {
    const p = parsePages({ './pages/02-b.md': '# Bee\nx', './pages/01-a.md': '# Ay\ny' })
    expect(p.map((x) => [x.slug, x.title])).toEqual([['a', 'Ay'], ['b', 'Bee']])
  })

  it('ships pages with unique slugs and a title each', () => {
    expect(PAGES.length).toBeGreaterThan(0)
    expect(new Set(PAGES.map((p) => p.slug)).size).toBe(PAGES.length)
    for (const p of PAGES) expect(p.text.startsWith('# ')).toBe(true)
  })

  it('resolves every page link to a page', () => {
    const slugs = new Set(PAGES.map((p) => p.slug))
    for (const p of PAGES) {
      for (const m of p.text.matchAll(/\]\(([^)]+)\)/g)) {
        if (/^https?:/.test(m[1])) continue
        const link = parseHelpLink(m[1])
        expect(link, `${p.slug}: ${m[1]}`).not.toBeNull()
        expect(slugs.has(link!.slug), `${p.slug} links to a missing page ${m[1]}`).toBe(true)
      }
    }
  })

  it('parses help links and ignores others', () => {
    expect(parseHelpLink('agents')).toEqual({ slug: 'agents', anchor: undefined })
    expect(parseHelpLink('remote-access#troubleshooting')).toEqual({ slug: 'remote-access', anchor: 'troubleshooting' })
    expect(parseHelpLink('https://example.com')).toBeNull()
    expect(parseHelpLink('mailto:a@b.c')).toBeNull()
    expect(parseHelpLink('#top')).toBeNull()
  })

  it('finds pages holding every word, title hits first', () => {
    const pages = parsePages({
      './pages/01-a.md': '# Tailscale\nserve it',
      './pages/02-b.md': '# Other\nuse tailscale to serve it',
      './pages/03-c.md': '# Third\nnothing',
    })
    expect(searchPages('tailscale', pages).map((h) => h.page.slug)).toEqual(['a', 'b'])
    expect(searchPages('tailscale serve', pages)).toHaveLength(2)
    expect(searchPages('tailscale zzz', pages)).toEqual([])
    expect(searchPages('  ', pages)).toEqual([])
  })

  it('finds the setup steps a user is likely to look for', () => {
    for (const q of ['tailscale', 'qr code', 'allowed_hosts', 'service install', 'permission']) {
      expect(searchPages(q).length, q).toBeGreaterThan(0)
    }
  })
})
