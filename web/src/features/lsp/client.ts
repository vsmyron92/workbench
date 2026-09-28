// The browser side of code intelligence: which projects have it on, one socket per
// such project (`LspConnection`), the Monaco models of their files (the files slice's
// `file:///<pid>/…` models and this slice's `lsp-src://<pid>/…` library sources),
// diagnostics as markers, and the editors (for actions and the enable banner).
//
// Nothing here loads Monaco: `install()` runs once Monaco is loaded for an editor.

import type { editor, IDisposable } from 'monaco-editor'
import { create } from 'zustand'
import { subscribe } from '@/api/events'
import { toast } from '@/shell/actions'
import { onBufferRevision, modelUriString } from '@/features/files/modelAccess'
import { lspApi, lspKeys, lspQueryClient, putStatus, type LspDiagnostic, type LspStatus } from './api'
import { LspConnection, type ConnState } from './connection'
import { projectOfUri, toMarker } from './convert'

type MonacoNs = (typeof import('@/lib/monacoSetup'))['monaco']

/**
 * React-visible runtime state: connection states, the model URI of the editor that had
 * focus last, and a tick when servers or documents change.
 */
export const useLspRuntime = create<{ conns: Record<string, ConnState>; tick: number; activeUri: string | null }>()(() => ({ conns: {}, tick: 0, activeUri: null }))

function bump() {
  useLspRuntime.setState((s) => ({ tick: s.tick + 1 }))
}

interface ModelEntry {
  model: editor.ITextModel
  uri: string
  pid: string
  version: number
  subs: IDisposable[]
}

export interface Target {
  conn: LspConnection
  pid: string
  uri: string
  server: string
  caps: Record<string, unknown>
}

/** A capability is on: present and not `false`. */
export function has(caps: Record<string, unknown> | undefined, key: string): boolean {
  const v = caps?.[key]
  return v !== undefined && v !== null && v !== false
}

class LspClient {
  monaco: MonacoNs | null = null
  private installing: Promise<void> | null = null
  private conns = new Map<string, LspConnection>()
  /** Known enablement per project. */
  private enabled = new Map<string, boolean>()
  private checking = new Map<string, Promise<boolean>>()
  private models = new Map<string, ModelEntry>()
  /** uri → server → diagnostics (for markers and code action context). */
  private diags = new Map<string, Map<string, LspDiagnostic[]>>()
  private closeTimers = new Map<string, ReturnType<typeof setTimeout>>()
  /** Called when a server says capabilities changed (providers re-register trigger characters). */
  capsListeners = new Set<() => void>()
  refreshListeners = new Set<(server: string, what: string) => void>()
  /** Answers `workspace/applyEdit` / `window/showMessageRequest` (set by the provider component). */
  requestHandler: ((pid: string, server: string, method: string, params: unknown) => Promise<unknown>) | null = null
  editors = new Set<editor.ICodeEditor>()
  lastEditor: editor.ICodeEditor | null = null
  private recentMessages = new Map<string, number>()

  /** Load Monaco's hooks once (when the first editor exists). */
  install(): Promise<void> {
    if (!this.installing) this.installing = this.doInstall()
    return this.installing
  }

  private async doInstall() {
    const { monaco } = await import('@/lib/monacoSetup')
    this.monaco = monaco
    const { registerProviders } = await import('./providers')
    const { installEditorHooks } = await import('./editorHooks')
    registerProviders(monaco, this)
    installEditorHooks(monaco, this)
    monaco.editor.onDidCreateModel((m) => this.attach(m))
    monaco.editor.onWillDisposeModel((m) => this.detach(m))
    for (const m of monaco.editor.getModels()) this.attach(m)
    subscribe('lsp.state', (ev) => {
      const pid = ev.projectId
      const data = ev.data as { enabled?: boolean }
      if (pid && typeof data?.enabled === 'boolean') this.setEnabled(pid, data.enabled)
    })
    subscribe('resync', () => {
      for (const pid of this.enabled.keys()) this.recheck(pid)
    })
    onBufferRevision((projectId, path) => {
      if (!projectId) return
      const uri = modelUriString(projectId, path)
      this.conns.get(projectId)?.saved(uri)
    })
  }

  // ------------------------------------------------------------ projects

  isEnabled(pid: string): boolean | undefined {
    return this.enabled.get(pid)
  }

  /** Enablement of a project (asked once, then kept fresh by `lsp.state`). */
  ensureProject(pid: string): Promise<boolean> {
    const known = this.enabled.get(pid)
    if (known !== undefined) return Promise.resolve(known)
    let p = this.checking.get(pid)
    if (!p) {
      p = lspApi
        .status(pid)
        .then((s) => {
          putStatus(s)
          this.enabled.set(pid, s.enabled)
          return s.enabled
        })
        .catch(() => false)
        .finally(() => this.checking.delete(pid))
      this.checking.set(pid, p)
    }
    return p
  }

  private recheck(pid: string) {
    lspApi.status(pid).then(
      (s) => {
        putStatus(s)
        this.setEnabled(pid, s.enabled)
      },
      () => {},
    )
  }

  /** The user (here or in another tab) turned it on or off. */
  setEnabled(pid: string, on: boolean) {
    const was = this.enabled.get(pid)
    this.enabled.set(pid, on)
    void lspQueryClient()?.invalidateQueries({ queryKey: lspKeys.status(pid) })
    if (on && !was) {
      for (const e of this.models.values()) if (e.pid === pid) this.track(e)
    } else if (!on) {
      this.closeConnection(pid)
      // Deleting while iterating is safe for a Map.
      for (const uri of this.diags.keys()) if (projectOfUri(uri) === pid) this.setDiagnostics(uri, null, [])
    }
    bump()
  }

  /** Status answer from an action (enable, restart…). */
  statusChanged(s: LspStatus) {
    putStatus(s)
    this.setEnabled(s.projectId, s.enabled)
  }

  connection(pid: string): LspConnection | null {
    return this.conns.get(pid) ?? null
  }

  /** The project's socket, opened if code intelligence is on (for Go to Symbol without an open file). */
  async connect(pid: string): Promise<LspConnection | null> {
    if (!(await this.ensureProject(pid))) return null
    const c = this.openConnection(pid)
    // Without editors of the project, it closes again after a while.
    this.maybeIdle(pid)
    return c
  }

  private openConnection(pid: string): LspConnection {
    const t = this.closeTimers.get(pid)
    if (t) {
      clearTimeout(t)
      this.closeTimers.delete(pid)
    }
    let c = this.conns.get(pid)
    if (c) return c
    c = new LspConnection(pid, {
      onState: (s) => {
        useLspRuntime.setState((st) => ({ conns: { ...st.conns, [pid]: s } }))
        if (s === 'disabled') this.setEnabled(pid, false)
      },
      // A document got its server: features that ask per document (semantic tokens,
      // inlay hints) ask again, since they may have run before it was ready.
      onOpened: () => {
        this.capsListeners.forEach((l) => l())
        bump()
      },
      onDiagnostics: (uri, server, list) => this.setDiagnostics(uri, server, list),
      onCaps: () => {
        this.capsListeners.forEach((l) => l())
        bump()
      },
      onDown: () => bump(),
      onMessage: (server, level, message) => this.serverMessage(server, level, message),
      onRefresh: (server, what) => this.refreshListeners.forEach((l) => l(server, what)),
      onRequest: (server, method, params) => this.requestHandler?.(pid, server, method, params) ?? Promise.resolve(null),
      onRefused: () => this.recheck(pid),
    })
    this.conns.set(pid, c)
    useLspRuntime.setState((st) => ({ conns: { ...st.conns, [pid]: c!.state } }))
    return c
  }

  private closeConnection(pid: string) {
    const c = this.conns.get(pid)
    if (!c) return
    c.dispose()
    this.conns.delete(pid)
    useLspRuntime.setState((st) => {
      const { [pid]: _gone, ...rest } = st.conns
      return { conns: rest }
    })
  }

  /** No model of the project is left: close its socket after a while. */
  private maybeIdle(pid: string) {
    if ([...this.models.values()].some((e) => e.pid === pid)) return
    if (this.closeTimers.has(pid)) return
    this.closeTimers.set(
      pid,
      setTimeout(() => {
        this.closeTimers.delete(pid)
        if (![...this.models.values()].some((e) => e.pid === pid)) this.closeConnection(pid)
      }, 60_000),
    )
  }

  private serverMessage(server: string, level: number, message: string) {
    if (level > 2 || !message) return
    const key = `${server}:${message}`
    const now = Date.now()
    if ((this.recentMessages.get(key) ?? 0) > now - 60_000) return
    this.recentMessages.set(key, now)
    if (this.recentMessages.size > 200) this.recentMessages.clear()
    toast(level === 1 ? 'error' : 'warning', `${server}: ${message.split('\n')[0].slice(0, 300)}`)
  }

  // ------------------------------------------------------------ models

  private attach(model: editor.ITextModel) {
    const scheme = model.uri.scheme
    if (scheme !== 'file' && scheme !== 'lsp-src') return
    const uri = model.uri.toString()
    const pid = projectOfUri(uri)
    if (!pid || this.models.has(uri)) return
    const entry: ModelEntry = { model, uri, pid, version: 0, subs: [] }
    entry.subs.push(
      model.onDidChangeContent(() => {
        entry.version++
        this.conns.get(pid)?.changed(uri)
      }),
    )
    this.models.set(uri, entry)
    void this.ensureProject(pid).then((on) => {
      if (on && this.models.get(uri) === entry) this.track(entry)
    })
    // Markers for diagnostics that arrived before the model (a file opened from Problems).
    this.applyMarkers(uri)
    bump()
  }

  private track(e: ModelEntry) {
    const c = this.openConnection(e.pid)
    c.track({ uri: e.uri, languageId: e.model.getLanguageId(), text: () => e.model.getValue(), version: () => e.version })
  }

  private detach(model: editor.ITextModel) {
    const uri = model.uri.toString()
    const e = this.models.get(uri)
    if (!e) return
    e.subs.forEach((d) => d.dispose())
    this.models.delete(uri)
    this.conns.get(e.pid)?.untrack(uri)
    this.maybeIdle(e.pid)
    bump()
  }

  // ------------------------------------------------------------ diagnostics

  private setDiagnostics(uri: string, server: string | null, list: LspDiagnostic[]) {
    if (server === null) this.diags.delete(uri)
    else {
      const m = this.diags.get(uri) ?? new Map<string, LspDiagnostic[]>()
      if (list.length) m.set(server, list.map((d) => ({ ...d, server })))
      else m.delete(server)
      if (m.size) this.diags.set(uri, m)
      else this.diags.delete(uri)
    }
    this.applyMarkers(uri)
  }

  private applyMarkers(uri: string) {
    const monaco = this.monaco
    if (!monaco) return
    const model = this.models.get(uri)?.model
    if (!model || model.isDisposed()) return
    const list = this.diagnosticsOf(uri)
    monaco.editor.setModelMarkers(
      model,
      'lsp',
      list.map((d) => {
        const m = toMarker(d)
        return {
          ...m,
          code: typeof m.code === 'object' ? { value: m.code.value, target: monaco.Uri.parse(m.code.target) } : m.code,
          relatedInformation: m.relatedInformation?.map((r) => ({ ...r, resource: monaco.Uri.parse(r.resource) })),
        } as editor.IMarkerData
      }),
    )
  }

  diagnosticsOf(uri: string): LspDiagnostic[] {
    const m = this.diags.get(uri)
    return m ? [...m.values()].flat() : []
  }

  // ------------------------------------------------------------ targets

  /** The ready server behind a model, or null (off, starting, no server for the file). */
  target(model: editor.ITextModel): Target | null {
    const uri = model.uri.toString()
    const pid = projectOfUri(uri)
    if (!pid) return null
    const conn = this.conns.get(pid)
    if (!conn || conn.state !== 'open') return null
    const server = conn.serverOf(uri)
    if (!server) return null
    const caps = conn.servers.get(server)
    if (!caps) return null
    return { conn, pid, uri, server, caps }
  }

  /** Why a model has no target (for messages on explicit actions). */
  whyNot(model: editor.ITextModel): string {
    const uri = model.uri.toString()
    const pid = projectOfUri(uri)
    if (!pid) return 'Code intelligence works on project files'
    if (this.enabled.get(pid) === false) return 'Code intelligence is off for this project'
    const conn = this.conns.get(pid)
    if (!conn || conn.state !== 'open') return 'Code intelligence is connecting…'
    const server = conn.serverOf(uri)
    if (server === null) return conn.docError(uri) ?? 'No language server handles this file'
    if (server === undefined) return 'The file is not synchronized yet'
    return `${server} is still starting`
  }

  models_(): ModelEntry[] {
    return [...this.models.values()]
  }
}

export type { LspClient }

/** The one client. */
export const lsp = new LspClient()
