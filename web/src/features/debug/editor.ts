// The debugger in the editor, without touching the files feature: every `file:` model
// (`file:///<projectId>/<path>`, see features/files/buffers.ts) gets model decorations
// for its breakpoints and for the execution point of suspended sessions; every editor
// gets gutter clicks, a context menu, actions (Ctrl+F8, Alt+F9, Ctrl+Shift+F8) and
// evaluate-on-hover while a session is suspended.
//
// Monaco loads lazily with the first editor. Until then nothing here runs: a cheap
// poll notices `MonacoEnvironment` (set by lib/monacoSetup) and then hooks the
// models and editors that exist and every later one.

import type { editor, IDisposable } from 'monaco-editor'
import { showMenu, type MenuEntry } from '@/ui'
import { toast } from '@/shell/actions'
import { addWatch, breakpointAt, breakpointsMoved, currentSession, removeBreakpoint, runToCursor, toggleBreakpoint, updateBreakpoint, viewBreakpoints } from './actions'
import { cachedBreakpoints, debugApi, queryClient } from './api'
import { openBreakpointDialog } from './BreakpointDialog'
import { modelFile, samePath } from '@/features/files/modelAccess'
import { expressionAt, fileBreakpoints, glyphKind, glyphTitle, isLive, sourceViewSession } from './logic'
import { sessionsOf, useDebug } from './store'
import type { Frame } from './types'

type Monaco = (typeof import('@/lib/monacoSetup'))['monaco']

interface ModelEntry {
  model: editor.ITextModel
  projectId: string | null
  path: string
  decorations: string[]
  /** Decoration id → breakpoint id, for following breakpoints through edits. */
  bpOf: Map<string, string>
  moveTimer?: number
  disposables: IDisposable[]
}

let ns: Monaco | null = null
const entries = new Map<editor.ITextModel, ModelEntry>()
let lastEditor: editor.ICodeEditor | null = null
let frame = 0

/** The editor that had focus last (palette commands act on it). */
export function lastFocusedEditor(): { editor: editor.ICodeEditor; projectId: string | null; path: string } | null {
  const m = lastEditor?.getModel()
  if (!lastEditor || !m || m.isDisposed()) return null
  const f = modelFile(m.uri)
  return f ? { editor: lastEditor, ...f } : null
}

// ---------------------------------------------------------------- decorations

/** Whether a frame is in the model's file: a project file, or an absolute one (a Windows
 *  adapter may spell `C:\x` as `c:/x`). */
function sameSource(e: ModelEntry, pid: string, f: Frame | undefined): boolean {
  const src = f?.source
  if (!src?.path) return false
  return (src.inProject ? e.projectId === pid : e.projectId === null) && samePath(e.path, src.path)
}

/** Apply breakpoint moves the model tracked but nobody saved yet. */
function flushMove(e: ModelEntry) {
  window.clearTimeout(e.moveTimer)
  e.moveTimer = undefined
  if (!e.projectId || e.model.isDisposed()) return
  const moved = new Map<string, number>()
  for (const [deco, id] of e.bpOf) {
    const r = e.model.getDecorationRange(deco)
    if (r) moved.set(id, r.startLineNumber)
  }
  if (moved.size) void breakpointsMoved(e.projectId, e.path, moved)
}

function decorate(e: ModelEntry) {
  if (!ns || e.model.isDisposed()) return
  if (e.moveTimer) flushMove(e)
  const st = useDebug.getState()
  const pid = e.projectId
  const view = pid ? cachedBreakpoints(pid) : undefined
  const bps = view ? fileBreakpoints(view.breakpoints, e.path) : []
  const muted = view?.muted ?? false
  const live = pid ? sessionsOf(st.sessions, pid).some(isLive) : false
  const lines = new Map<number, { bp?: (typeof bps)[number]; exec?: boolean; frame?: boolean }>()
  for (const b of bps) lines.set(b.line, { ...lines.get(b.line), bp: b })
  for (const s of Object.values(st.sessions)) {
    if (s.state !== 'stopped') continue
    const stack = st.stacks[s.id]
    if (!stack || stack.epoch !== s.stopEpoch || !stack.frames.length) continue
    const top = stack.frames[0]
    if (sameSource(e, s.projectId, top)) lines.set(top.line, { ...lines.get(top.line), exec: true })
    const sel = st.selection[s.id]?.frameIndex ?? 0
    const f = stack.frames[sel]
    if (sel > 0 && sameSource(e, s.projectId, f)) lines.set(f.line, { ...lines.get(f.line), frame: true })
  }
  const max = e.model.getLineCount()
  const decos: editor.IModelDeltaDecoration[] = []
  const ids: (string | null)[] = []
  for (const [line, v] of lines) {
    if (line < 1 || line > max) continue
    const cls = ['wb-dbg-glyph']
    if (v.bp) cls.push(glyphKind(v.bp, muted, live))
    if (v.exec) cls.push('exec')
    else if (v.frame) cls.push('frame')
    const lineClass = v.exec ? 'wb-dbg-exec-line' : v.frame ? 'wb-dbg-frame-line' : v.bp && v.bp.enabled && !muted && !v.bp.logMessage ? 'wb-dbg-bp-line' : undefined
    decos.push({
      range: new ns.Range(line, 1, line, 1),
      options: {
        isWholeLine: true,
        className: lineClass,
        glyphMarginClassName: cls.join(' '),
        glyphMarginHoverMessage: v.bp ? { value: glyphTitle(v.bp, muted) } : v.exec ? { value: 'Execution point' } : undefined,
        stickiness: ns.editor.TrackedRangeStickiness.NeverGrowsWhenTypingAtEdges,
      },
    })
    ids.push(v.bp?.id ?? null)
  }
  e.decorations = e.model.deltaDecorations(e.decorations, decos)
  e.bpOf = new Map()
  e.decorations.forEach((d, i) => {
    const id = ids[i]
    if (id) e.bpOf.set(d, id)
  })
}

function decorateAll() {
  frame = 0
  for (const e of entries.values()) decorate(e)
}

function scheduleAll() {
  if (frame) return
  frame = window.requestAnimationFrame(decorateAll)
}

function track(model: editor.ITextModel) {
  if (entries.has(model)) return
  const f = modelFile(model.uri)
  if (!f) return
  const e: ModelEntry = { model, projectId: f.projectId, path: f.path, decorations: [], bpOf: new Map(), disposables: [] }
  entries.set(model, e)
  e.disposables.push(
    model.onDidChangeContent(() => {
      if (!e.bpOf.size) return
      window.clearTimeout(e.moveTimer)
      e.moveTimer = window.setTimeout(() => flushMove(e), 700)
    }),
    model.onWillDispose(() => {
      window.clearTimeout(e.moveTimer)
      e.disposables.forEach((d) => d.dispose())
      entries.delete(model)
    }),
  )
  decorate(e)
}

// ---------------------------------------------------------------- editors

function fileOf(ed: editor.ICodeEditor): { projectId: string | null; path: string } | null {
  const m = ed.getModel()
  return m ? modelFile(m.uri) : null
}

function gutterMenu(pid: string, path: string, line: number, at: { clientX: number; clientY: number }) {
  const bp = breakpointAt(pid, path, line)
  const s = currentSession(pid)
  const items: MenuEntry[] = bp
    ? [
        { label: 'Edit Breakpoint…', run: () => openBreakpointDialog(pid, path, line) },
        { label: bp.enabled ? 'Disable Breakpoint' : 'Enable Breakpoint', run: () => void updateBreakpoint(pid, path, line, { enabled: !bp.enabled }) },
        { label: 'Remove Breakpoint', danger: true, run: () => void removeBreakpoint(pid, path, line) },
      ]
    : [
        { label: 'Add Breakpoint', shortcut: 'Ctrl+F8', run: () => void toggleBreakpoint(pid, path, line) },
        { label: 'Add Conditional Breakpoint…', run: () => openBreakpointDialog(pid, path, line, { focus: 'condition' }) },
        { label: 'Add Log Point…', run: () => openBreakpointDialog(pid, path, line, { focus: 'log' }) },
      ]
  items.push('separator', { label: 'Run to Cursor', shortcut: 'Alt+F9', disabled: s?.state !== 'stopped', run: () => void runToCursor(pid, path, line) })
  items.push({ label: 'View Breakpoints', shortcut: 'Ctrl+Shift+F8', run: viewBreakpoints })
  showMenu(at, items)
}

const hooked = new WeakSet<editor.ICodeEditor>()

function hookEditor(ed: editor.ICodeEditor) {
  if (!ns || hooked.has(ed)) return
  hooked.add(ed)
  const monaco = ns
  const hint = ed.createDecorationsCollection()
  // The glyph margin is shared: Monaco puts its code-action lightbulb there (lsp quick
  // fixes) when the line leaves no room for it; a click on it is the lightbulb's.
  const foreign = (t: editor.IMouseTarget) => t.element instanceof Element && !!t.element.closest('[class*="codicon-gutter-lightbulb"]')
  const glyph = (t: editor.IMouseTarget) => t.type === monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN && !foreign(t)
  const subs: IDisposable[] = [
    ed.onDidFocusEditorText(() => (lastEditor = ed)),
    ed.onMouseDown((ev) => {
      if (!glyph(ev.target)) return
      const f = fileOf(ed)
      const line = ev.target.position?.lineNumber
      if (!f?.projectId || !line) return
      ev.event.preventDefault()
      if (ev.event.rightButton) return // the context menu handler shows the menu
      if (!ev.event.leftButton) return
      hint.clear()
      const bp = breakpointAt(f.projectId, f.path, line)
      if (ev.event.shiftKey) openBreakpointDialog(f.projectId, f.path, line, { focus: 'log' })
      else if ((ev.event.ctrlKey || ev.event.metaKey) && bp) void updateBreakpoint(f.projectId, f.path, line, { enabled: !bp.enabled })
      else void toggleBreakpoint(f.projectId, f.path, line)
    }),
    ed.onContextMenu((ev) => {
      if (!glyph(ev.target)) return
      const f = fileOf(ed)
      const line = ev.target.position?.lineNumber
      if (!f?.projectId || !line) return
      ev.event.preventDefault()
      ev.event.stopPropagation()
      gutterMenu(f.projectId, f.path, line, { clientX: ev.event.posx, clientY: ev.event.posy })
    }),
    ed.onMouseMove((ev) => {
      const f = fileOf(ed)
      const line = ev.target.position?.lineNumber
      if (!glyph(ev.target) || !f?.projectId || !line || breakpointAt(f.projectId, f.path, line)) {
        if (hint.length) hint.clear()
        return
      }
      hint.set([{ range: new monaco.Range(line, 1, line, 1), options: { glyphMarginClassName: 'wb-dbg-glyph hint' } }])
    }),
    ed.onMouseLeave(() => hint.clear()),
  ]
  const at = () => {
    const f = fileOf(ed)
    const pos = ed.getPosition()
    return f && pos ? { ...f, line: pos.lineNumber } : null
  }
  // Actions (not commands): their keys apply only while this editor has focus, and
  // they go away with it. Only standalone editors have them.
  const sa = ed as Partial<editor.IStandaloneCodeEditor>
  const addAction = typeof sa.addAction === 'function' ? (a: editor.IActionDescriptor) => sa.addAction!(a) : null
  if (addAction) subs.push(
    addAction({
      id: 'wb.debug.toggleBreakpoint',
      label: 'Toggle Line Breakpoint',
      keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyCode.F8],
      contextMenuGroupId: 'y_debug',
      contextMenuOrder: 1,
      run: () => {
        const a = at()
        if (a?.projectId) void toggleBreakpoint(a.projectId, a.path, a.line)
        else toast('info', 'Breakpoints are for files of a project')
      },
    }),
    addAction({
      id: 'wb.debug.runToCursor',
      label: 'Run to Cursor',
      keybindings: [monaco.KeyMod.Alt | monaco.KeyCode.F9],
      contextMenuGroupId: 'y_debug',
      contextMenuOrder: 2,
      run: () => {
        const a = at()
        if (a?.projectId) void runToCursor(a.projectId, a.path, a.line)
      },
    }),
    addAction({
      id: 'wb.debug.addWatch',
      label: 'Add to Watches',
      contextMenuGroupId: 'y_debug',
      contextMenuOrder: 3,
      run: () => {
        const a = at()
        const m = ed.getModel()
        const sel = ed.getSelection()
        if (!a?.projectId || !m || !sel) return
        let expr = sel.isEmpty() ? '' : m.getValueInRange(sel).trim()
        if (!expr) {
          const pos = ed.getPosition()
          expr = (pos && expressionAt(m.getLineContent(pos.lineNumber), pos.column)?.expr) ?? ''
        }
        if (!expr || expr.includes('\n')) return
        void addWatch(a.projectId, expr)
      },
    }),
    addAction({
      id: 'wb.debug.editBreakpoint',
      label: 'Edit Breakpoint / View Breakpoints',
      keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyMod.Shift | monaco.KeyCode.F8],
      run: () => {
        const a = at()
        if (a?.projectId && breakpointAt(a.projectId, a.path, a.line)) openBreakpointDialog(a.projectId, a.path, a.line)
        else viewBreakpoints()
      },
    }),
  )
  ed.onDidDispose(() => {
    subs.forEach((d) => d.dispose())
    if (lastEditor === ed) lastEditor = null
  })
}

// ---------------------------------------------------------------- hover

function registerHover(monaco: Monaco) {
  monaco.languages.registerHoverProvider('*', {
    provideHover: async (model, position, token) => {
      const f = modelFile(model.uri)
      // A project file: the project's current session; a debug source view: its own.
      const viewOf = sourceViewSession(model.uri)
      const s = f?.projectId ? currentSession(f.projectId) : viewOf ? (useDebug.getState().sessions[viewOf] ?? null) : null
      if (!s || s.state !== 'stopped') return null
      const e = expressionAt(model.getLineContent(position.lineNumber), position.column)
      if (!e || e.expr.length > 200) return null
      const st = useDebug.getState()
      const stack = st.stacks[s.id]
      const frameId = stack?.epoch === s.stopEpoch ? stack.frames[st.selection[s.id]?.frameIndex ?? 0]?.id : undefined
      if (frameId === undefined) return null
      const r = await debugApi.evaluate(s.projectId, s.id, e.expr, 'hover', frameId).catch(() => null)
      if (!r || token.isCancellationRequested || !r.value) return null
      const value = r.value.length > 2000 ? r.value.slice(0, 2000) + '…' : r.value
      return {
        range: new monaco.Range(position.lineNumber, e.start, position.lineNumber, e.end),
        contents: [{ value: '```\n' + `${e.expr} = ${value}` + (r.type ? `\n(${r.type})` : '') + '\n```' }],
      }
    },
  })
}

// ---------------------------------------------------------------- install

function setup(monaco: Monaco) {
  if (ns) return
  ns = monaco
  for (const m of monaco.editor.getModels()) track(m)
  monaco.editor.onDidCreateModel(track)
  for (const e of monaco.editor.getEditors()) hookEditor(e)
  // The event fires inside the editor's constructor, before a standalone editor has
  // its keybinding service: hook it once construction is done.
  monaco.editor.onDidCreateEditor((e) => queueMicrotask(() => hookEditor(e)))
  registerHover(monaco)
  useDebug.subscribe((s, prev) => {
    if (s.sessions !== prev.sessions || s.stacks !== prev.stacks || s.selection !== prev.selection) scheduleAll()
  })
  queryClient()
    ?.getQueryCache()
    .subscribe((ev) => {
      const k = ev.query.queryKey
      if (k[0] === 'debug' && k[1] === 'breakpoints' && ev.type === 'updated') scheduleAll()
    })
}

let polling = false

/** Hook Monaco once it has loaded (called by the provider; idempotent). */
export function installEditorIntegration() {
  if (polling || ns) return
  polling = true
  const timer = window.setInterval(() => {
    if (!(self as unknown as { MonacoEnvironment?: unknown }).MonacoEnvironment) return
    window.clearInterval(timer)
    void import('@/lib/monacoSetup').then(({ monaco }) => setup(monaco))
  }, 400)
}
