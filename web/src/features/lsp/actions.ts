// What the editor's CLion-style actions do: Ctrl+B, Ctrl+Alt+B, Ctrl+Shift+B, Alt+F7,
// Ctrl+Alt+F7, Shift+F6, Ctrl+F12. Each asks the model's server directly; a single
// target navigates, several open a chooser at the caret.

import type { editor } from 'monaco-editor'
import { showToolWindow, toast, toastError } from '@/shell/actions'
import type { LspDocumentSymbol, LspHierarchyItem, LspLocation, LspLocationLink, LspRange, LspSymbolInformation, LspTextEdit } from './api'
import { has, lsp, type Target } from './client'
import { flattenSymbols, toLocs, toLspPosition, toLspRange, toRange, wordAt, type Loc } from './convert'
import { openLocation } from './nav'
import { useHierarchy, usePopups, useUsages, type HierarchyKind } from './store'

function need(ed: editor.ICodeEditor): { t: Target; model: editor.ITextModel } | null {
  const model = ed.getModel()
  if (!model) return null
  const t = lsp.target(model)
  if (!t) {
    const why = lsp.whyNot(model)
    const pid = /^file:\/\/\/([^/~][^/]*)\//.exec(model.uri.toString())?.[1]
    if (pid && lsp.isEnabled(pid) === false) {
      toast('info', why, { action: { label: 'Enable…', run: () => usePopups.getState().set({ enable: { projectId: pid } }) } })
    } else toast('info', why)
    return null
  }
  return { t, model }
}

function wordName(model: editor.ITextModel, pos: { lineNumber: number; column: number }): string {
  return model.getWordAtPosition(pos)?.word ?? wordAt(model.getLineContent(pos.lineNumber), pos.column - 1)?.word ?? 'symbol'
}

/** Screen position just below the caret, for popups. */
export function caretAnchor(ed: editor.ICodeEditor): { x: number; y: number } {
  const pos = ed.getPosition()
  const dom = ed.getDomNode()
  const rect = dom?.getBoundingClientRect()
  const vis = pos ? ed.getScrolledVisiblePosition(pos) : null
  if (!rect || !vis) return { x: window.innerWidth / 2 - 250, y: 120 }
  return { x: rect.left + vis.left, y: rect.top + vis.top + vis.height + 2 }
}

async function locations(t: Target, method: string, capability: string, pos: { lineNumber: number; column: number }, extra: object = {}): Promise<Loc[]> {
  if (!has(t.caps, capability)) return []
  const r = await t.conn.request<LspLocation | LspLocation[] | LspLocationLink[] | null>(method, { textDocument: { uri: t.uri }, position: toLspPosition(pos), ...extra })
  return toLocs(r.result)
}

function contains(r: LspRange, line: number, character: number): boolean {
  const after = line > r.start.line || (line === r.start.line && character >= r.start.character)
  const before = line < r.end.line || (line === r.end.line && character <= r.end.character)
  return after && before
}

/** Move to a location: in this editor when it is the same file, else open it. */
export function navigate(ed: editor.ICodeEditor | undefined, loc: Loc) {
  const model = ed?.getModel()
  if (ed && model && model.uri.toString() === loc.uri) {
    const r = toRange(loc.range)
    ed.setSelection({ startLineNumber: r.startLineNumber, startColumn: r.startColumn, endLineNumber: r.startLineNumber, endColumn: r.startLineNumber === r.endLineNumber ? r.endColumn : r.startColumn })
    ed.revealRangeInCenterIfOutsideViewport(r)
    ed.focus()
    return
  }
  openLocation(loc.uri, loc.range)
}

function choose(ed: editor.ICodeEditor, title: string, locs: Loc[]) {
  const a = caretAnchor(ed)
  usePopups.getState().set({ chooser: { title, locs, x: a.x, y: a.y, editor: ed } })
}

/** Ctrl+B / Ctrl+click: go to the declaration; on the declaration itself, show its usages. */
export async function gotoDeclaration(ed: editor.ICodeEditor) {
  const n = need(ed)
  const pos = ed.getPosition()
  if (!n || !pos) return
  const { t, model } = n
  try {
    let locs = await locations(t, 'textDocument/definition', 'definitionProvider', pos)
    if (!locs.length) locs = await locations(t, 'textDocument/declaration', 'declarationProvider', pos)
    if (!locs.length) {
      toast('info', 'Cannot find declaration to go to', { timeout: 2500 })
      return
    }
    // On the declaration's own name: its usages instead (CLion's Ctrl+B).
    if (locs.length === 1 && locs[0].uri === t.uri && contains(locs[0].range, pos.lineNumber - 1, pos.column - 1)) return showUsages(ed)
    if (locs.length === 1) return navigate(ed, locs[0])
    choose(ed, `Choose Declaration of ${wordName(model, pos)}`, locs)
  } catch (e) {
    toastError(e, 'Go to Declaration')
  }
}

async function gotoWith(ed: editor.ICodeEditor, method: string, capability: string, what: string) {
  const n = need(ed)
  const pos = ed.getPosition()
  if (!n || !pos) return
  if (!has(n.t.caps, capability)) {
    toast('info', `${n.t.server} does not offer ${what.toLowerCase()}`)
    return
  }
  try {
    const locs = await locations(n.t, method, capability, pos)
    if (!locs.length) toast('info', `No ${what.toLowerCase()} found`, { timeout: 2500 })
    else if (locs.length === 1) navigate(ed, locs[0])
    else choose(ed, `Choose ${what} of ${wordName(n.model, pos)}`, locs)
  } catch (e) {
    toastError(e, what)
  }
}

export const gotoImplementation = (ed: editor.ICodeEditor) => gotoWith(ed, 'textDocument/implementation', 'implementationProvider', 'Implementation')
export const gotoTypeDeclaration = (ed: editor.ICodeEditor) => gotoWith(ed, 'textDocument/typeDefinition', 'typeDefinitionProvider', 'Type Declaration')

/** Ctrl+Alt+F7: usages in a popup at the caret. */
export async function showUsages(ed: editor.ICodeEditor) {
  const n = need(ed)
  const pos = ed.getPosition()
  if (!n || !pos) return
  try {
    const locs = await locations(n.t, 'textDocument/references', 'referencesProvider', pos, { context: { includeDeclaration: false } })
    if (!locs.length) toast('info', `No usages of ${wordName(n.model, pos)} found`, { timeout: 2500 })
    else choose(ed, `Usages of ${wordName(n.model, pos)} (${locs.length})`, locs)
  } catch (e) {
    toastError(e, 'Show Usages')
  }
}

/** Alt+F7: every usage in the Find Usages window, grouped by file. */
export async function findUsages(ed: editor.ICodeEditor) {
  const n = need(ed)
  const pos = ed.getPosition()
  if (!n || !pos) return
  if (!has(n.t.caps, 'referencesProvider')) {
    toast('info', `${n.t.server} does not find usages`)
    return
  }
  const store = useUsages.getState()
  const id = store.add({ projectId: n.t.pid, title: wordName(n.model, pos), origin: { uri: n.t.uri, line: pos.lineNumber - 1 }, state: 'loading', locs: [] })
  showToolWindow('usages', 'bottom')
  try {
    const locs = await locations(n.t, 'textDocument/references', 'referencesProvider', pos, { context: { includeDeclaration: true } })
    useUsages.getState().update(id, { state: 'done', locs })
  } catch (e) {
    useUsages.getState().update(id, { state: 'error', error: e instanceof Error ? e.message : String(e) })
  }
}

/** Ctrl+Alt+H / Ctrl+H: the call or type hierarchy of the symbol at the caret, in the Hierarchy window. */
export async function showHierarchy(ed: editor.ICodeEditor, kind: HierarchyKind) {
  const n = need(ed)
  const pos = ed.getPosition()
  if (!n || !pos) return
  const cap = kind === 'call' ? 'callHierarchyProvider' : 'typeHierarchyProvider'
  if (!has(n.t.caps, cap)) {
    toast('info', `${n.t.server} has no ${kind} hierarchy`)
    return
  }
  try {
    const r = await n.t.conn.request<LspHierarchyItem[] | null>(kind === 'call' ? 'textDocument/prepareCallHierarchy' : 'textDocument/prepareTypeHierarchy', {
      textDocument: { uri: n.t.uri },
      position: toLspPosition(pos),
    })
    const root = r.result?.[0]
    if (!root) {
      toast('info', kind === 'call' ? 'No function or method at the caret' : 'No type at the caret', { timeout: 2500 })
      return
    }
    useHierarchy.getState().show({ projectId: n.t.pid, kind, root, direction: kind === 'call' ? 'incoming' : 'subtypes' })
    showToolWindow('hierarchy', 'right')
  } catch (e) {
    toastError(e, `Could not get the ${kind} hierarchy`)
  }
}

/** Shift+F6: ask for the new name, then preview every edit (RenameDialog). */
export async function renameAt(ed: editor.ICodeEditor) {
  const n = need(ed)
  const pos = ed.getPosition()
  if (!n || !pos) return
  const { t, model } = n
  if (!has(t.caps, 'renameProvider')) {
    toast('info', `${t.server} cannot rename`)
    return
  }
  let oldName = wordName(model, pos)
  let position = toLspPosition(pos)
  const prepare = (t.caps.renameProvider as { prepareProvider?: boolean } | boolean) as { prepareProvider?: boolean }
  if (typeof prepare === 'object' && prepare.prepareProvider) {
    try {
      const r = await t.conn.request<LspRange | { range: LspRange; placeholder: string } | { defaultBehavior: boolean } | null>('textDocument/prepareRename', {
        textDocument: { uri: t.uri },
        position,
      })
      const res = r.result
      if (!res) {
        toast('info', 'This element cannot be renamed', { timeout: 2500 })
        return
      }
      if ('placeholder' in res) {
        oldName = res.placeholder
        position = res.range.start
      } else if ('start' in res) {
        oldName = model.getValueInRange(toRange(res))
        position = res.start
      }
    } catch (e) {
      toast('info', e instanceof Error ? e.message : 'This element cannot be renamed')
      return
    }
  }
  usePopups.getState().set({ rename: { projectId: t.pid, server: t.server, uri: t.uri, position, oldName, editor: ed } })
}

/** Ctrl+F12: the file's structure in a filterable popup. */
export async function fileStructure(ed: editor.ICodeEditor) {
  const n = need(ed)
  if (!n) return
  if (!has(n.t.caps, 'documentSymbolProvider')) {
    toast('info', `${n.t.server} does not list symbols`)
    return
  }
  const name = n.model.uri.path.split('/').pop() ?? ''
  usePopups.getState().set({ structure: { editor: ed, title: name, symbols: [], loading: true } })
  try {
    const r = await n.t.conn.request<(LspDocumentSymbol | LspSymbolInformation)[] | null>('textDocument/documentSymbol', { textDocument: { uri: n.t.uri } })
    const cur = usePopups.getState().structure
    if (cur?.editor === ed) usePopups.getState().set({ structure: { ...cur, symbols: flattenSymbols(r.result), loading: false } })
  } catch (e) {
    const cur = usePopups.getState().structure
    if (cur?.editor === ed) usePopups.getState().set({ structure: { ...cur, loading: false, error: e instanceof Error ? e.message : String(e) } })
  }
}

/**
 * Ctrl+Alt+L: reformat the selection, or the whole file, with the file's language
 * server; a failure (rustfmt missing, a syntax error) is reported, not swallowed.
 * Files without a server use Monaco's own formatter, where it has one.
 */
export async function reformat(ed: editor.ICodeEditor) {
  const model = ed.getModel()
  const sel = ed.getSelection()
  const t = model ? lsp.target(model) : null
  const ranged = !!sel && !sel.isEmpty()
  if (!model || !t || !(has(t.caps, 'documentFormattingProvider') || (ranged && has(t.caps, 'documentRangeFormattingProvider')))) {
    ed.trigger('keyboard', ranged ? 'editor.action.formatSelection' : 'editor.action.formatDocument', null)
    return
  }
  const opts = model.getOptions()
  const options = { tabSize: opts.tabSize, insertSpaces: opts.insertSpaces, trimTrailingWhitespace: true, insertFinalNewline: true }
  const version = model.getAlternativeVersionId()
  try {
    const useRange = ranged && has(t.caps, 'documentRangeFormattingProvider')
    const r = await t.conn.request<LspTextEdit[] | null>(
      useRange ? 'textDocument/rangeFormatting' : 'textDocument/formatting',
      useRange ? { textDocument: { uri: t.uri }, range: toLspRange(sel!), options } : { textDocument: { uri: t.uri }, options },
    )
    if (model.isDisposed() || model.getAlternativeVersionId() !== version) return
    const edits = r.result ?? []
    if (!edits.length) {
      // `null` is also how some servers answer when their formatter failed (rust-analyzer
      // without rustfmt): their log says why.
      if (r.result === null) {
        const server = t.server
        toast('info', `${server} made no formatting changes`, {
          timeout: 4000,
          action: { label: 'Show Log', run: () => usePopups.getState().set({ logs: { projectId: t.pid, serverId: server } }) },
        })
      } else toast('info', 'Already formatted', { timeout: 1500 })
      return
    }
    ed.pushUndoStop()
    ed.executeEdits('lsp.format', edits.map((e) => ({ range: toRange(e.range), text: e.newText })))
    ed.pushUndoStop()
  } catch (e) {
    toast('warning', `${t.server} could not format this file`, { detail: e instanceof Error ? e.message.slice(0, 400) : String(e) })
  }
}
