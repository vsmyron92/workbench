// The 'help' panel: a page list with search on the left, one page at a time on the right.
// `HelpView` is also the phone's Help (More tab), which shows the list or a page, not both.

import { useEffect, useMemo, useRef, useState } from 'react'
import { ArrowLeft, BookOpen, Search } from 'lucide-react'
import type { PanelProps } from '@/shell/types'
import { Input, Markdown } from '@/ui'
import { FIRST_PAGE, PAGES, pageBySlug, parseHelpLink, searchPages } from './pages'
import './help.css'

export interface HelpParams {
  page?: string
}

export function HelpPanel({ params, setParams, setTitle }: PanelProps<HelpParams>) {
  const page = pageBySlug(params.page) ?? pageBySlug(FIRST_PAGE)
  useEffect(() => setTitle(page ? `Help: ${page.title}` : 'Help'), [page, setTitle])
  return <HelpView slug={page?.slug} onPage={(slug) => setParams({ page: slug })} />
}

/** `compact` (a phone): the list and the page take turns. */
export function HelpView({ slug, onPage, compact = false }: { slug: string | undefined; onPage: (slug: string | undefined) => void; compact?: boolean }) {
  const [query, setQuery] = useState('')
  const hits = useMemo(() => searchPages(query), [query])
  const searching = query.trim().length > 0
  const page = pageBySlug(slug)
  const main = useRef<HTMLDivElement>(null)
  const pending = useRef<string | undefined>(undefined)

  // A new page starts at the top, or at the heading a link named.
  useEffect(() => {
    const el = main.current
    if (!el) return
    const anchor = pending.current
    pending.current = undefined
    const target = anchor && el.querySelector(`[id="${CSS.escape(anchor)}"], [id="user-content-${CSS.escape(anchor)}"]`)
    if (target) target.scrollIntoView({ block: 'start' })
    else el.scrollTo({ top: 0 })
  }, [slug])

  const open = (s: string, anchor?: string) => {
    pending.current = anchor
    setQuery('')
    onPage(s)
  }

  const onLinkClick = (href: string) => {
    const link = parseHelpLink(href)
    if (!link || !pageBySlug(link.slug)) return false
    open(link.slug, link.anchor)
    return true
  }

  const list = (
    <nav className="wb-help-nav" aria-label="Help pages">
      <div className="wb-help-search">
        <Search size={14} />
        <Input small value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search Help" aria-label="Search Help" />
      </div>
      {searching ? (
        hits.length === 0 ? (
          <div className="wb-help-empty">Nothing in Help matches “{query.trim()}”.</div>
        ) : (
          hits.map((h) => (
            <button key={h.page.slug} className="wb-help-hit" onClick={() => open(h.page.slug)}>
              <span className="wb-help-hit-title">{h.page.title}</span>
              <span className="wb-help-hit-snippet">{h.snippet}</span>
            </button>
          ))
        )
      ) : (
        PAGES.map((p) => (
          <button key={p.slug} className={p.slug === slug ? 'wb-help-item active' : 'wb-help-item'} onClick={() => open(p.slug)}>
            <BookOpen size={14} />
            <span>{p.title}</span>
          </button>
        ))
      )}
    </nav>
  )

  // On a phone the list is the front page; a page (or a search) replaces it.
  const showList = !compact || !page || searching
  const showPage = !!page && (!compact || !searching)

  return (
    <div className={compact ? 'wb-help compact' : 'wb-help'}>
      {showList && list}
      {showPage && (
        <div className="wb-help-main" ref={main}>
          {compact && (
            <button className="wb-help-back" onClick={() => onPage(undefined)}>
              <ArrowLeft size={14} /> All pages
            </button>
          )}
          <div className="wb-help-page">
            <Markdown key={page.slug} text={page.text} onLinkClick={onLinkClick} />
          </div>
        </div>
      )}
    </div>
  )
}

export default HelpPanel
