// 'workspace.home' panel: every card of a scope as a grid (Mr. Mak's home, adapted):
// a pinned section, then one section per category, freshest first. Search digs
// through the archive; the archive toggle shows the rest.

import { useEffect, useMemo, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Archive, LayoutGrid, Pin, Plus, Search, Trash2, X } from 'lucide-react'
import { useProjects } from '@/api/queries'
import type { PanelProps } from '@/shell/types'
import { useUi } from '@/state/store'
import { Button, EmptyState, ErrorBox, IconButton, Loading, showMenu } from '@/ui'
import { type WorkspaceCard, useCards, useTrash } from './api'
import { cardMenu, dragHasPayload, dropOnCard, openCard } from './actions'
import { ALL, categoryLabel, groupCards, HOME, relativeDay, visibleCards } from './logic'
import { CardThumb, CategoryIcon, KindIcon } from './parts'
import { useWsPrefs, useWsUi } from './store'
import { TrashView } from './TrashView'

export interface HomeParams {
  scope?: string
  /** Show the Workspace trash instead of the cards. */
  view?: 'trash'
}

export function CardTile({ card, showScope }: { card: WorkspaceCard; showScope?: boolean }) {
  const qc = useQueryClient()
  const [over, setOver] = useState(false)
  const kinds = [...new Set(card.steps.map((s) => s.kind))].slice(0, 4)
  return (
    <div
      role="button"
      tabIndex={0}
      className={['ws-tile', card.pinned && 'pinned', card.archived && 'archived', over && 'drop'].filter(Boolean).join(' ')}
      data-card={card.id}
      onClick={() => openCard(card.scope, card.id, card.title)}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          openCard(card.scope, card.id, card.title)
        }
      }}
      onContextMenu={(e) => showMenu(e, cardMenu(qc, card))}
      onDragOver={(e) => {
        if (!dragHasPayload(e.dataTransfer)) return
        e.preventDefault()
        e.dataTransfer.dropEffect = 'copy'
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
      <CardThumb card={card} className="ws-tile-thumb" />
      <div className="ws-tile-body">
        <div className="ws-tile-chips">
          <span className="ws-chip">
            <CategoryIcon category={card.category} size={12} />
            {categoryLabel(card.category)}
          </span>
          {card.pinned && (
            <span className="ws-chip accent">
              <Pin size={11} /> pinned
            </span>
          )}
          {card.origin === 'repo' && (
            <span className="ws-chip" title="From the project's workspace/workspace.json">
              repo
            </span>
          )}
          <span className="spacer" />
          <span className={`wb-dot ${card.archived ? '' : card.status === 'done' ? 'accent' : 'success'}`} title={card.archived ? 'archived' : card.status} />
        </div>
        <div className="ws-tile-title">{card.title}</div>
        {card.description && <div className="ws-tile-desc">{card.description}</div>}
        <div className="ws-tile-foot">
          <span className="ws-tile-kinds">
            {kinds.map((k) => (
              <KindIcon key={k} kind={k} size={12} />
            ))}
          </span>
          <span>
            {card.steps.length} step{card.steps.length === 1 ? '' : 's'}
          </span>
          <span>·</span>
          <span title={new Date(card.touchedAt).toLocaleString()}>{relativeDay(card.touchedAt)}</span>
          {showScope && (
            <>
              <span>·</span>
              <span className="wb-ellipsis">{card.scopeName}</span>
            </>
          )}
        </div>
      </div>
    </div>
  )
}

function Section({ title, icon, cards, showScope }: { title: string; icon: React.ReactNode; cards: WorkspaceCard[]; showScope: boolean }) {
  return (
    <section className="ws-section">
      <div className="ws-section-head">
        {icon}
        <span className="ws-section-title">{title}</span>
        <span className="ws-section-count">{cards.length}</span>
      </div>
      <div className="ws-grid">
        {cards.map((c) => (
          <CardTile key={`${c.scope}:${c.id}`} card={c} showScope={showScope} />
        ))}
      </div>
    </section>
  )
}

export function HomePanel({ params, setParams, setTitle, active }: PanelProps<HomeParams>) {
  const projectId = useUi((s) => s.projectId)
  const { data: projects } = useProjects()
  const scope = params.scope || projectId || HOME
  const trashView = params.view === 'trash'
  const { data, error, isLoading, refetch } = useCards(scope)
  const trash = useTrash(scope)
  const trashCount = trash.data?.items.length ?? 0
  const [query, setQuery] = useState('')
  const showArchived = useWsPrefs((s) => s.showArchived)
  const setShowArchived = useWsPrefs((s) => s.setShowArchived)
  const searchRef = useRef<HTMLInputElement>(null)
  const rootRef = useRef<HTMLDivElement>(null)

  useEffect(() => setTitle('Workspace'), [setTitle])

  // "/" focuses search while this panel is the active one, unless the key belongs to
  // something else: a widget that handled it, or focus in another part of the
  // window (the files tree's speed search takes "/" too). Focus on the page, in
  // this panel or on the dock container holding it counts as ours.
  useEffect(() => {
    if (!active) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== '/' || e.ctrlKey || e.metaKey || e.altKey || e.defaultPrevented) return
      const focus = document.activeElement
      const root = rootRef.current
      if (focus && focus !== document.body && !(root && (root.contains(focus) || focus.contains(root)))) return
      const t = e.target as HTMLElement | null
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.isContentEditable || t.closest('.xterm, .monaco-editor'))) return
      e.preventDefault()
      searchRef.current?.focus()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [active])

  const scopeOptions = useMemo(() => {
    const ids = [projectId, scope !== HOME && scope !== ALL ? scope : null].filter((x, i, a): x is string => !!x && a.indexOf(x) === i)
    return [
      ...ids.map((id) => ({ id, label: projects?.find((p) => p.id === id)?.name ?? id })),
      { id: HOME, label: 'Home' },
      { id: ALL, label: 'All' },
    ]
  }, [projectId, projects, scope])

  const cards = data?.cards ?? []
  const searching = query.trim().length > 0
  const visible = visibleCards(cards, query, showArchived)
  const archivedCount = cards.filter((c) => c.archived).length
  const { pinned, groups } = groupCards(visible, searching || showArchived)
  const activeCount = cards.filter((c) => !c.archived).length
  const categories = new Set(cards.map((c) => c.category || 'other')).size
  const newCard = () => useWsUi.getState().openNewCard(scope === ALL ? projectId ?? HOME : scope)

  return (
    <div className="wb-fill ws-home" ref={rootRef}>
      <div className="ws-home-bar">
        <span className="ws-home-title">
          <LayoutGrid size={16} />
          Workspace
        </span>
        <div className="ws-seg" role="tablist" aria-label="Scope">
          {scopeOptions.map((o) => (
            <button key={o.id} role="tab" aria-selected={o.id === scope} className={o.id === scope ? 'active' : ''} onClick={() => setParams({ ...params, scope: o.id })}>
              {o.label}
            </button>
          ))}
        </div>
        <span className="spacer" />
        <label className="ws-search">
          <Search size={14} />
          <input
            ref={searchRef}
            value={query}
            placeholder={trashView ? 'Search the trash  ( / )' : 'Search cards  ( / )'}
            aria-label={trashView ? 'Search the trash' : 'Search cards'}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape') {
                setQuery('')
                e.currentTarget.blur()
              }
            }}
          />
          {query && <IconButton icon={X} size="small" label="Clear search" onClick={() => setQuery('')} />}
        </label>
        {!trashView && (
          <Button size="small" variant={showArchived ? 'primary' : 'default'} icon={Archive} onClick={() => setShowArchived(!showArchived)} title="Cards archived by hand or untouched for 7 days">
            Archive{archivedCount ? ` (${archivedCount})` : ''}
          </Button>
        )}
        <Button
          size="small"
          variant={trashView ? 'primary' : 'default'}
          icon={trashView ? LayoutGrid : Trash2}
          onClick={() => setParams({ ...params, view: trashView ? undefined : 'trash' })}
          title={trashView ? 'Back to the cards' : 'Deleted cards, restorable until you empty the trash'}
        >
          {trashView ? 'Cards' : `Trash${trashCount ? ` (${trashCount})` : ''}`}
        </Button>
        <Button size="small" variant="primary" icon={Plus} onClick={newCard}>
          New card
        </Button>
      </div>
      <div className="wb-scroll ws-home-scroll">
        {trashView ? (
          <div className="ws-home-inner">
            <TrashView scope={scope} query={query} />
          </div>
        ) : error ? (
          <ErrorBox error={error} onRetry={() => void refetch()} />
        ) : isLoading ? (
          <Loading label="Loading cards…" />
        ) : (
          <div className="ws-home-inner">
            <div className="ws-home-sub">
              <b>{activeCount}</b> active · <b>{cards.length}</b> card{cards.length === 1 ? '' : 's'} · <b>{categories}</b> categor{categories === 1 ? 'y' : 'ies'}
              {searching && (
                <span>
                  {' '}
                  · {visible.length} match{visible.length === 1 ? '' : 'es'} for “{query.trim()}”, archive included
                </span>
              )}
            </div>
            {data?.warnings.map((w) => (
              <div key={w} className="ws-warning">
                {w}
              </div>
            ))}
            {pinned.length > 0 && <Section title="Pinned" icon={<Pin size={14} />} cards={pinned} showScope={scope === ALL} />}
            {groups.map((g) => (
              <Section
                key={g.key}
                title={g.label}
                icon={g.key === '__date' ? <Archive size={14} /> : <CategoryIcon category={g.key} size={14} />}
                cards={g.cards}
                showScope={scope === ALL}
              />
            ))}
            {visible.length === 0 &&
              (searching ? (
                <EmptyState icon={Search} title="No cards match">
                  Search covers titles, descriptions, categories and the archive.
                </EmptyState>
              ) : cards.length > 0 ? (
                <EmptyState icon={Archive} title="Everything here is archived" action={<Button size="small" onClick={() => setShowArchived(true)}>Show the archive</Button>} />
              ) : (
                <EmptyState
                  icon={LayoutGrid}
                  title="No cards yet"
                  action={
                    <Button size="small" variant="primary" icon={Plus} onClick={newCard}>
                      New card
                    </Button>
                  }
                >
                  Cards collect deliverables: reports, documents, image sets, 3D comparisons. Agents add them with the
                  workspace_create_card tool; you can drop files from the project tree onto a card.
                </EmptyState>
              ))}
          </div>
        )}
      </div>
    </div>
  )
}

export default HomePanel
