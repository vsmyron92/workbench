// Things the Workspace UI does from several places: open cards, change them, take
// drops from the files tree or the desktop.

import type { QueryClient } from '@tanstack/react-query'
import { Archive, ArchiveRestore, Bot, CheckCircle2, CircleDot, Copy, ExternalLink, Pin, PinOff, SquareTerminal, Trash2 } from 'lucide-react'
import { api } from '@/api/client'
import type { TerminalInfo } from '@/api/types'
import { closePanel, confirmDialog, getDockApi, openPanel, toast, toastError } from '@/shell/actions'
import { askAgent } from '@/shell/agentBridge'
import { useUi } from '@/state/store'
import type { MenuEntry } from '@/ui'
import { type CardPatch, type WorkspaceCard, type WorkspaceStep, wk, wsApi } from './api'
import { askPrompt, basename, cardPanelId, HOME, isProjectless, SANDBOX } from './logic'
import { useWsDrafts } from './store'

/** Drag payload of the files tree (docs/ARCHITECTURE.md, "Drag and drop"). */
export const PATH_MIME = 'application/x-workbench-path'
const MAX_DROP_FILES = 50

export function openCard(scope: string, cardId: string, title?: string, step?: number) {
  const params: Record<string, unknown> = { scope, cardId }
  if (step !== undefined) params.step = step
  openPanel({ kind: 'card', id: cardPanelId(scope, cardId), title: title ?? 'Card', params })
}

export function openHome(scope?: string) {
  // Params merge into an open home: clear the trash view explicitly.
  openPanel({ kind: 'workspace.home', id: 'workspace.home', title: 'Workspace', params: scope ? { scope, view: undefined } : { view: undefined } })
}

/** The Workspace home showing the trash (of `scope`, else the current one). */
export function openTrash(scope?: string) {
  openPanel({ kind: 'workspace.home', id: 'workspace.home', title: 'Workspace', params: scope ? { scope, view: 'trash' } : { view: 'trash' } })
}

/** The scope a new card goes to by default: the current project, else Home. */
export function defaultScope(): string {
  return useUi.getState().projectId ?? HOME
}

/** Refresh every view of a card after a change (the event does it too, a bit later). */
export function applyCard(qc: QueryClient, card: WorkspaceCard) {
  qc.setQueryData(wk.card(card.scope, card.id), card)
  void qc.invalidateQueries({ queryKey: ['workspace', 'cards'] })
  void qc.invalidateQueries({ queryKey: wk.scopes })
}

export async function patchCard(qc: QueryClient, card: WorkspaceCard, patch: CardPatch): Promise<WorkspaceCard | null> {
  try {
    const c = await wsApi.patch(card.scope, card.id, patch)
    applyCard(qc, c)
    return c
  } catch (e) {
    toastError(e, 'Could not update the card')
    return null
  }
}

export async function deleteCard(qc: QueryClient, card: WorkspaceCard) {
  const ok = await confirmDialog({
    title: `Delete "${card.title}"?`,
    message: 'The card and its files move to the Workspace trash. Restore them from Trash in the Workspace home until you empty it.',
    confirmLabel: 'Move to Trash',
    danger: true,
  })
  if (!ok) return
  try {
    const r = await wsApi.remove(card.scope, card.id)
    closePanel(cardPanelId(card.scope, card.id))
    useWsDrafts.getState().dropCard(card.scope, card.id)
    qc.removeQueries({ queryKey: wk.card(card.scope, card.id) })
    void qc.invalidateQueries({ queryKey: wk.all })
    const item = r.trashItem
    toast('success', `Moved "${card.title}" to the Workspace trash`, {
      timeout: 8000,
      action: item ? { label: 'Undo', run: () => void restoreFromTrash(qc, card.scope, item, card.title) } : undefined,
    })
  } catch (e) {
    toastError(e, 'Could not delete the card')
  }
}

/** Put a trashed card back. Resolves to the restored card, or null. */
export async function restoreFromTrash(qc: QueryClient, scope: string, item: string, title: string): Promise<WorkspaceCard | null> {
  try {
    const c = await wsApi.restore(scope, item)
    void qc.invalidateQueries({ queryKey: wk.all })
    toast('success', `Restored "${title}"`, { action: { label: 'Open', run: () => openCard(c.scope, c.id, c.title) } })
    return c
  } catch (e) {
    toastError(e, `Could not restore "${title}"`)
    return null
  }
}

/** Empty the Sandbox (every card to the trash) after a confirmation. */
export async function resetSandbox(qc: QueryClient) {
  const ok = await confirmDialog({
    title: 'Reset the Sandbox?',
    message: 'Every card in the Sandbox moves to the Workspace trash and the guide card comes back. Restore cards from Trash until you empty it.',
    confirmLabel: 'Reset',
    danger: true,
  })
  if (!ok) return
  try {
    const r = await wsApi.resetSandbox()
    // Open cards of the Sandbox are gone, and so are their unsaved drafts.
    for (const p of getDockApi()?.panels ?? []) if (p.id.startsWith(`card:${SANDBOX}:`)) closePanel(p.id)
    useWsDrafts.getState().dropScope(SANDBOX)
    void qc.invalidateQueries({ queryKey: wk.all })
    toast('success', r.removed ? `Sandbox reset: ${r.removed} card${r.removed === 1 ? '' : 's'} moved to the trash` : 'Sandbox reset')
  } catch (e) {
    toastError(e, 'Could not reset the Sandbox')
  }
}

export function askAboutCard(card: WorkspaceCard, step?: WorkspaceStep) {
  const projectId = !isProjectless(card.scope) ? card.scope : useUi.getState().projectId
  if (!projectId) {
    toast('info', 'Open a project first: agents run inside a project')
    return
  }
  void askAgent({ projectId, prompt: askPrompt(card, step), submit: false })
}

export async function openTerminalIn(card: WorkspaceCard) {
  try {
    const projectId = !isProjectless(card.scope) ? card.scope : null
    const t = await api.post<TerminalInfo>('/api/terminals', { kind: 'shell', projectId, cwd: card.folderPath })
    openPanel({ kind: 'terminal', id: `terminal:${t.id}`, title: t.title, params: { terminalId: t.id } })
  } catch (e) {
    toastError(e, 'Could not open a terminal')
  }
}

export async function copyText(text: string, what: string) {
  try {
    await navigator.clipboard.writeText(text)
    toast('success', `Copied ${what}`)
  } catch {
    toast('error', 'The clipboard is not available here')
  }
}

export function cardMenu(qc: QueryClient, card: WorkspaceCard, opts: { open?: boolean } = {}): MenuEntry[] {
  const items: MenuEntry[] = []
  if (opts.open !== false) items.push({ label: 'Open', icon: ExternalLink, run: () => openCard(card.scope, card.id, card.title) })
  items.push(
    card.pinned
      ? { label: 'Unpin', icon: PinOff, run: () => void patchCard(qc, card, { pinned: false }) }
      : { label: 'Pin', icon: Pin, run: () => void patchCard(qc, card, { pinned: true }) },
  )
  // Any change touches the card, so "Unarchive" also brings an auto-archived card back.
  if (card.archived) {
    items.push({ label: 'Unarchive', icon: ArchiveRestore, run: () => void patchCard(qc, card, { status: 'active' }) })
  } else {
    if (card.status === 'done') items.push({ label: 'Mark as active', icon: CircleDot, run: () => void patchCard(qc, card, { status: 'active' }) })
    else items.push({ label: 'Mark as done', icon: CheckCircle2, run: () => void patchCard(qc, card, { status: 'done' }) })
    items.push({ label: 'Archive', icon: Archive, run: () => void patchCard(qc, card, { status: 'archived' }) })
  }
  items.push(
    'separator',
    { label: 'Ask agent about this card', icon: Bot, run: () => askAboutCard(card) },
    { label: 'Open terminal in card folder', icon: SquareTerminal, run: () => void openTerminalIn(card) },
    { label: 'Copy folder path', icon: Copy, run: () => void copyText(card.folderPath, 'the folder path') },
  )
  if (card.editable) items.push('separator', { label: 'Delete card…', icon: Trash2, danger: true, run: () => void deleteCard(qc, card) })
  return items
}

/** Whether a drag carries something a card can take. */
export function dragHasPayload(dt: DataTransfer): boolean {
  const types = Array.from(dt.types)
  return types.includes(PATH_MIME) || types.includes('Files')
}

/** A path from the files tree becomes a step (copied in when outside the card);
 *  files from the desktop are uploaded as steps. */
export async function dropOnCard(qc: QueryClient, card: WorkspaceCard, dt: DataTransfer) {
  if (!card.editable) {
    toast('info', "Cards from the repository's workspace.json are read-only here")
    return
  }
  const path = dt.getData(PATH_MIME)
  if (path) {
    try {
      const c = await wsApi.addStep(card.scope, card.id, { path })
      applyCard(qc, c)
      toast('success', `Added ${basename(path)} to "${card.title}"`)
    } catch (e) {
      toastError(e, 'Could not add the step')
    }
    return
  }
  const files = Array.from(dt.files)
  if (!files.length) return
  if (files.length > MAX_DROP_FILES) {
    toast('error', `Drop at most ${MAX_DROP_FILES} files at a time`)
    return
  }
  let added = 0
  let last: WorkspaceCard | null = null
  for (const f of files) {
    try {
      const r = await wsApi.upload(card.scope, card.id, f, f.name, { step: true })
      last = r.card
      added++
    } catch (e) {
      toastError(e, `Could not add ${f.name}`)
    }
  }
  if (last) applyCard(qc, last)
  if (added) toast('success', `Added ${added} file${added === 1 ? '' : 's'} to "${card.title}"`)
}
