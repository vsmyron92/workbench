// Search Everywhere's Symbols tab: `workspace/symbol` from the project's language
// servers. It never turns code intelligence on; for a project without it, the tab
// says so instead.

import type { SearchItem, SearchProvider } from '@/shell/types'
import type { LspSymbolInformation } from './api'
import { lsp } from './client'
import { displayPath, shortenPath } from './convert'
import { openLocation } from './nav'
import { SymbolIcon } from './SymbolIcon'

/** Letters of `name` matching `query` in order (case-insensitive), for highlighting. */
export function matchPositions(name: string, query: string): number[] {
  const q = query.toLowerCase().replace(/\s+/g, '')
  const n = name.toLowerCase()
  const out: number[] = []
  let j = 0
  for (let i = 0; i < n.length && j < q.length; i++) {
    if (n[i] === q[j]) {
      out.push(i)
      j++
    }
  }
  return j === q.length ? out : []
}

export const symbolsSearch: SearchProvider = {
  id: 'symbols',
  title: 'Symbols',
  order: 20,
  inAll: 6,
  when: (c) => !!c.projectId,
  search: async (q, ctx, signal) => {
    const conn = await lsp.connect(ctx.projectId!)
    if (!conn) return []
    await conn.whenOpen()
    const r = await conn.request<LspSymbolInformation[] | null>('workspace/symbol', { query: q }, { signal })
    return (r.result ?? [])
      .filter((s) => s.location && 'uri' in s.location)
      .slice(0, 100)
      .map<SearchItem>((s, i) => {
        const range = 'range' in s.location ? s.location.range : undefined
        const path = shortenPath(displayPath(s.location.uri))
        return {
          key: `${i}:${s.name}:${s.location.uri}`,
          title: s.name,
          highlight: matchPositions(s.name, q),
          detail: [s.containerName, range ? `${path}:${range.start.line + 1}` : path].filter(Boolean).join(' · '),
          icon: <SymbolIcon kind={s.kind} />,
          run: ({ side }) => openLocation(s.location.uri, range, { side }),
        }
      })
  },
  hint: (ctx) =>
    ctx.projectId && lsp.isEnabled(ctx.projectId) === false ? 'Symbols come from language servers: turn on code intelligence for this project (Code Intelligence… in the palette).' : null,
}
