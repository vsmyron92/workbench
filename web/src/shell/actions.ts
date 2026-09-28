// Imperative shell actions any feature can call: open/close/focus panels, show
// tool windows, toasts and dialogs.

import type { DockviewApi, DockviewGroupPanel } from 'dockview-react'
import { create } from 'zustand'
import { api as http } from '@/api/client'
import { useUi } from '@/state/store'
import type { Side } from './types'

// ---------------------------------------------------------------- panels

let dockApi: DockviewApi | null = null
/** Opens requested before the desktop dock exists (startup), replayed once it does. */
const pending: OpenPanelOptions[] = []
const MAX_PENDING = 20

/** Set by the phone shell: shows a panel as a mobile tab when some tab can. */
type MobileRouter = (panel: { kind: string; id: string; title?: string; params: Record<string, unknown> }) => boolean
let mobileRouter: MobileRouter | null = null

export function setDockApi(api: DockviewApi | null) {
  dockApi = api
  if (api) pending.splice(0).forEach((o) => openPanel(o))
}

/** The phone layout is mounted: `openPanel` goes to its tabs instead of a dock. */
export function setMobileRouter(router: MobileRouter | null) {
  mobileRouter = router
  if (router) pending.length = 0
}

export function isMobileShell() {
  return mobileRouter !== null
}

export function getDockApi() {
  return dockApi
}

/** Panel kinds that belong to the "agents" column (left of documents by default). */
const TERMINAL_KINDS = new Set(['terminal', 'agents.home'])

export interface OpenPanelOptions {
  /** Panel kind registered by a feature (see docs/ARCHITECTURE.md#panels). */
  kind: string
  /** Stable id; reopening the same id focuses the existing tab. Default: kind + params. */
  id?: string
  title?: string
  params?: Record<string, unknown>
  /**
   * 'auto' (default): terminals go to the agents column, everything else to the
   * documents column (created to the right of the agents on first use).
   * 'active': the active group. 'right' | 'below': split the active group.
   */
  position?: 'auto' | 'active' | 'right' | 'below'
  /** Focus the panel (default true). */
  focus?: boolean
}

function defaultId(kind: string, params?: Record<string, unknown>) {
  return params && Object.keys(params).length ? `${kind}:${JSON.stringify(params)}` : kind
}

function groupHasTerminals(g: DockviewGroupPanel) {
  return g.panels.some((p) => TERMINAL_KINDS.has(p.view.contentComponent))
}

let lastDocGroup: string | null = null
let lastTermGroup: string | null = null

export function noteActiveGroup(g: DockviewGroupPanel | undefined) {
  if (!g) return
  if (groupHasTerminals(g)) lastTermGroup = g.id
  else if (g.panels.length) lastDocGroup = g.id
}

/**
 * Open (or focus) a panel. Returns the panel id. On a phone, a mobile tab that can
 * show the panel is selected instead; panels no tab shows are not opened there.
 */
export function openPanel(o: OpenPanelOptions): string {
  const id = o.id ?? defaultId(o.kind, o.params)
  const api = dockApi
  if (!api) {
    if (mobileRouter) {
      if (!mobileRouter({ kind: o.kind, id, title: o.title, params: o.params ?? {} }) && o.focus !== false) {
        toast('info', `${o.title ?? o.kind} opens on the desktop`)
      }
      return id
    }
    if (pending.length >= MAX_PENDING) pending.shift()
    pending.push({ ...o, id })
    return id
  }
  const existing = api.getPanel(id)
  if (existing) {
    if (o.params) existing.api.updateParameters(o.params)
    if (o.title) existing.api.setTitle(o.title)
    if (o.focus !== false) existing.api.setActive()
    return id
  }
  const base = { id, component: o.kind, title: o.title ?? o.kind, params: o.params ?? {}, inactive: o.focus === false }
  const pos = o.position ?? 'auto'
  if (pos === 'right' || pos === 'below') {
    const ref = api.activePanel
    api.addPanel(ref ? { ...base, position: { referencePanel: ref.id, direction: pos } } : base)
  } else if (pos === 'active' || api.groups.length === 0) {
    api.addPanel(base)
  } else {
    const isTerm = TERMINAL_KINDS.has(o.kind)
    const byId = (gid: string | null) => (gid ? api.groups.find((g) => g.id === gid) : undefined)
    if (isTerm) {
      const g = byId(lastTermGroup) ?? api.groups.find(groupHasTerminals) ?? api.groups[0]
      api.addPanel({ ...base, position: { referenceGroup: g } })
      lastTermGroup = g.id
    } else {
      const g =
        byId(lastDocGroup) ?? api.groups.find((x) => !groupHasTerminals(x) && x.panels.length > 0) ?? api.groups.find((x) => x.panels.length === 0)
      if (g) {
        api.addPanel({ ...base, position: { referenceGroup: g } })
        lastDocGroup = g.id
      } else {
        const termGroup = api.groups.find(groupHasTerminals) ?? api.groups[0]
        const panel = api.addPanel({ ...base, position: { referenceGroup: termGroup, direction: 'right' } })
        lastDocGroup = panel.group.id
      }
    }
  }
  return id
}

export function closePanel(id: string) {
  const p = dockApi?.getPanel(id)
  if (p) dockApi!.removePanel(p)
}

export function focusPanel(id: string): boolean {
  const p = dockApi?.getPanel(id)
  p?.api.setActive()
  return !!p
}

export function isPanelOpen(id: string): boolean {
  return !!dockApi?.getPanel(id)
}

// ---------------------------------------------------------------- tool windows

/** Show a tool window by id (looked up in the feature registry for its side). */
export function showToolWindow(id: string, side?: Side) {
  const s = side ?? toolWindowSides.get(id)
  if (s) useUi.getState().showToolWindow(s, id)
}

export const toolWindowSides = new Map<string, Side>()

// ---------------------------------------------------------------- toasts

export type ToastLevel = 'info' | 'success' | 'warning' | 'error'

export interface ToastAction {
  label: string
  run: () => void
  variant?: 'primary' | 'danger'
}

export interface Toast {
  id: number
  level: ToastLevel
  message: string
  detail?: string
  /** Text shown whole in monospace on its own lines (e.g. the command a request runs). */
  code?: string
  action?: { label: string; run: () => void }
  /** Several buttons in a row (after `action`), e.g. Allow / Deny. */
  actions?: ToastAction[]
  /** ms; 0 = sticky */
  timeout: number
}

interface ToastState {
  toasts: Toast[]
  push: (t: Omit<Toast, 'id'>) => number
  dismiss: (id: number) => void
}

let toastSeq = 1
export const useToasts = create<ToastState>()((set) => ({
  toasts: [],
  push: (t) => {
    const id = toastSeq++
    set((s) => ({ toasts: [...s.toasts.slice(-4), { ...t, id }] }))
    if (t.timeout > 0) window.setTimeout(() => set((s) => ({ toasts: s.toasts.filter((x) => x.id !== id) })), t.timeout)
    return id
  },
  dismiss: (id) => set((s) => ({ toasts: s.toasts.filter((x) => x.id !== id) })),
}))

export function toast(
  level: ToastLevel,
  message: string,
  opts: { detail?: string; code?: string; action?: Toast['action']; actions?: ToastAction[]; timeout?: number } = {},
) {
  return useToasts.getState().push({
    level,
    message,
    detail: opts.detail,
    code: opts.code,
    action: opts.action,
    actions: opts.actions,
    timeout: opts.timeout ?? (level === 'error' ? 9000 : 4500),
  })
}

/** Remove a toast before its time (e.g. what it asked about was settled elsewhere). */
export function dismissToast(id: number) {
  useToasts.getState().dismiss(id)
}

/** Toast an error from a failed request. */
export function toastError(e: unknown, prefix?: string) {
  const msg = e instanceof Error ? e.message : String(e)
  toast('error', prefix ? `${prefix}: ${msg}` : msg)
}

// ---------------------------------------------------------------- dialogs

export interface ConfirmOptions {
  title: string
  message?: string
  confirmLabel?: string
  danger?: boolean
  /** Require typing this text to enable the confirm button (deploys to production). */
  typed?: string
}

export interface PromptOptions {
  title: string
  label?: string
  initial?: string
  placeholder?: string
  multiline?: boolean
  confirmLabel?: string
}

type Dialog =
  | { kind: 'confirm'; opts: ConfirmOptions; resolve: (v: boolean) => void }
  | { kind: 'prompt'; opts: PromptOptions; resolve: (v: string | null) => void }

export const useDialogs = create<{ current: Dialog | null; set: (d: Dialog | null) => void }>()((set) => ({
  current: null,
  set: (d) => set({ current: d }),
}))

export function confirmDialog(opts: ConfirmOptions): Promise<boolean> {
  return new Promise((resolve) => useDialogs.getState().set({ kind: 'confirm', opts, resolve }))
}

export function promptDialog(opts: PromptOptions): Promise<string | null> {
  return new Promise((resolve) => useDialogs.getState().set({ kind: 'prompt', opts, resolve }))
}

// ---------------------------------------------------------------- common actions

/** Open Settings, at a section when given (`projects`, `agents`, `integrations`, `secrets`, `raw`…). */
export function openSettings(section?: string) {
  openPanel({ kind: 'settings', id: 'settings', title: 'Settings', params: section ? { section } : {} })
}

/** Ask for a directory and add it as a project (project switcher, palette, first-run banner). */
export async function addProjectInteractive() {
  const path = await promptDialog({ title: 'Add project', label: 'Directory (absolute or ~/…)', placeholder: '~/workspace/my-app' })
  if (!path) return
  try {
    const r = await http.post<{ id: string | null }>('/api/projects', { path })
    if (r.id) useUi.getState().setProject(r.id)
    toast('success', 'Project added')
  } catch (e) {
    toastError(e)
  }
}
