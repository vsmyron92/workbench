// Pure conversions between the Language Server Protocol and Monaco shapes. Monaco's
// enums are plain numbers here (their values are part of Monaco's API), so this
// module needs no Monaco at runtime and is covered by vitest.
//
// Positions: LSP lines and characters are 0-based UTF-16 offsets (the encoding
// Workbench announces); Monaco's are 1-based UTF-16 columns.

import type { IMarkdownString, IRange, languages } from 'monaco-editor'
import type { LspDiagnostic, LspDocumentSymbol, LspLocation, LspLocationLink, LspRange, LspSymbolInformation, LspTextEdit } from './api'

export function toLspPosition(p: { lineNumber: number; column: number }) {
  return { line: p.lineNumber - 1, character: p.column - 1 }
}

export function toLspRange(r: IRange): LspRange {
  return { start: { line: r.startLineNumber - 1, character: r.startColumn - 1 }, end: { line: r.endLineNumber - 1, character: r.endColumn - 1 } }
}

export function toRange(r: LspRange): IRange {
  return { startLineNumber: r.start.line + 1, startColumn: r.start.character + 1, endLineNumber: r.end.line + 1, endColumn: r.end.character + 1 }
}

export function rangesOverlap(a: IRange, b: IRange): boolean {
  const before = (l1: number, c1: number, l2: number, c2: number) => l1 < l2 || (l1 === l2 && c1 <= c2)
  return before(a.startLineNumber, a.startColumn, b.endLineNumber, b.endColumn) && before(b.startLineNumber, b.startColumn, a.endLineNumber, a.endColumn)
}

// ---------------------------------------------------------------- markdown

type MarkedString = string | { language: string; value: string }
export type LspMarkup = MarkedString | MarkedString[] | { kind: 'markdown' | 'plaintext'; value: string } | null | undefined

/** Escape text so Markdown shows it literally. */
export function escapeMarkdown(s: string): string {
  return s.replace(/[\\`*_{}[\]()#+\-.!<>|~]/g, '\\$&')
}

function fence(language: string, value: string): string {
  const longest = Math.max(2, ...Array.from(value.matchAll(/`+/g), (m) => m[0].length))
  const f = '`'.repeat(longest + 1)
  return `${f}${language}\n${value}\n${f}`
}

/** LSP hover/documentation content → Markdown text ('' for nothing). */
export function markupToMarkdown(m: LspMarkup): string {
  if (m === null || m === undefined) return ''
  if (typeof m === 'string') return m
  if (Array.isArray(m)) return m.map(markupToMarkdown).filter(Boolean).join('\n\n---\n\n')
  if ('kind' in m) return m.kind === 'markdown' ? m.value : m.value ? fence('text', m.value) : ''
  return m.value ? fence(m.language, m.value) : ''
}

export function toMarkdownString(m: LspMarkup): IMarkdownString | undefined {
  const value = markupToMarkdown(m)
  return value.trim() ? { value, supportHtml: false } : undefined
}

/** Completion/signature documentation: plain strings stay plain. */
export function toDocumentation(d: string | { kind: string; value: string } | undefined): string | IMarkdownString | undefined {
  if (d === undefined || d === null) return undefined
  if (typeof d === 'string') return d || undefined
  if (d.kind === 'markdown') return d.value ? { value: d.value } : undefined
  return d.value || undefined
}

// ---------------------------------------------------------------- kinds

/** LSP CompletionItemKind (1-based) → Monaco CompletionItemKind. */
const COMPLETION_KIND: Record<number, number> = {
  1: 18, // Text
  2: 0, // Method
  3: 1, // Function
  4: 2, // Constructor
  5: 3, // Field
  6: 4, // Variable
  7: 5, // Class
  8: 7, // Interface
  9: 8, // Module
  10: 9, // Property
  11: 12, // Unit
  12: 13, // Value
  13: 15, // Enum
  14: 17, // Keyword
  15: 28, // Snippet
  16: 19, // Color
  17: 20, // File
  18: 21, // Reference
  19: 23, // Folder
  20: 16, // EnumMember
  21: 14, // Constant
  22: 6, // Struct
  23: 10, // Event
  24: 11, // Operator
  25: 24, // TypeParameter
}

export function completionKind(k: number | undefined): number {
  return k !== undefined && k in COMPLETION_KIND ? COMPLETION_KIND[k] : 18
}

/** LSP SymbolKind is Monaco's plus one. */
export function symbolKind(k: number): number {
  return Math.max(0, Math.min(25, k - 1))
}

export const SYMBOL_KIND_NAMES = [
  'File', 'Module', 'Namespace', 'Package', 'Class', 'Method', 'Property', 'Field', 'Constructor', 'Enum', 'Interface', 'Function', 'Variable',
  'Constant', 'String', 'Number', 'Boolean', 'Array', 'Object', 'Key', 'Null', 'EnumMember', 'Struct', 'Event', 'Operator', 'TypeParameter',
]

/** Name of an LSP SymbolKind. */
export function symbolKindName(k: number): string {
  return SYMBOL_KIND_NAMES[k - 1] ?? 'Symbol'
}

/** Monaco MarkerSeverity: Hint 1, Info 2, Warning 4, Error 8. */
export function markerSeverity(s: number | undefined): number {
  switch (s) {
    case 2:
      return 4
    case 3:
      return 2
    case 4:
      return 1
    default:
      return 8
  }
}

// ---------------------------------------------------------------- diagnostics

export interface MarkerData {
  severity: number
  message: string
  source?: string
  code?: string | { value: string; target: string }
  startLineNumber: number
  startColumn: number
  endLineNumber: number
  endColumn: number
  tags?: number[]
  relatedInformation?: { resource: string; message: string; startLineNumber: number; startColumn: number; endLineNumber: number; endColumn: number }[]
}

/** A diagnostic as a Monaco marker (`resource` URIs of related information stay strings). */
export function toMarker(d: LspDiagnostic): MarkerData {
  const r: { startLineNumber: number; startColumn: number; endLineNumber: number; endColumn: number } = toRange(d.range)
  // An empty range is invisible: widen it to one character.
  if (r.startLineNumber === r.endLineNumber && r.startColumn === r.endColumn) r.endColumn += 1
  const code = d.code === undefined || d.code === null ? undefined : String(d.code)
  return {
    severity: markerSeverity(d.severity),
    message: d.message,
    source: d.source ?? d.server,
    code: code && d.codeDescription?.href ? { value: code, target: d.codeDescription.href } : code,
    ...r,
    tags: d.tags?.filter((t) => t === 1 || t === 2),
    relatedInformation: d.relatedInformation?.slice(0, 20).map((ri) => ({ resource: ri.location.uri, message: ri.message, ...toRange(ri.location.range) })),
  }
}

// ---------------------------------------------------------------- locations

export interface Loc {
  uri: string
  range: LspRange
  /** For LocationLinks: the whole target (the range above is its name). */
  targetRange?: LspRange
  originSelectionRange?: LspRange
}

/** Location | Location[] | LocationLink[] → a flat list (names first). */
export function toLocs(r: LspLocation | LspLocation[] | LspLocationLink[] | null | undefined): Loc[] {
  if (!r) return []
  const list = Array.isArray(r) ? r : [r]
  return list.flatMap((l): Loc[] => {
    if ('targetUri' in l) return [{ uri: l.targetUri, range: l.targetSelectionRange ?? l.targetRange, targetRange: l.targetRange, originSelectionRange: l.originSelectionRange }]
    if ('uri' in l && l.range) return [{ uri: l.uri, range: l.range }]
    return []
  })
}

/** Locations grouped by file, in order of first appearance, deduplicated, each file sorted. */
export function groupByFile(locs: Loc[]): { uri: string; locs: Loc[] }[] {
  const map = new Map<string, Loc[]>()
  const seen = new Set<string>()
  for (const l of locs) {
    const k = `${l.uri}#${l.range.start.line}:${l.range.start.character}:${l.range.end.line}:${l.range.end.character}`
    if (seen.has(k)) continue
    seen.add(k)
    if (!map.has(l.uri)) map.set(l.uri, [])
    map.get(l.uri)!.push(l)
  }
  return [...map.entries()].map(([uri, ls]) => ({
    uri,
    locs: ls.sort((a, b) => a.range.start.line - b.range.start.line || a.range.start.character - b.range.start.character),
  }))
}

// ---------------------------------------------------------------- symbols

export interface FlatSymbol {
  name: string
  detail?: string
  kind: number
  depth: number
  range: LspRange
  selectionRange: LspRange
  container?: string
}

/** DocumentSymbol[] (a tree) or SymbolInformation[] (flat) → a flat list with depths. */
export function flattenSymbols(r: (LspDocumentSymbol | LspSymbolInformation)[] | null | undefined): FlatSymbol[] {
  const out: FlatSymbol[] = []
  // Source order at every level (some servers, typescript-language-server among them,
  // list symbols alphabetically; CLion's structure views follow the file).
  const byPosition = (a: LspDocumentSymbol, b: LspDocumentSymbol) => a.range.start.line - b.range.start.line || a.range.start.character - b.range.start.character
  const walk = (list: LspDocumentSymbol[], depth: number, container?: string) => {
    for (const s of [...list].sort(byPosition)) {
      out.push({ name: s.name, detail: s.detail, kind: s.kind, depth, range: s.range, selectionRange: s.selectionRange, container })
      if (s.children?.length) walk(s.children, depth + 1, s.name)
    }
  }
  const docs = (r ?? []).filter((s): s is LspDocumentSymbol => 'selectionRange' in s)
  if (docs.length) walk(docs, 0)
  for (const s of r ?? []) {
    if ('selectionRange' in s) continue
    else if ('range' in s.location) out.push({ name: s.name, kind: s.kind, depth: s.containerName ? 1 : 0, range: s.location.range, selectionRange: s.location.range, container: s.containerName })
  }
  if (r?.length && !('selectionRange' in r[0])) out.sort((a, b) => a.range.start.line - b.range.start.line)
  return out
}

/** Monaco DocumentSymbol tree (outline, breadcrumbs, sticky scroll). */
export function toDocumentSymbols(r: (LspDocumentSymbol | LspSymbolInformation)[] | null | undefined): languages.DocumentSymbol[] {
  const conv = (s: LspDocumentSymbol): languages.DocumentSymbol => ({
    name: s.name || ' ',
    detail: s.detail ?? '',
    kind: symbolKind(s.kind),
    tags: (s.tags ?? []).filter((t) => t === 1) as languages.SymbolTag[],
    range: toRange(s.range),
    selectionRange: toRange(s.selectionRange),
    children: (s.children ?? []).map(conv),
  })
  return (r ?? []).map((s) =>
    'selectionRange' in s
      ? conv(s)
      : {
          name: s.name || ' ',
          detail: '',
          kind: symbolKind(s.kind),
          tags: [],
          containerName: s.containerName,
          range: 'range' in s.location ? toRange(s.location.range) : toRange({ start: { line: 0, character: 0 }, end: { line: 0, character: 0 } }),
          selectionRange: 'range' in s.location ? toRange(s.location.range) : toRange({ start: { line: 0, character: 0 }, end: { line: 0, character: 0 } }),
        },
  )
}

// ---------------------------------------------------------------- edits

export function toTextEdits(edits: LspTextEdit[] | null | undefined): languages.TextEdit[] {
  return (edits ?? []).map((e) => ({ range: toRange(e.range), text: e.newText }))
}

/** Sort edits last-to-first so applying them one by one keeps earlier offsets valid. */
export function sortEditsDescending<T extends { range: LspRange }>(edits: T[]): T[] {
  return [...edits].sort((a, b) => b.range.start.line - a.range.start.line || b.range.start.character - a.range.start.character)
}

/** Apply LSP text edits to a string (for previews). Offsets are UTF-16, like JS strings. */
export function applyEditsToText(text: string, edits: LspTextEdit[]): string {
  const lineStarts = [0]
  for (let i = 0; i < text.length; i++) if (text.charCodeAt(i) === 10) lineStarts.push(i + 1)
  const offset = (p: { line: number; character: number }) => {
    if (p.line >= lineStarts.length) return text.length
    const start = lineStarts[p.line]
    const end = p.line + 1 < lineStarts.length ? lineStarts[p.line + 1] - 1 : text.length
    return Math.min(start + p.character, end)
  }
  let out = text
  for (const e of sortEditsDescending(edits)) {
    out = out.slice(0, offset(e.range.start)) + e.newText + out.slice(offset(e.range.end))
  }
  return out
}

// ---------------------------------------------------------------- misc

/** A word at a 0-based character of a line (identifier characters, any script). */
export function wordAt(line: string, character: number): { start: number; end: number; word: string } | null {
  const re = /[\p{L}\p{N}_$]+/gu
  for (const m of line.matchAll(re)) {
    const start = m.index ?? 0
    const end = start + m[0].length
    if (start <= character && character <= end) return { start, end, word: m[0] }
  }
  return null
}

/** Display path of a browser URI: project-relative for project files, absolute otherwise. */
export function displayPath(uri: string): string {
  if (uri.startsWith('file:///')) {
    const rest = safeDecode(uri.slice('file:///'.length).split(/[?#]/)[0])
    const i = rest.indexOf('/')
    return i < 0 ? rest : rest.slice(i + 1)
  }
  if (uri.startsWith('lsp-src://')) {
    const rest = uri.slice('lsp-src://'.length)
    const i = rest.indexOf('/')
    return i < 0 ? '' : safeDecode(rest.slice(i).split(/[?#]/)[0])
  }
  return uri
}

function safeDecode(s: string): string {
  try {
    return decodeURIComponent(s)
  } catch {
    return s
  }
}

/** The project of a browser URI (`file:///<pid>/…` or `lsp-src://<pid>/…`). */
export function projectOfUri(uri: string): string | null {
  const m = /^(?:file:\/\/\/|lsp-src:\/\/)([^/~][^/]*)\//.exec(uri)
  return m ? safeDecode(m[1]) : null
}

/** Shorter display of absolute library paths (`~/.cargo/registry/src/…/serde-1.0/src/lib.rs`). */
export function shortenPath(p: string, home?: string): string {
  let s = home && p.startsWith(home + '/') ? '~' + p.slice(home.length) : p
  s = s.replace(/\/\.cargo\/registry\/src\/[^/]+\//, '/.cargo/registry/…/')
  s = s.replace(/\/\.rustup\/toolchains\/[^/]+\/lib\/rustlib\/src\/rust\//, '/.rustup/…/rust/')
  return s
}
