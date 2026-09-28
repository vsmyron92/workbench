// Pick a Confluence page by title (search as you type), for Move, Copy and links.

import { useEffect, useState, type KeyboardEvent, type ReactNode } from 'react'
import { FileText, Search } from 'lucide-react'
import { ErrorBox, Input, Spinner } from '@/ui'
import type { SearchHit } from '../api'
import { usePageSearch } from './people'

export function PagePicker({
  projectId,
  value,
  onChange,
  exclude,
  placeholder = 'Search pages by title',
  autoFocus,
  onEnter,
  extra,
  initialQuery = '',
  onQueryEnter,
  onQueryChange,
  hideResults,
}: {
  projectId: string | null
  value: SearchHit | null
  onChange: (hit: SearchHit | null) => void
  /** Page ids not to offer (the page itself). */
  exclude?: string[]
  placeholder?: string
  autoFocus?: boolean
  /** Enter with a page chosen. */
  onEnter?: (hit: SearchHit) => void
  /** Rendered after the input (e.g. "link to URL" hints). */
  extra?: (query: string) => ReactNode
  initialQuery?: string
  /** Enter with this query; return true when handled (e.g. a URL). */
  onQueryEnter?: (query: string) => boolean
  onQueryChange?: (query: string) => void
  /** Hide the page results (e.g. while the input is a web address). */
  hideResults?: boolean
}) {
  const [query, setQuery] = useState(initialQuery)
  const q = usePageSearch(projectId, query.trim() ? query : null)
  const hits = (q.data?.results ?? []).filter((h) => h.type === 'page' && !exclude?.includes(h.id))
  const [active, setActive] = useState(0)
  useEffect(() => setActive(0), [q.data])

  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault()
      if (!hits.length) return
      const next = (active + (e.key === 'ArrowDown' ? 1 : -1) + hits.length) % hits.length
      setActive(next)
      onChange(hits[next])
    } else if (e.key === 'Enter') {
      if (onQueryEnter?.(query)) {
        e.preventDefault()
        return
      }
      const pick = value ?? hits[active]
      if (pick) {
        e.preventDefault()
        onChange(pick)
        onEnter?.(pick)
      }
    }
  }

  return (
    <div className="cf-picker">
      <div className="atl-search">
        <Search size={13} className="icon" />
        <Input
          small
          autoFocus={autoFocus}
          value={query}
          placeholder={placeholder}
          onChange={(e) => {
            setQuery(e.target.value)
            onQueryChange?.(e.target.value)
            onChange(null)
          }}
          onKeyDown={onKey}
          aria-label="Page title"
        />
        {q.isFetching && <Spinner size={11} />}
      </div>
      {extra?.(query)}
      {hideResults ? null : q.error ? (
        <ErrorBox error={q.error} />
      ) : (
        <div className="cf-picker-list" role="listbox">
          {hits.map((h, i) => (
            <div
              key={h.id}
              role="option"
              aria-selected={value?.id === h.id}
              className={['item', value?.id === h.id && 'selected', i === active && !value && 'active'].filter(Boolean).join(' ')}
              onClick={() => onChange(h)}
              onDoubleClick={() => onEnter?.(h)}
              title={h.title}
            >
              <FileText size={13} className="wb-muted" />
              <span className="wb-ellipsis">{h.title}</span>
              <span className="hint">{h.spaceKey ?? h.spaceName}</span>
            </div>
          ))}
          {query.trim() && !q.isFetching && q.data && !hits.length && <div className="empty">No pages match “{query.trim()}”</div>}
          {!query.trim() && <div className="empty">Type part of a page title</div>}
        </div>
      )}
    </div>
  )
}
