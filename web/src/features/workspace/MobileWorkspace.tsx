// Phone tab 'workspace': the card list, then one card with its steps as chips and
// the step's viewer (reports, documents, images and galleries read well on a phone).

import { useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { ChevronLeft, LayoutGrid, Pin, PinOff, Search, X } from 'lucide-react'
import { ApiError } from '@/api/client'
import { EmptyState, ErrorBox, IconButton, Loading, showMenuAt } from '@/ui'
import { useCard, useCards } from './api'
import { patchCard } from './actions'
import { categoryLabel, HOME, listedScope, relativeDay, resolveStep, SANDBOX, visibleCards } from './logic'
import { CardThumb, KindIcon, StatusBadge } from './parts'
import { type MobileSelection, useWsPrefs, useWsUi } from './store'
import { StepViewer } from './viewers/StepViewer'

function MobileCard({ sel, onBack }: { sel: MobileSelection; onBack: () => void }) {
  const qc = useQueryClient()
  const { data: card, error, isLoading, refetch } = useCard(sel.scope, sel.cardId)
  const setMobile = useWsUi((s) => s.setMobile)
  const chipsRef = useRef<HTMLDivElement>(null)
  const shown = card ? resolveStep(card.steps.length, sel.step, card.defaultIndex) : -1

  // The card opens on its last step by default: bring that chip into view. Only the
  // chip row scrolls (scrollIntoView would move the page too).
  useEffect(() => {
    const row = chipsRef.current
    const chip = row?.querySelector<HTMLElement>('button.active')
    if (!row || !chip) return
    const left = chip.offsetLeft
    const right = left + chip.offsetWidth
    if (left < row.scrollLeft) row.scrollLeft = Math.max(0, left - 10)
    else if (right > row.scrollLeft + row.clientWidth) row.scrollLeft = right - row.clientWidth + 10
  }, [shown, card?.steps.length])

  if (error)
    return (
      <div className="wb-fill">
        <div className="ws-m-head">
          <IconButton icon={ChevronLeft} label="Back" onClick={onBack} />
        </div>
        {error instanceof ApiError && error.status === 404 ? <EmptyState title="This card no longer exists" /> : <ErrorBox error={error} onRetry={() => void refetch()} />}
      </div>
    )
  if (isLoading || !card) return <Loading />
  const index = resolveStep(card.steps.length, sel.step, card.defaultIndex)
  const step = card.steps[index]
  return (
    <div className="wb-fill ws-m-card">
      <div className="ws-m-head">
        <IconButton icon={ChevronLeft} label="Back to the cards" onClick={onBack} />
        <div className="wb-grow" style={{ minWidth: 0 }}>
          <div className="ws-m-title wb-ellipsis">{card.title}</div>
          <div className="wb-xs wb-muted wb-ellipsis">
            {categoryLabel(card.category)} · {relativeDay(card.touchedAt)}
          </div>
        </div>
        <button
          className="ws-m-status"
          onClick={(e) =>
            showMenuAt(e.currentTarget, [
              { label: 'Active', run: () => void patchCard(qc, card, { status: 'active' }) },
              { label: 'Done', run: () => void patchCard(qc, card, { status: 'done' }) },
              { label: 'Archived', run: () => void patchCard(qc, card, { status: 'archived' }) },
            ])
          }
        >
          <StatusBadge card={card} />
        </button>
        <IconButton icon={card.pinned ? PinOff : Pin} label={card.pinned ? 'Unpin' : 'Pin'} active={card.pinned} onClick={() => void patchCard(qc, card, { pinned: !card.pinned })} />
      </div>
      {card.steps.length > 1 && (
        <div className="ws-m-steps" ref={chipsRef}>
          {card.steps.map((s) => (
            <button key={`${s.index}:${s.path}`} className={s.index === index ? 'active' : ''} onClick={() => setMobile({ ...sel, step: s.index })}>
              <KindIcon kind={s.kind} size={12} />
              {s.name}
            </button>
          ))}
        </div>
      )}
      <div className="ws-m-view">
        {step ? (
          <StepViewer card={card} target={{ path: step.path, name: step.name, kind: step.kind, exists: step.exists, size: step.size, mtime: step.mtime }} />
        ) : (
          <EmptyState title="No steps yet">{card.description}</EmptyState>
        )}
      </div>
    </div>
  )
}

export function MobileWorkspace({ projectId }: { projectId: string | null }) {
  const sel = useWsUi((s) => s.mobile)
  const setMobile = useWsUi((s) => s.setMobile)
  const listScope = useWsPrefs((s) => s.listScope)
  const setListScope = useWsPrefs((s) => s.setListScope)
  const scope = listedScope(listScope, projectId)
  const { data, error, isLoading, refetch } = useCards(scope)
  const [query, setQuery] = useState('')
  if (sel) return <MobileCard sel={sel} onBack={() => setMobile(null)} />
  const cards = visibleCards(data?.cards ?? [], query, false)
  return (
    <div className="wb-fill ws-m">
      <div className="ws-m-bar">
        <div className="ws-seg" role="tablist" aria-label="Scope">
          {projectId && (
            <button role="tab" aria-selected={scope !== HOME && scope !== SANDBOX} className={scope !== HOME && scope !== SANDBOX ? 'active' : ''} onClick={() => setListScope('project')}>
              Project
            </button>
          )}
          <button role="tab" aria-selected={scope === HOME} className={scope === HOME ? 'active' : ''} onClick={() => setListScope('home')}>
            Home
          </button>
          <button role="tab" aria-selected={scope === SANDBOX} className={scope === SANDBOX ? 'active' : ''} onClick={() => setListScope('sandbox')}>
            Sandbox
          </button>
        </div>
        <label className="ws-search wb-grow">
          <Search size={14} />
          <input value={query} placeholder="Search" aria-label="Search cards" onChange={(e) => setQuery(e.target.value)} />
          {query && <IconButton icon={X} size="small" label="Clear" onClick={() => setQuery('')} />}
        </label>
      </div>
      <div className="wb-scroll">
        {error ? (
          <ErrorBox error={error} onRetry={() => void refetch()} />
        ) : isLoading ? (
          <Loading />
        ) : !cards.length ? (
          <EmptyState icon={LayoutGrid} title={query ? 'No cards match' : 'No cards yet'}>
            {!query && 'Deliverables from agents show up here.'}
          </EmptyState>
        ) : (
          cards.map((c) => (
            <button key={`${c.scope}:${c.id}`} className="ws-m-row" onClick={() => setMobile({ scope: c.scope, cardId: c.id })}>
              <CardThumb card={c} size={48} />
              <span className="wb-grow ws-m-row-text">
                <span className="ws-m-row-title wb-ellipsis">
                  {c.pinned && <Pin size={11} />} {c.title}
                </span>
                {c.description && <span className="ws-m-row-desc">{c.description}</span>}
                <span className="wb-xs wb-muted">
                  {categoryLabel(c.category)} · {c.steps.length} step{c.steps.length === 1 ? '' : 's'} · {relativeDay(c.touchedAt)}
                </span>
              </span>
            </button>
          ))
        )}
      </div>
    </div>
  )
}

export default MobileWorkspace
