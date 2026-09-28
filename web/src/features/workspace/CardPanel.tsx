// 'card' panel: one card — header (title, status, pin, ask agent, files), a tab per
// step, and the step's viewer. Drop files from the project tree (or the desktop)
// anywhere on it to add steps. A file drawer browses the card folder.

import { useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import {
  ArrowLeft,
  ArrowRight,
  Bot,
  ChevronDown,
  Copy,
  ExternalLink,
  FileX,
  Folder,
  FolderOpen,
  MoreHorizontal,
  Pencil,
  Pin,
  PinOff,
  Plus,
  Star,
  Trash2,
  X,
} from 'lucide-react'
import { ApiError } from '@/api/client'
import { toastError } from '@/shell/actions'
import type { PanelProps } from '@/shell/types'
import { Button, EmptyState, ErrorBox, formatBytes, IconButton, Loading, showMenu, showMenuAt, type MenuEntry } from '@/ui'
import { type FileEntry, type WorkspaceCard, type WorkspaceStep, useCard, useCardFiles, wsApi } from './api'
import { applyCard, askAboutCard, cardMenu, copyText, dragHasPayload, dropOnCard, patchCard } from './actions'
import { EditCardDialog, StepDialog } from './dialogs'
import { basename, categoryLabel, fileUrl, relativeDay, resolveStep } from './logic'
import { CardThumb, CategoryIcon, KindIcon } from './parts'
import { draftKey, isDirty, useWsDrafts } from './store'
import { StepViewer, type ViewTarget } from './viewers/StepViewer'

export interface CardParams {
  scope: string
  cardId: string
  step?: number
}

function StatusButton({ card }: { card: WorkspaceCard }) {
  const qc = useQueryClient()
  const label = card.status === 'archived' || card.archived ? 'Archived' : card.status === 'done' ? 'Done' : 'Active'
  return (
    <Button
      size="small"
      variant="ghost"
      onClick={(e) =>
        showMenuAt(e.currentTarget, [
          { label: 'Active', run: () => void patchCard(qc, card, { status: 'active' }) },
          { label: 'Done', run: () => void patchCard(qc, card, { status: 'done' }) },
          { label: 'Archived', run: () => void patchCard(qc, card, { status: 'archived' }) },
        ])
      }
      title="Status (any change also marks the card as touched)"
    >
      <span className={`wb-dot ${label === 'Active' ? 'success' : label === 'Done' ? 'accent' : ''}`} />
      {label}
      <ChevronDown size={12} />
    </Button>
  )
}

function stepMenu(qc: ReturnType<typeof useQueryClient>, card: WorkspaceCard, s: WorkspaceStep, edit: () => void): MenuEntry[] {
  const move = async (to: number) => {
    try {
      applyCard(qc, await wsApi.patchStep(card.scope, card.id, s.index, { position: to, expectPath: s.path }))
    } catch (e) {
      toastError(e, 'Could not move the step')
    }
  }
  const items: MenuEntry[] = [{ label: 'Open in a new browser tab', icon: ExternalLink, run: () => window.open(fileUrl(card.base, s.path), '_blank', 'noopener,noreferrer') }]
  if (!card.editable) return items
  items.push(
    { label: 'Edit step…', icon: Pencil, run: edit },
    { label: 'Open this tab by default', icon: Star, disabled: card.defaultStep === s.index, run: () => void patchCard(qc, card, { defaultStep: s.index }) },
    { label: 'Move left', icon: ArrowLeft, disabled: s.index === 0, run: () => void move(s.index - 1) },
    { label: 'Move right', icon: ArrowRight, disabled: s.index === card.steps.length - 1, run: () => void move(s.index + 1) },
    'separator',
    {
      label: 'Remove from card',
      icon: Trash2,
      danger: true,
      run: async () => {
        try {
          applyCard(qc, await wsApi.deleteStep(card.scope, card.id, s.index, s.path))
        } catch (e) {
          toastError(e, 'Could not remove the step')
        }
      },
    },
  )
  return items
}

function FilesDrawer({ card, onPreview, onClose }: { card: WorkspaceCard; onPreview: (f: FileEntry) => void; onClose: () => void }) {
  const qc = useQueryClient()
  const [dir, setDir] = useState('')
  const { data, error, isLoading } = useCardFiles(card.scope, card.id, dir)
  const stepPaths = new Set(card.steps.map((s) => s.path))
  const add = async (f: FileEntry) => {
    try {
      applyCard(qc, await wsApi.addStep(card.scope, card.id, { path: f.path, name: f.name.replace(/\.[^.]+$/, '') }))
    } catch (e) {
      toastError(e, 'Could not add the step')
    }
  }
  return (
    <aside className="ws-drawer">
      <div className="wb-toolbar">
        <span className="title">Card folder</span>
        <span className="spacer" />
        <IconButton icon={Copy} size="small" label="Copy folder path" onClick={() => void copyText(card.folderPath, 'the folder path')} />
        <IconButton icon={X} size="small" label="Close" onClick={onClose} />
      </div>
      <div className="ws-drawer-path wb-xs wb-muted wb-ellipsis" title={card.folderPath}>
        {dir ? (
          <button className="ws-crumb" onClick={() => setDir(dir.includes('/') ? dir.slice(0, dir.lastIndexOf('/')) : '')}>
            <ArrowLeft size={12} /> {dir}
          </button>
        ) : (
          card.folderPath
        )}
      </div>
      <div className="wb-scroll">
        {error ? (
          <ErrorBox error={error} />
        ) : isLoading ? (
          <Loading />
        ) : !data?.entries.length ? (
          <EmptyState title="Empty folder">Agents write the card's files here.</EmptyState>
        ) : (
          data.entries.map((f) => (
            <div
              key={f.path}
              className="wb-list-row ws-file-row"
              onClick={() => (f.dir ? setDir(f.path) : onPreview(f))}
              onDoubleClick={() => f.dir && onPreview(f)}
              title={f.dir ? 'Open folder (double-click to view as a gallery)' : `Preview ${f.name}`}
            >
              {f.dir ? <Folder size={14} className="wb-subtle" /> : <KindIcon kind={f.kind === 'dir' ? 'gallery' : f.kind} />}
              <span className="wb-grow wb-ellipsis">{f.name}</span>
              {stepPaths.has(f.path) ? (
                <span className="wb-xs wb-subtle">step</span>
              ) : (
                card.editable && (
                  <IconButton
                    icon={Plus}
                    size="small"
                    label="Add as a step"
                    className="ws-row-action"
                    onClick={(e) => {
                      e.stopPropagation()
                      void add(f)
                    }}
                  />
                )
              )}
              {!f.dir && <span className="wb-xs wb-subtle">{formatBytes(f.size)}</span>}
            </div>
          ))
        )}
      </div>
    </aside>
  )
}

export function CardPanel({ params, setParams, setTitle, close, active }: PanelProps<CardParams>) {
  const qc = useQueryClient()
  const { data: card, error, isLoading, refetch } = useCard(params.scope, params.cardId)
  const [preview, setPreview] = useState<FileEntry | null>(null)
  const [filesOpen, setFilesOpen] = useState(false)
  const [stepDialog, setStepDialog] = useState<{ step?: WorkspaceStep } | null>(null)
  const [editing, setEditing] = useState(false)
  const [dragOver, setDragOver] = useState(false)
  const stepsRef = useRef<HTMLDivElement>(null)
  const drafts = useWsDrafts((s) => s.drafts)
  const shownStep = card ? resolveStep(card.steps.length, params.step, card.defaultIndex) : -1

  // Keep the selected tab visible in a long strip of steps.
  useEffect(() => {
    // Only the strip scrolls (scrollIntoView would also move the dock's containers).
    const strip = stepsRef.current
    const tab = strip?.querySelector<HTMLElement>('.ws-step.active')
    if (!strip || !tab) return
    const left = tab.offsetLeft
    const right = left + tab.offsetWidth
    if (left < strip.scrollLeft) strip.scrollLeft = Math.max(0, left - 8)
    else if (right > strip.scrollLeft + strip.clientWidth) strip.scrollLeft = right - strip.clientWidth + 8
  }, [shownStep, preview, card?.steps.length])

  useEffect(() => {
    if (card) setTitle(card.title)
  }, [card?.title, setTitle]) // eslint-disable-line react-hooks/exhaustive-deps

  if (error instanceof ApiError && error.status === 404) {
    return (
      <EmptyState icon={FileX} title="This card no longer exists" action={<Button size="small" onClick={close}>Close</Button>}>
        It was deleted, or its registry changed.
      </EmptyState>
    )
  }
  if (error) return <ErrorBox error={error} onRetry={() => void refetch()} />
  if (isLoading || !card) return <Loading label="Loading card…" />

  const index = resolveStep(card.steps.length, params.step, card.defaultIndex)
  const step = card.steps[index]
  const select = (i: number) => {
    setPreview(null)
    setParams({ scope: params.scope, cardId: params.cardId, step: i })
  }
  const openPath = (path: string) => {
    const s = card.steps.find((x) => x.path === path)
    if (!s) return false
    select(s.index)
    return true
  }
  const target: ViewTarget | null = preview
    ? { path: preview.path, name: preview.name, kind: preview.dir ? 'gallery' : preview.kind === 'dir' ? 'gallery' : preview.kind, exists: true, size: preview.size, mtime: preview.mtime }
    : step
      ? { path: step.path, name: step.name, kind: step.kind, exists: step.exists, size: step.size, mtime: step.mtime }
      : null

  const more: MenuEntry[] = [...(card.editable ? [{ label: 'Edit details…', icon: Pencil, run: () => setEditing(true) }] : []), ...cardMenu(qc, card, { open: false })]

  return (
    <div
      className={`wb-fill ws-card${dragOver ? ' drop' : ''}`}
      onDragOver={(e) => {
        if (!card.editable || !dragHasPayload(e.dataTransfer)) return
        e.preventDefault()
        e.dataTransfer.dropEffect = 'copy'
        setDragOver(true)
      }}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragOver(false)
      }}
      onDrop={(e) => {
        if (!dragHasPayload(e.dataTransfer)) return
        e.preventDefault()
        setDragOver(false)
        void dropOnCard(qc, card, e.dataTransfer)
      }}
    >
      <header className="ws-card-head">
        <CardThumb card={card} size={40} />
        <div className="wb-grow ws-card-titles">
          <div className="wb-row">
            <h2 className="ws-card-title wb-ellipsis" title={card.title}>
              {card.title}
            </h2>
            {card.origin === 'repo' && (
              <span className="wb-badge" title="From the project's workspace/workspace.json: status and pin only">
                repo
              </span>
            )}
          </div>
          <div className="ws-card-meta">
            <span className="ws-chip">
              <CategoryIcon category={card.category} size={12} />
              {categoryLabel(card.category)}
            </span>
            <span title={new Date(card.touchedAt).toLocaleString()}>updated {relativeDay(card.touchedAt)}</span>
            <span>·</span>
            <span>{card.scopeName}</span>
            {card.description && (
              <>
                <span>·</span>
                <span className="wb-ellipsis ws-card-desc" title={card.description}>
                  {card.description}
                </span>
              </>
            )}
          </div>
        </div>
        <div className="ws-card-actions">
          <IconButton icon={card.pinned ? PinOff : Pin} label={card.pinned ? 'Unpin' : 'Pin'} active={card.pinned} onClick={() => void patchCard(qc, card, { pinned: !card.pinned })} />
          <StatusButton card={card} />
          <Button size="small" variant="ghost" icon={Bot} onClick={() => askAboutCard(card, step)} title="Paste this card's context into an agent session">
            Ask agent
          </Button>
          <IconButton icon={FolderOpen} label="Card folder" active={filesOpen} onClick={() => setFilesOpen(!filesOpen)} />
          <IconButton icon={MoreHorizontal} label="More" onClick={(e) => showMenuAt(e.currentTarget, more)} />
        </div>
      </header>
      <div className="ws-steps" role="tablist" ref={stepsRef}>
        {card.steps.map((s) => (
          <button
            key={`${s.index}:${s.path}`}
            role="tab"
            aria-selected={!preview && s.index === index}
            className={`ws-step${!preview && s.index === index ? ' active' : ''}${s.exists ? '' : ' missing'}`}
            onClick={() => select(s.index)}
            onContextMenu={(e) => showMenu(e, stepMenu(qc, card, s, () => setStepDialog({ step: s })))}
            title={`${s.path}${s.exists ? '' : ' (missing)'}${card.defaultStep === s.index ? ' · opens by default' : ''}`}
          >
            <KindIcon kind={s.kind} />
            <span className="wb-ellipsis">{s.name}</span>
            {isDirty(drafts[draftKey(card.scope, card.id, s.path)]) && <span className="ws-step-dirty" title="Unsaved changes" />}
          </button>
        ))}
        {preview && (
          <span className="ws-step active preview" title={preview.path}>
            <KindIcon kind={preview.dir || preview.kind === 'dir' ? 'gallery' : preview.kind} />
            <span className="wb-ellipsis">{basename(preview.path)}</span>
            <IconButton icon={X} size="small" label="Close preview" onClick={() => setPreview(null)} />
          </span>
        )}
        {card.editable && <IconButton icon={Plus} size="small" label="Add step" onClick={() => setStepDialog({})} />}
      </div>
      <div className="ws-card-main">
        <div className="ws-card-view">
          {target ? (
            <StepViewer card={card} target={target} onOpenPath={openPath} active={active} />
          ) : (
            <EmptyState
              icon={FolderOpen}
              title="No steps yet"
              action={
                card.editable && (
                  <Button size="small" variant="primary" icon={Plus} onClick={() => setStepDialog({})}>
                    Add step
                  </Button>
                )
              }
            >
              Steps are the card's tabs: reports, documents, images, 3D comparisons. Drop files here from the project tree, or let an
              agent add them with workspace_add_step.
            </EmptyState>
          )}
        </div>
        {filesOpen && <FilesDrawer card={card} onPreview={setPreview} onClose={() => setFilesOpen(false)} />}
      </div>
      {dragOver && <div className="ws-drop-hint">Drop to add to “{card.title}”</div>}
      {stepDialog && <StepDialog card={card} step={stepDialog.step} onClose={() => setStepDialog(null)} />}
      {editing && <EditCardDialog card={card} onClose={() => setEditing(false)} />}
    </div>
  )
}

export default CardPanel
