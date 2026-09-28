// Monaco language features backed by the project's language servers, registered once
// for every `file:` and `lsp-src:` model. Each call finds the model's server
// (`lsp.target`) and asks only when that server has the capability; anything else
// (no server, off, starting, an error) answers "nothing", so Monaco's own behaviour
// (word completion, indentation folding) stays.
//
// Rename is not a Monaco provider: Shift+F6 previews every edit first (rename.ts),
// and edits to files no editor has open go through their buffers.

import type { CancellationToken, editor, IDisposable, IRange, languages, Position } from 'monaco-editor'
import type { LspCodeAction, LspCommand, LspDiagnostic, LspLocation, LspLocationLink, LspTextEdit, LspWorkspaceEdit } from './api'
import { has, type LspClient, type Target } from './client'
import { MONACO_TOKEN_TYPES, remapSemanticTokens, TOKEN_MODIFIERS } from './semanticTokens'
import { ensurePreviewModels } from './sourceModels'
import { executeCommand, runCodeAction } from './workspaceEdit'
import {
  completionKind,
  rangesOverlap,
  toDocumentation,
  toDocumentSymbols,
  toLocs,
  toLspPosition,
  toLspRange,
  toMarkdownString,
  toRange,
  toTextEdits,
} from './convert'

type MonacoNs = (typeof import('@/lib/monacoSetup'))['monaco']

const SELECTOR: languages.LanguageSelector = [{ scheme: 'file' }, { scheme: 'lsp-src' }]

/** Ask the model's server; `null` when it cannot or will not answer. */
async function ask<T>(
  client: LspClient,
  model: editor.ITextModel,
  capability: string | null,
  method: string,
  params: (t: Target) => unknown,
  token?: CancellationToken,
): Promise<{ result: T; target: Target } | null> {
  const t = client.target(model)
  if (!t || (capability && !has(t.caps, capability))) return null
  const ctrl = new AbortController()
  const sub = token?.onCancellationRequested(() => ctrl.abort())
  try {
    const r = await t.conn.request<T>(method, params(t), { signal: ctrl.signal })
    return { result: r.result, target: t }
  } catch {
    return null
  } finally {
    sub?.dispose()
  }
}

const doc = (t: Target) => ({ textDocument: { uri: t.uri } })
const at = (t: Target, p: Position) => ({ textDocument: { uri: t.uri }, position: toLspPosition(p) })

/** Characters of every ready server (a provider registration takes one static list). */
function triggerChars(client: LspClient, key: 'completionProvider' | 'signatureHelpProvider', field: 'triggerCharacters' | 'retriggerCharacters'): string[] {
  const out = new Set<string>()
  for (const e of client.models_()) {
    const c = client.connection(e.pid)
    if (!c) continue
    for (const caps of c.servers.values()) {
      const p = caps[key] as Record<string, unknown> | undefined
      for (const ch of (p?.[field] as string[] | undefined) ?? []) if (typeof ch === 'string' && ch.length === 1) out.add(ch)
    }
  }
  return [...out].sort()
}

export function toMonacoLocations(monaco: MonacoNs, r: LspLocation | LspLocation[] | LspLocationLink[] | null): languages.LocationLink[] {
  return toLocs(r).map((l) => ({
    uri: monaco.Uri.parse(l.uri),
    range: toRange(l.targetRange ?? l.range),
    targetSelectionRange: toRange(l.range),
    originSelectionRange: l.originSelectionRange ? toRange(l.originSelectionRange) : undefined,
  }))
}

interface LspCompletionItem {
  label: string
  labelDetails?: { detail?: string; description?: string }
  kind?: number
  tags?: number[]
  detail?: string
  documentation?: string | { kind: string; value: string }
  deprecated?: boolean
  preselect?: boolean
  sortText?: string
  filterText?: string
  insertText?: string
  insertTextFormat?: number
  textEdit?: LspTextEdit | { newText: string; insert: { start: never; end: never }; replace: never }
  textEditText?: string
  additionalTextEdits?: LspTextEdit[]
  commitCharacters?: string[]
  command?: LspCommand
  data?: unknown
}

interface CompletionList {
  isIncomplete?: boolean
  items: LspCompletionItem[]
  itemDefaults?: { commitCharacters?: string[]; editRange?: unknown; insertTextFormat?: number; data?: unknown }
}

/** A Monaco completion item remembers where it came from (for resolve). */
type MonacoItem = languages.CompletionItem & { _lsp?: { item: LspCompletionItem; target: Target } }

function editorCommand(cmd: LspCommand | undefined, t: Target): languages.Command | undefined {
  if (!cmd) return undefined
  // Commands of the editor itself (VS Code's names are Monaco's).
  if (cmd.command.startsWith('editor.action.')) return { id: cmd.command, title: cmd.title, arguments: cmd.arguments }
  return { id: 'lsp.executeCommand', title: cmd.title, arguments: [t.pid, t.server, cmd] }
}

function completionRange(r: unknown): IRange | languages.CompletionItemRanges | undefined {
  if (!r || typeof r !== 'object') return undefined
  if ('insert' in r && 'replace' in r) {
    const x = r as { insert: Parameters<typeof toRange>[0]; replace: Parameters<typeof toRange>[0] }
    return { insert: toRange(x.insert), replace: toRange(x.replace) }
  }
  if ('start' in r) return toRange(r as Parameters<typeof toRange>[0])
  return undefined
}

export function convertCompletion(item: LspCompletionItem, list: CompletionList, defaultRange: languages.CompletionItemRanges, t: Target): MonacoItem {
  const defaults = list.itemDefaults ?? {}
  const edit = item.textEdit as { newText: string; range?: unknown; insert?: unknown; replace?: unknown } | undefined
  const range = completionRange(edit ? (edit.range ?? { insert: edit.insert, replace: edit.replace }) : defaults.editRange) ?? defaultRange
  const format = item.insertTextFormat ?? defaults.insertTextFormat
  const insertText = edit?.newText ?? item.textEditText ?? item.insertText ?? item.label
  const out: MonacoItem = {
    label: item.labelDetails ? { label: item.label, detail: item.labelDetails.detail, description: item.labelDetails.description } : item.label,
    kind: completionKind(item.kind),
    detail: item.detail,
    documentation: toDocumentation(item.documentation),
    sortText: item.sortText,
    filterText: item.filterText,
    preselect: item.preselect,
    insertText,
    insertTextRules: format === 2 ? 4 : 0,
    range,
    commitCharacters: item.commitCharacters ?? defaults.commitCharacters,
    additionalTextEdits: item.additionalTextEdits ? toTextEdits(item.additionalTextEdits) : undefined,
    command: editorCommand(item.command, t),
    tags: item.deprecated || item.tags?.includes(1) ? [1] : undefined,
  }
  if (item.data === undefined && defaults.data !== undefined) item = { ...item, data: defaults.data }
  out._lsp = { item, target: t }
  return out
}

export function registerProviders(monaco: MonacoNs, client: LspClient): void {
  const L = monaco.languages
  const disposables: IDisposable[] = []

  L.registerHoverProvider(SELECTOR, {
    provideHover: async (model, position, token) => {
      const r = await ask<{ contents: Parameters<typeof toMarkdownString>[0]; range?: Parameters<typeof toRange>[0] } | null>(client, model, 'hoverProvider', 'textDocument/hover', (t) => at(t, position), token)
      const md = r?.result && toMarkdownString(r.result.contents)
      if (!md) return null
      return { contents: [md], range: r.result!.range ? toRange(r.result!.range) : undefined }
    },
  })

  // Completion and signature help take their trigger characters at registration:
  // registered again when servers with other characters become ready.
  let completion: IDisposable | null = null
  let signature: IDisposable | null = null
  let lastChars = ''
  const registerTriggered = () => {
    const cc = triggerChars(client, 'completionProvider', 'triggerCharacters')
    const sc = triggerChars(client, 'signatureHelpProvider', 'triggerCharacters')
    const rc = triggerChars(client, 'signatureHelpProvider', 'retriggerCharacters')
    const key = `${cc.join('')}|${sc.join('')}|${rc.join('')}`
    if (key === lastChars && completion) return
    lastChars = key
    completion?.dispose()
    signature?.dispose()
    completion = L.registerCompletionItemProvider(SELECTOR, {
      triggerCharacters: cc,
      provideCompletionItems: async (model, position, context, token) => {
        const t = client.target(model)
        if (!t || !has(t.caps, 'completionProvider')) return undefined
        const own = ((t.caps.completionProvider as { triggerCharacters?: string[] }).triggerCharacters ?? []) as string[]
        // Another server's trigger character: not this one's business.
        if (context.triggerKind === 1 && context.triggerCharacter && !own.includes(context.triggerCharacter)) return { suggestions: [] }
        const r = await ask<CompletionList | LspCompletionItem[] | null>(
          client,
          model,
          'completionProvider',
          'textDocument/completion',
          (tt) => ({ ...at(tt, position), context: { triggerKind: context.triggerKind + 1, triggerCharacter: context.triggerCharacter } }),
          token,
        )
        if (!r?.result) return { suggestions: [] }
        const list: CompletionList = Array.isArray(r.result) ? { items: r.result } : r.result
        const word = model.getWordUntilPosition(position)
        const lineMax = model.getLineMaxColumn(position.lineNumber)
        const wordEnd = model.getWordAtPosition(position)?.endColumn ?? position.column
        const defaultRange: languages.CompletionItemRanges = {
          insert: { startLineNumber: position.lineNumber, startColumn: word.startColumn, endLineNumber: position.lineNumber, endColumn: position.column },
          replace: { startLineNumber: position.lineNumber, startColumn: word.startColumn, endLineNumber: position.lineNumber, endColumn: Math.min(Math.max(wordEnd, position.column), lineMax) },
        }
        return {
          suggestions: (list.items ?? []).slice(0, 5000).map((it) => convertCompletion(it, list, defaultRange, r.target)),
          incomplete: !!list.isIncomplete,
        }
      },
      resolveCompletionItem: async (item: MonacoItem, token) => {
        const src = item._lsp
        if (!src) return item
        const caps = src.target.caps.completionProvider as { resolveProvider?: boolean } | undefined
        if (!caps?.resolveProvider) return item
        const ctrl = new AbortController()
        const sub = token.onCancellationRequested(() => ctrl.abort())
        try {
          const r = await src.target.conn.request<LspCompletionItem>('completionItem/resolve', src.item, { server: src.target.server, signal: ctrl.signal })
          const res = r.result
          if (!res) return item
          if (res.documentation !== undefined) item.documentation = toDocumentation(res.documentation)
          if (res.detail !== undefined) item.detail = res.detail
          if (res.additionalTextEdits && !item.additionalTextEdits) item.additionalTextEdits = toTextEdits(res.additionalTextEdits)
          if (res.command && !item.command) item.command = editorCommand(res.command, src.target)
          return item
        } catch {
          return item
        } finally {
          sub.dispose()
        }
      },
    })
    signature = L.registerSignatureHelpProvider(SELECTOR, {
      signatureHelpTriggerCharacters: sc,
      signatureHelpRetriggerCharacters: rc,
      provideSignatureHelp: async (model, position, token, context) => {
        interface SigInfo {
          label: string
          documentation?: string | { kind: string; value: string }
          parameters?: { label: string | [number, number]; documentation?: string | { kind: string; value: string } }[]
          activeParameter?: number
        }
        const r = await ask<{ signatures: SigInfo[]; activeSignature?: number; activeParameter?: number } | null>(
          client,
          model,
          'signatureHelpProvider',
          'textDocument/signatureHelp',
          (t) => ({
            ...at(t, position),
            context: {
              triggerKind: context.triggerKind,
              triggerCharacter: context.triggerCharacter,
              isRetrigger: context.isRetrigger,
            },
          }),
          token,
        )
        const res = r?.result
        if (!res?.signatures?.length) return null
        return {
          value: {
            signatures: res.signatures.map((s) => ({
              label: s.label,
              documentation: toDocumentation(s.documentation),
              parameters: (s.parameters ?? []).map((p) => ({ label: p.label, documentation: toDocumentation(p.documentation) })),
              activeParameter: s.activeParameter,
            })),
            activeSignature: res.activeSignature ?? 0,
            activeParameter: res.activeParameter ?? 0,
          },
          dispose: () => {},
        }
      },
    })
  }
  registerTriggered()
  client.capsListeners.add(registerTriggered)

  const locationProvider = (capability: string, method: string) => async (model: editor.ITextModel, position: Position, token: CancellationToken) => {
    const r = await ask<LspLocation | LspLocation[] | LspLocationLink[] | null>(client, model, capability, method, (t) => at(t, position), token)
    if (!r?.result) return null
    const links = toMonacoLocations(monaco, r.result)
    // Monaco previews the target (Ctrl+hover, peek) from its model: make sure there is one.
    const others = links.map((l) => l.uri.toString()).filter((u) => u !== model.uri.toString())
    if (others.length && !token.isCancellationRequested) await ensurePreviewModels(monaco, others)
    return links
  }
  L.registerDefinitionProvider(SELECTOR, { provideDefinition: locationProvider('definitionProvider', 'textDocument/definition') })
  L.registerDeclarationProvider(SELECTOR, { provideDeclaration: locationProvider('declarationProvider', 'textDocument/declaration') })
  L.registerTypeDefinitionProvider(SELECTOR, { provideTypeDefinition: locationProvider('typeDefinitionProvider', 'textDocument/typeDefinition') })
  L.registerImplementationProvider(SELECTOR, { provideImplementation: locationProvider('implementationProvider', 'textDocument/implementation') })
  L.registerReferenceProvider(SELECTOR, {
    provideReferences: async (model, position, context, token) => {
      const r = await ask<LspLocation[] | null>(
        client,
        model,
        'referencesProvider',
        'textDocument/references',
        (t) => ({ ...at(t, position), context: { includeDeclaration: context.includeDeclaration } }),
        token,
      )
      const refs = (r?.result ?? []).map((l) => ({ uri: monaco.Uri.parse(l.uri), range: toRange(l.range) }))
      // Monaco's peek shows each file from its model (at most 10 files are loaded).
      const others = refs.map((l) => l.uri.toString()).filter((u) => u !== model.uri.toString())
      if (others.length && !token.isCancellationRequested) await ensurePreviewModels(monaco, others)
      return refs
    },
  })

  L.registerDocumentHighlightProvider(SELECTOR, {
    provideDocumentHighlights: async (model, position, token) => {
      const r = await ask<{ range: Parameters<typeof toRange>[0]; kind?: number }[] | null>(client, model, 'documentHighlightProvider', 'textDocument/documentHighlight', (t) => at(t, position), token)
      return (r?.result ?? []).map((h) => ({ range: toRange(h.range), kind: h.kind ? h.kind - 1 : 0 }))
    },
  })

  L.registerDocumentSymbolProvider(SELECTOR, {
    displayName: 'Language server',
    provideDocumentSymbols: async (model, token) => {
      const r = await ask<Parameters<typeof toDocumentSymbols>[0]>(client, model, 'documentSymbolProvider', 'textDocument/documentSymbol', doc, token)
      return r?.result ? toDocumentSymbols(r.result) : null
    },
  })

  const formattingOptions = (o: languages.FormattingOptions) => ({ tabSize: o.tabSize, insertSpaces: o.insertSpaces, trimTrailingWhitespace: true, insertFinalNewline: true })
  L.registerDocumentFormattingEditProvider(SELECTOR, {
    displayName: 'Language server',
    provideDocumentFormattingEdits: async (model, options, token) => {
      const r = await ask<LspTextEdit[] | null>(client, model, 'documentFormattingProvider', 'textDocument/formatting', (t) => ({ ...doc(t), options: formattingOptions(options) }), token)
      return r?.result ? toTextEdits(r.result) : null
    },
  })
  L.registerDocumentRangeFormattingEditProvider(SELECTOR, {
    displayName: 'Language server',
    provideDocumentRangeFormattingEdits: async (model, range, options, token) => {
      const r = await ask<LspTextEdit[] | null>(
        client,
        model,
        'documentRangeFormattingProvider',
        'textDocument/rangeFormatting',
        (t) => ({ ...doc(t), range: toLspRange(range), options: formattingOptions(options) }),
        token,
      )
      return r?.result ? toTextEdits(r.result) : null
    },
  })

  L.registerCodeActionProvider(
    SELECTOR,
    {
      provideCodeActions: async (model, range, context, token) => {
        const t = client.target(model)
        if (!t || !has(t.caps, 'codeActionProvider')) return { actions: [], dispose: () => {} }
        const diagnostics: LspDiagnostic[] = client
          .diagnosticsOf(t.uri)
          .filter((d) => d.server === t.server && rangesOverlap(toRange(d.range), range))
          .map(({ server: _s, ...d }) => d)
        const r = await ask<(LspCodeAction | LspCommand)[] | null>(
          client,
          model,
          'codeActionProvider',
          'textDocument/codeAction',
          (tt) => ({
            ...doc(tt),
            range: toLspRange(range),
            context: { diagnostics, only: context.only ? [context.only] : undefined, triggerKind: context.trigger === 2 ? 2 : 1 },
          }),
          token,
        )
        const actions: languages.CodeAction[] = (r?.result ?? []).map((a) => {
          if ('command' in a && typeof a.command === 'string') {
            const c = a as LspCommand
            return { title: c.title, command: { id: 'lsp.executeCommand', title: c.title, arguments: [t.pid, t.server, c] } }
          }
          const ca = a as LspCodeAction
          const markers = context.markers.filter((m) => ca.diagnostics?.some((d) => d.message === m.message && toRange(d.range).startLineNumber === m.startLineNumber))
          return {
            title: ca.title,
            kind: ca.kind,
            isPreferred: ca.isPreferred,
            disabled: ca.disabled?.reason,
            diagnostics: markers,
            command: { id: 'lsp.applyCodeAction', title: ca.title, arguments: [t.pid, t.server, ca] },
          }
        })
        return { actions, dispose: () => {} }
      },
    },
    { providedCodeActionKinds: ['quickfix', 'refactor', 'refactor.extract', 'refactor.inline', 'refactor.rewrite', 'source', 'source.organizeImports', 'source.fixAll'] },
  )

  // Semantic highlighting: the server's tokens in Workbench's legend (semanticTokens.ts),
  // coloured by the theme rules in lib/monacoSetup.ts; Monarch colours the rest.
  const tokensChanged = new monaco.Emitter<void>()
  client.refreshListeners.add((_server, what) => {
    if (what === 'semanticTokens') tokensChanged.fire()
  })
  client.capsListeners.add(() => tokensChanged.fire())
  L.registerDocumentSemanticTokensProvider(SELECTOR, {
    onDidChange: tokensChanged.event,
    getLegend: () => ({ tokenTypes: MONACO_TOKEN_TYPES, tokenModifiers: TOKEN_MODIFIERS }),
    provideDocumentSemanticTokens: async (model, _lastResultId, token) => {
      const r = await ask<{ data: number[] } | null>(client, model, 'semanticTokensProvider', 'textDocument/semanticTokens/full', doc, token)
      const legend = (r?.target.caps.semanticTokensProvider as { legend?: { tokenTypes: string[]; tokenModifiers: string[] } } | undefined)?.legend
      if (!r?.result?.data || !legend) return null
      return { data: remapSemanticTokens(r.result.data, legend.tokenTypes, legend.tokenModifiers) }
    },
    releaseDocumentSemanticTokens: () => {},
  })

  const inlayChanged = new monaco.Emitter<void>()
  client.refreshListeners.add((_server, what) => {
    if (what === 'inlayHint') inlayChanged.fire()
  })
  client.capsListeners.add(() => inlayChanged.fire())
  L.registerInlayHintsProvider(SELECTOR, {
    onDidChangeInlayHints: inlayChanged.event,
    provideInlayHints: async (model, range, token) => {
      interface Hint {
        position: { line: number; character: number }
        label: string | { value: string; tooltip?: string | { kind: string; value: string } }[]
        kind?: number
        tooltip?: string | { kind: string; value: string }
        paddingLeft?: boolean
        paddingRight?: boolean
        textEdits?: LspTextEdit[]
      }
      const r = await ask<Hint[] | null>(client, model, 'inlayHintProvider', 'textDocument/inlayHint', (t) => ({ ...doc(t), range: toLspRange(range) }), token)
      if (!r?.result) return { hints: [], dispose: () => {} }
      return {
        hints: r.result.slice(0, 5000).map((h) => ({
          label: typeof h.label === 'string' ? h.label : h.label.map((p) => ({ label: p.value, tooltip: toDocumentation(p.tooltip) })),
          position: { lineNumber: h.position.line + 1, column: h.position.character + 1 },
          kind: h.kind,
          tooltip: toDocumentation(h.tooltip),
          paddingLeft: h.paddingLeft,
          paddingRight: h.paddingRight,
          textEdits: h.textEdits ? toTextEdits(h.textEdits) : undefined,
        })),
        dispose: () => {},
      }
    },
  })

  L.registerFoldingRangeProvider(SELECTOR, {
    provideFoldingRanges: async (model, _context, token) => {
      const r = await ask<{ startLine: number; endLine: number; kind?: string }[] | null>(client, model, 'foldingRangeProvider', 'textDocument/foldingRange', doc, token)
      if (!r?.result?.length) return null
      return r.result.slice(0, 5000).map((f) => ({
        start: f.startLine + 1,
        end: f.endLine + 1,
        kind: f.kind ? new monaco.languages.FoldingRangeKind(f.kind) : undefined,
      }))
    },
  })

  L.registerSelectionRangeProvider(SELECTOR, {
    provideSelectionRanges: async (model, positions, token) => {
      interface Sel {
        range: Parameters<typeof toRange>[0]
        parent?: Sel
      }
      const r = await ask<Sel[] | null>(client, model, 'selectionRangeProvider', 'textDocument/selectionRange', (t) => ({ ...doc(t), positions: positions.map(toLspPosition) }), token)
      if (!r?.result) return null
      return r.result.map((s) => {
        const out: languages.SelectionRange[] = []
        for (let cur: Sel | undefined = s; cur && out.length < 50; cur = cur.parent) out.push({ range: toRange(cur.range) })
        return out
      })
    },
  })

  // Code actions and server commands run through Workbench (edits to files no
  // editor shows go to their buffers, never straight to disk).
  disposables.push(
    monaco.editor.registerCommand('lsp.applyCodeAction', (_accessor, pid: string, server: string, action: LspCodeAction) => {
      void runCodeAction(pid, server, action)
    }),
    monaco.editor.registerCommand('lsp.executeCommand', (_accessor, pid: string, server: string, command: LspCommand) => {
      void executeCommand(pid, server, command)
    }),
  )
}

export type { LspWorkspaceEdit }
