// Terminal actions shared by panels, the tool windows, cards and menus.

import {
  Copy,
  ExternalLink,
  Eraser,
  FileDiff,
  Palette,
  Pencil,
  Pin,
  PinOff,
  Play,
  Power,
  RotateCcw,
  SquareTerminal,
  Trash2,
  XCircle,
} from 'lucide-react'
import { api } from '@/api/client'
import { subscribe } from '@/api/events'
import type { TerminalInfo } from '@/api/types'
import { closePanel, confirmDialog, isPanelOpen, openPanel, promptDialog, showToolWindow, toast, toastError } from '@/shell/actions'
import type { MenuEntry } from '@/ui'
import { openTerminal, terminalsApi } from './api'
import { resumes } from './lib/providers'
import { canKill, isRunning, lingering, restartMode, TAB_COLORS, terminalPanelId } from './lib/sessions'

export async function renameTerminal(t: TerminalInfo) {
  const title = await promptDialog({ title: t.kind === 'agent' ? 'Rename session' : 'Rename terminal', initial: t.title, confirmLabel: 'Rename' })
  if (!title?.trim() || title.trim() === t.title) return
  try {
    await terminalsApi.patch(t.id, { title: title.trim() })
  } catch (e) {
    toastError(e, 'Rename failed')
  }
}

export async function setColor(t: TerminalInfo, color: string | null) {
  try {
    await terminalsApi.patch(t.id, { color })
  } catch (e) {
    toastError(e)
  }
}

export async function togglePin(t: TerminalInfo) {
  try {
    await terminalsApi.patch(t.id, { pinned: !t.pinned })
  } catch (e) {
    toastError(e)
  }
}

/** Why a deploy or env command does not restart from its terminal. */
export function ownerHint(t: TerminalInfo): string {
  const m = t.meta ?? {}
  if (m.action === 'deploy') return 'Deploys start again from the environment’s Deploy button, which re-checks the gates and asks for confirmation.'
  if (m.action === 'command') return 'Environment commands start again from the Apps tool window.'
  return 'This terminal starts again from where it came from.'
}

/** Wait for a terminal matching `pred` to be created (`null` after `ms`). */
function nextTerminal(pred: (t: TerminalInfo) => boolean, ms: number): { promise: Promise<TerminalInfo | null>; cancel: () => void } {
  let off = () => {}
  let timer: number | undefined
  let resolve: (t: TerminalInfo | null) => void = () => {}
  const promise = new Promise<TerminalInfo | null>((r) => (resolve = r))
  const done = (t: TerminalInfo | null) => {
    off()
    window.clearTimeout(timer)
    resolve(t)
  }
  off = subscribe('terminal.created', (ev) => {
    const t = ev.data as TerminalInfo
    if (pred(t)) done(t)
  })
  timer = window.setTimeout(() => done(null), ms)
  return { promise, cancel: () => done(null) }
}

/** Restart a run configuration through apps (it tracks the run) and show its new terminal. */
async function rerun(t: TerminalInfo, onReplaced?: (next: TerminalInfo) => void) {
  const name = String(t.meta.run)
  const pid = t.projectId!
  const next = nextTerminal((x) => x.kind === 'run' && x.projectId === pid && x.meta?.run === name && x.id !== t.id, 120_000)
  try {
    await api.post(`/api/projects/${encodeURIComponent(pid)}/runs/${encodeURIComponent(name)}/restart`, { freePort: false })
  } catch (e) {
    next.cancel()
    toastError(e, `Could not restart ${name}`)
    return
  }
  const created = await next.promise
  if (!created) return
  if (onReplaced) onReplaced(created)
  else if (isPanelOpen(terminalPanelId(t.id))) {
    openTerminal({ id: created.id, title: created.title })
    closePanel(terminalPanelId(t.id))
  }
}

/**
 * Start a terminal's process again. Agents resume, shells and log follows restart in
 * place; run configurations restart through apps (a new terminal, passed to
 * `onReplaced`); deploys and env commands only start again from their environment, so
 * their gates and confirmation run again (the server refuses them too).
 */
export async function restartTerminal(t: TerminalInfo, onReplaced?: (next: TerminalInfo) => void) {
  const mode = restartMode(t)
  if (mode === 'owner') {
    if (typeof t.meta?.env === 'string') showToolWindow('apps')
    toast('info', ownerHint(t))
    return
  }
  if (mode === 'run') return rerun(t, onReplaced)
  try {
    const next = await terminalsApi.restart(t.id)
    if (next.kind === 'agent') openTerminal(next)
  } catch (e) {
    toastError(e, t.kind === 'agent' ? 'Could not resume the session' : 'Could not restart')
  }
}

export async function killTerminal(t: TerminalInfo) {
  const busy = t.agent && (t.agent.state === 'working' || t.agent.state === 'needs_permission')
  if (busy) {
    const ok = await confirmDialog({
      title: 'Stop this session?',
      message: `“${t.title}” is in the middle of a turn. Its process stops; the conversation stays on disk and can be resumed.`,
      confirmLabel: 'Stop',
      danger: true,
    })
    if (!ok) return
  }
  try {
    await terminalsApi.kill(t.id)
  } catch (e) {
    toastError(e, 'Could not stop the process')
  }
}

/** Close the tab: stops the process; the terminal stays in history. Resolves to whether it was closed. */
export async function closeTerminal(t: TerminalInfo): Promise<boolean> {
  const busy = isRunning(t) && t.agent && t.agent.state === 'working'
  if (busy) {
    const ok = await confirmDialog({
      title: 'Close a working session?',
      message: `“${t.title}” is still working. Closing stops it; you can resume it from the history.`,
      confirmLabel: 'Close',
      danger: true,
    })
    if (!ok) return false
  }
  closePanel(terminalPanelId(t.id))
  try {
    await terminalsApi.close(t.id)
    return true
  } catch (e) {
    toastError(e, 'Could not close the terminal')
    return false
  }
}

export async function forgetTerminal(t: TerminalInfo) {
  const ok = await confirmDialog({
    title: 'Remove from history?',
    message:
      t.kind === 'agent'
        ? 'Workbench forgets this terminal and its saved screen. The conversation itself stays on disk (where the agent CLI keeps it) and remains in the session history.'
        : 'Workbench forgets this terminal and its saved screen.',
    confirmLabel: 'Remove',
    danger: true,
  })
  if (!ok) return
  closePanel(terminalPanelId(t.id))
  try {
    await terminalsApi.close(t.id, true)
  } catch (e) {
    toastError(e)
  }
}

export async function interrupt(t: TerminalInfo) {
  try {
    await terminalsApi.keys(t.id, ['esc'])
  } catch (e) {
    toastError(e, 'Could not interrupt')
  }
}

export async function copyText(text: string, what = 'Copied') {
  try {
    await navigator.clipboard.writeText(text)
    toast('success', what)
  } catch {
    toast('warning', 'The clipboard needs a secure context (HTTPS or localhost)')
  }
}

export function openRemote(url: string) {
  if (url.startsWith('https://')) window.open(url, '_blank', 'noopener,noreferrer')
}

export function colorMenu(t: TerminalInfo): MenuEntry[] {
  return [
    ...TAB_COLORS.map<MenuEntry>((c) => ({ label: `${t.color === c.id ? '✓ ' : ''}${c.label}`, icon: Palette, run: () => void setColor(t, c.id) })),
    'separator',
    { label: 'No colour', icon: Eraser, disabled: !t.color, run: () => void setColor(t, null) },
  ]
}

/** Review Changes: the files an agent session changed (the files slice's `agentChanges` panel). */
export function reviewChanges(t: TerminalInfo) {
  if (!t.projectId) return
  openPanel({ kind: 'agentChanges', id: `agentChanges:${t.id}`, title: `Changes · ${t.title}`, params: { projectId: t.projectId, terminalId: t.id, title: t.title } })
}

/** Context menu for a terminal (tool windows, cards, header). */
export function terminalMenu(t: TerminalInfo, opts: { open?: boolean; onReplaced?: (next: TerminalInfo) => void } = {}): MenuEntry[] {
  const running = isRunning(t)
  const agent = t.kind === 'agent'
  const mode = restartMode(t)
  const items: MenuEntry[] = []
  if (opts.open !== false) items.push({ label: 'Open', icon: SquareTerminal, run: () => openTerminal(t) })
  if (mode === 'run') {
    items.push({ label: running ? 'Restart run' : 'Run again', icon: running ? RotateCcw : Play, run: () => void restartTerminal(t, opts.onReplaced) })
  } else if (mode === 'self') {
    const cont = agent && resumes(t)
    if (!running) items.push({ label: cont ? 'Resume session' : agent ? 'Start again' : 'Restart', icon: Play, run: () => void restartTerminal(t) })
    else if (agent) items.push({ label: cont ? 'Restart (resume session)' : 'Restart', icon: RotateCcw, run: () => void restartTerminal(t) })
  }
  // What the session changed (files slice: Local History's agent attribution).
  if (agent && t.projectId) items.push({ label: 'Review Changes', icon: FileDiff, run: () => reviewChanges(t) })
  items.push(
    'separator',
    { label: 'Rename…', icon: Pencil, run: () => void renameTerminal(t) },
    { label: t.pinned ? 'Unpin' : 'Pin', icon: t.pinned ? PinOff : Pin, run: () => void togglePin(t) },
    ...TAB_COLORS.slice(0, 4).map<MenuEntry>((c) => ({
      label: `${t.color === c.id ? '✓ ' : ''}Colour: ${c.label}`,
      icon: Palette,
      run: () => void setColor(t, t.color === c.id ? null : c.id),
    })),
  )
  const url = t.agent?.remoteUrl
  if (url || t.agent?.sessionId) items.push('separator')
  if (url) {
    items.push({ label: 'Copy Remote Control link', icon: Copy, run: () => void copyText(url, 'Remote Control link copied') })
    items.push({ label: 'Open on claude.ai', icon: ExternalLink, run: () => openRemote(url) })
  }
  if (t.agent?.sessionId) items.push({ label: 'Copy session id', icon: Copy, run: () => void copyText(t.agent!.sessionId, 'Session id copied') })
  items.push('separator')
  if (canKill(t)) {
    const n = lingering(t)
    const label = running ? (agent ? 'Stop session' : 'Kill process') : `Kill ${n} background process${n === 1 ? '' : 'es'}`
    items.push({ label, icon: Power, danger: true, run: () => void killTerminal(t) })
  }
  if (t.open) items.push({ label: 'Close tab', icon: XCircle, run: () => void closeTerminal(t) })
  items.push({ label: 'Remove from history…', icon: Trash2, danger: true, run: () => void forgetTerminal(t) })
  return items
}
