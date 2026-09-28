// People search for @mentions (Confluence `user.fullname ~ "…"`), shared by the rich
// editor and the comment composer, and the popup list both show.

import { useEffect, useState, type CSSProperties } from 'react'
import { createPortal } from 'react-dom'
import { useQuery } from '@tanstack/react-query'
import { FileText } from 'lucide-react'
import { Spinner } from '@/ui'
import { confluenceApi, qk, type SearchHit, type UserHit } from '../api'
import { initials } from '../links'

export function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value)
  useEffect(() => {
    const t = window.setTimeout(() => setV(value), ms)
    return () => window.clearTimeout(t)
  }, [value, ms])
  return v
}

/** People whose name starts with `query` (debounced; nothing for an empty query). */
export function useUserSearch(projectId: string | null, query: string | null) {
  const q = useDebounced((query ?? '').trim(), 180)
  return useQuery({
    queryKey: qk.users(projectId, q),
    queryFn: ({ signal }) => confluenceApi.users(projectId, q, signal),
    enabled: query !== null && q.length >= 1,
    staleTime: 5 * 60_000,
    retry: false,
  })
}

/** CQL for a page picker: pages whose title starts with (or contains) the text. */
export function titleCql(text: string): string {
  const t = text.trim().replace(/[\\"]/g, (c) => `\\${c}`)
  return `type = page AND (title ~ "${t}*" OR title ~ "${t}")`
}

/** Pages for a picker, by title (debounced; nothing for an empty query). */
export function usePageSearch(projectId: string | null, query: string | null) {
  const q = useDebounced((query ?? '').trim(), 200)
  return useQuery({
    queryKey: ['confluence', 'pick', projectId ?? null, q],
    queryFn: () => confluenceApi.searchCql(projectId, titleCql(q), 12),
    enabled: query !== null && q.length >= 1,
    staleTime: 60_000,
    retry: false,
  })
}

export type Suggestion = { kind: 'user'; user: UserHit } | { kind: 'page'; page: SearchHit }

/** A floating list of suggestions at a screen position (fixed), keyboard-driven by its owner. */
export function SuggestMenu({
  at,
  items,
  active,
  loading,
  empty,
  onPick,
  onHover,
}: {
  at: { left: number; top: number; bottom: number }
  items: Suggestion[]
  active: number
  loading: boolean
  empty: string
  onPick: (s: Suggestion) => void
  onHover: (i: number) => void
}) {
  // Below the caret, or above it when there is no room.
  const below = at.bottom + 240 < window.innerHeight
  const style: CSSProperties = {
    left: Math.max(8, Math.min(at.left, window.innerWidth - 300)),
    ...(below ? { top: at.bottom + 4 } : { bottom: window.innerHeight - at.top + 4 }),
  }
  return createPortal(
    <div className="cf-suggest" style={style} role="listbox" onMouseDown={(e) => e.preventDefault()}>
      {items.map((s, i) => (
        <div
          key={s.kind === 'user' ? s.user.accountId : s.page.id}
          role="option"
          aria-selected={i === active}
          className={i === active ? 'item active' : 'item'}
          onMouseEnter={() => onHover(i)}
          onClick={() => onPick(s)}
        >
          {s.kind === 'user' ? (
            <>
              <span className="atl-avatar">{initials(s.user.displayName)}</span>
              <span className="wb-ellipsis">{s.user.displayName}</span>
              {s.user.email && <span className="hint wb-ellipsis">{s.user.email}</span>}
            </>
          ) : (
            <>
              <FileText size={13} className="wb-muted" />
              <span className="wb-ellipsis">{s.page.title}</span>
              <span className="hint">{s.page.spaceKey ?? s.page.spaceName}</span>
            </>
          )}
        </div>
      ))}
      {!items.length && <div className="empty">{loading ? <Spinner size={11} /> : empty}</div>}
    </div>,
    document.body,
  )
}
