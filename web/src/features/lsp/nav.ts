// Opening a location: a project file in the `editor` panel (the files contract), a
// library file outside the project in this slice's read-only `lsp.source` panel.

import { openPanel } from '@/shell/actions'
import { basename, parseModelUri, readText } from '@/features/files/modelAccess'
import type { LspRange } from './api'
import { displayPath, projectOfUri } from './convert'

export interface SourceParams {
  projectId: string
  uri: string
  line?: number
  column?: number
  endColumn?: number
  t?: number
}

export const sourcePanelId = (projectId: string, uri: string) => `lsp.source:${projectId}:${displayPath(uri)}`

/** Show `uri` at `range` (start selected up to its end on the same line). */
export function openLocation(uri: string, range?: LspRange, opts: { side?: boolean; focus?: boolean } = {}) {
  const line = range ? range.start.line + 1 : undefined
  const column = range ? range.start.character + 1 : undefined
  const endColumn = range && range.end.line === range.start.line && range.end.character > range.start.character ? range.end.character + 1 : undefined
  const ref = parseModelUri(uri)
  if (ref) {
    const params: Record<string, unknown> = { projectId: ref.projectId, path: ref.path }
    if (line) Object.assign(params, { line, column, endColumn, t: Date.now() })
    openPanel({
      kind: 'editor',
      id: `editor:${ref.projectId ?? ''}:${ref.path}` + (opts.side ? '#side' : ''),
      title: basename(ref.path),
      params,
      position: opts.side ? 'right' : 'auto',
      focus: opts.focus,
    })
    return
  }
  const pid = projectOfUri(uri)
  if (uri.startsWith('lsp-src://') && pid) {
    const params: SourceParams = { projectId: pid, uri, line, column, endColumn, t: Date.now() }
    openPanel({
      kind: 'lsp.source',
      id: sourcePanelId(pid, uri),
      title: basename(displayPath(uri)),
      params: params as unknown as Record<string, unknown>,
      position: opts.side ? 'right' : 'auto',
      focus: opts.focus,
    })
  }
}

/** Source texts fetched for previews (small LRU). */
const sourceCache = new Map<string, Promise<string | null>>()

/** Text of any location's file, for previews: open buffers, project files, library sources. */
export function textOf(uri: string): Promise<string | null> {
  const ref = parseModelUri(uri)
  if (ref) return readText(ref.projectId, ref.path).catch(() => null)
  const pid = projectOfUri(uri)
  if (!pid || !uri.startsWith('lsp-src://')) return Promise.resolve(null)
  let p = sourceCache.get(uri)
  if (!p) {
    p = import('./api').then(({ lspApi }) => lspApi.source(pid, uri).then((r) => r.content)).catch(() => null)
    sourceCache.set(uri, p)
    if (sourceCache.size > 40) sourceCache.delete(sourceCache.keys().next().value!)
  }
  return p
}

/** One line of a text (0-based), without its line break. */
export function lineOf(text: string | null, line: number): string {
  if (text === null) return ''
  let start = 0
  for (let i = 0; i < line; i++) {
    const n = text.indexOf('\n', start)
    if (n < 0) return ''
    start = n + 1
  }
  const end = text.indexOf('\n', start)
  return text.slice(start, end < 0 ? text.length : end).replace(/\r$/, '')
}
