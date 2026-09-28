// 'workspace' tool window: a compact card list for the current project (or Home),
// pinned first, grouped by category like Mr. Mak's sidebar. Click opens the card;
// files dropped on a row become steps.

import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Archive, LayoutGrid, Plus, Search, X } from 'lucide-react'
import { EmptyState, ErrorBox, IconButton, Loading, Section, showMenu } from '@/ui'
import { type WorkspaceCard, useCards } from './api'
import { cardMenu, dragHasPayload, dropOnCard, openCard, openHome } from './actions'
import { groupCards, HOME, relativeDay, visibleCards } from './logic'
import { CardThumb } from './parts'
import { useWsPrefs, useWsUi } from './store'

function Row({ card }: { card: WorkspaceCard }) {
  const qc = useQueryClient()
  const [over, setOver] = useState(false)
  return (
    <div
      className={`wb-list-row ws-row${card.archived ? ' archived' : ''}${over ? ' drop' : ''}`}
      onClick={() => openCard(card.scope, card.id, card.title)}
      onContextMenu={(e) => showMenu(e, cardMenu(qc, card))}
      onDragOver={(e) => {
        if (!dragHasPayload(e.dataTransfer)) return
        e.preventDefault()
        setOver(true)
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(e) => {
        e.preventDefault()
        setOver(false)
        void dropOnCard(qc, card, e.dataTransfer)
      }}
      title={card.description || card.title}
    >
      <CardThumb card={card} size={20} />
      <span className="wb-grow wb-ellipsis">{card.title}</span>
      <span className="wb-xs wb-subtle">{relativeDay(card.touchedAt)}</span>
      <span className={`wb-dot ${card.archived ? '' : card.status === 'done' ? 'accent' : 'success'}`} title={card.archived ? 'archived' : card.status} />
    </div>
  )
}

export function WorkspaceToolWindow({ projectId }: { projectId: string | null }) {
  const listScope = useWsPrefs((s) => s.listScope)
  const setListScope = useWsPrefs((s) => s.setListScope)
  const showArchived = useWsPrefs((s) => s.showArchived)
  const setShowArchived = useWsPrefs((s) => s.setShowArchived)
  const scope = listScope === 'home' || !projectId ? HOME : projectId
  const { data, error, isLoading, refetch } = useCards(scope)
  const [query, setQuery] = useState('')
  const cards = data?.cards ?? []
  const searching = query.trim().length > 0
  const visible = visibleCards(cards, query, showArchived)
  const { pinned, groups } = groupCards(visible, searching || showArchived)
  const archivedCount = cards.filter((c) => c.archived).length

  return (
    <div className="wb-fill ws-tw">
      <div className="wb-toolbar">
        <div className="ws-seg small" role="tablist" aria-label="Scope">
          {projectId && (
            <button role="tab" aria-selected={scope !== HOME} className={scope !== HOME ? 'active' : ''} onClick={() => setListScope('project')}>
              Project
            </button>
          )}
          <button role="tab" aria-selected={scope === HOME} className={scope === HOME ? 'active' : ''} onClick={() => setListScope('home')}>
            Home
          </button>
        </div>
        <span className="spacer" />
        <IconButton icon={Plus} size="small" label="New card" onClick={() => useWsUi.getState().openNewCard(scope)} />
        <IconButton icon={LayoutGrid} size="small" label="Open Workspace home" onClick={() => openHome(scope)} />
      </div>
      <label className="ws-search tw">
        <Search size={13} />
        <input value={query} placeholder="Search cards" aria-label="Search cards" onChange={(e) => setQuery(e.target.value)} onKeyDown={(e) => e.key === 'Escape' && setQuery('')} />
        {query && <IconButton icon={X} size="small" label="Clear search" onClick={() => setQuery('')} />}
      </label>
      <div className="wb-scroll">
        {error ? (
          <ErrorBox error={error} onRetry={() => void refetch()} />
        ) : isLoading ? (
          <Loading />
        ) : !visible.length ? (
          <EmptyState icon={LayoutGrid} title={searching ? 'No cards match' : cards.length ? 'Everything is archived' : 'No cards yet'}>
            {!searching && !cards.length && 'Agents add deliverables here; drop files from the project tree onto a card.'}
          </EmptyState>
        ) : (
          <>
            {pinned.length > 0 && (
              <Section title="Pinned" count={pinned.length}>
                {pinned.map((c) => (
                  <Row key={c.id} card={c} />
                ))}
              </Section>
            )}
            {groups.map((g) => (
              <Section key={g.key} title={g.label} count={g.cards.length}>
                {g.cards.map((c) => (
                  <Row key={c.id} card={c} />
                ))}
              </Section>
            ))}
          </>
        )}
        {archivedCount > 0 && !searching && (
          <button className="ws-archive-toggle" onClick={() => setShowArchived(!showArchived)}>
            <Archive size={13} />
            {showArchived ? `Hide archive (${archivedCount})` : `Show archive (${archivedCount})`}
          </button>
        )}
      </div>
    </div>
  )
}

export default WorkspaceToolWindow
