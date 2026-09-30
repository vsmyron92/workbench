// Opening things from anywhere in the files slice (tree, search, quick open,
// Markdown links, MCP `ui.open`), plus the cross-slice actions of the context menus.

import { api } from '@/api/client'
import type { TerminalInfo } from '@/api/types'
import { askAgent } from '@/shell/agentBridge'
import { getDockApi, openPanel, showToolWindow, toast, toastError } from '@/shell/actions'
import { useUi } from '@/state/store'
import { basename, dirname, editorPanelId, isAbsolutePath, isMarkdown, joinAbsolute, markdownPanelId, searchPanelId, tabTitle } from './paths'
import { isScratch, useFilesView } from './scratchStore'
import { noteRecent, useQuickOpen, useSearchStore, useTreeStore } from './store'

/** How a Markdown file shows in the editor: rendered page, side by side, or source. */
export type MarkdownMode = 'read' | 'split' | 'edit'

export interface EditorParams {
  projectId: string | null
  path: string
  /** Markdown only; unset means the preference (Settings › General). */
  mode?: MarkdownMode
  line?: number
  column?: number
  /** Select up to this column on `line` (search results). */
  endColumn?: number
  /** Changes on every navigation so the editor reveals again (same line twice). */
  t?: number
}

export function openFile(o: {
  projectId: string | null
  path: string
  line?: number
  column?: number
  endColumn?: number
  side?: boolean
  focus?: boolean
  mode?: MarkdownMode
}) {
  const params: EditorParams = { projectId: o.projectId, path: o.path }
  if (o.mode) params.mode = o.mode
  if (o.line) Object.assign(params, { line: o.line, column: o.column ?? 1, endColumn: o.endColumn, t: Date.now() })
  noteRecent(o.projectId, o.path)
  const id = editorPanelId(o.projectId, o.path) + (o.side ? '#side' : '')
  openPanel({ kind: 'editor', id, title: tabTitle(o.path), params: params as unknown as Record<string, unknown>, position: o.side ? 'right' : 'auto', focus: o.focus })
}

export function openMarkdown(projectId: string | null, path: string, side = false) {
  noteRecent(projectId, path)
  openPanel({
    kind: 'markdown',
    id: markdownPanelId(projectId, path) + (side ? '#side' : ''),
    title: `${basename(path)} (preview)`,
    params: { projectId, path },
    position: side ? 'right' : 'auto',
  })
}

/** Compare a project file (right) with another one of the project (left). */
export function compareFiles(projectId: string, path: string, other: string) {
  openPanel({ kind: 'compare', id: `compare:${projectId}:${path}:${other}`, title: `${basename(other)} ↔ ${basename(path)}`, params: { projectId, path, left: { path: other } } })
}

/** Compare With…: pick the other file with Go to File. */
export function compareWithPicked(projectId: string, path: string) {
  useQuickOpen.getState().choose(`Compare ${basename(path)} with…`, (other) => {
    if (other === path) toast('info', 'Pick another file')
    else compareFiles(projectId, path, other)
  })
}

/** Compare with Clipboard: the clipboard (left, read-only) against the file. */
export async function compareWithClipboard(projectId: string | null, path: string) {
  if (!projectId) return toast('info', 'Compare works on project files')
  let text: string
  try {
    text = await navigator.clipboard.readText()
  } catch {
    return toast('warning', 'The browser did not allow reading the clipboard')
  }
  if (!text) return toast('info', 'The clipboard is empty')
  const { rememberCompareText } = await import('./ComparePanel')
  const key = rememberCompareText(text)
  openPanel({ kind: 'compare', id: `compare:${projectId}:${path}:${key}`, title: `Clipboard ↔ ${basename(path)}`, params: { projectId, path, left: { textKey: key, label: 'Clipboard' } } })
}

/** Open a path the way its type deserves (Markdown as a rendered page, else the editor). */
export function openAny(projectId: string | null, path: string, line?: number) {
  if (isMarkdown(path) && !line) openFile({ projectId, path, mode: 'read' })
  else openFile({ projectId, path, line })
}

export function openSearchPanel(projectId: string, query?: string) {
  if (query !== undefined) useSearchStore.getState().set(projectId, { q: query })
  openPanel({ kind: 'search', id: searchPanelId(projectId), title: 'Find in Files', params: { projectId, query } })
}

/** Find in Files: show the Search tool window, prefilled with `query` if given. */
export function showSearch(projectId: string | null, query?: string, glob?: string) {
  if (!projectId) return
  const patch: Record<string, string> = {}
  if (query) patch.q = query
  if (glob !== undefined) patch.glob = glob
  if (Object.keys(patch).length) useSearchStore.getState().set(projectId, patch)
  showToolWindow('search', 'left')
  useSearchStore.getState().focus()
}

/** Expand the tree to `path`, select it and show the Files tool window. */
export function revealInTree(projectId: string | null, path: string) {
  if (!projectId) {
    toast('info', 'This file is outside the project tree')
    return
  }
  // Scratches show in the Files window's Scratches view, whatever the project.
  if (isScratch(projectId)) useFilesView.getState().setScratches(true)
  else {
    useFilesView.getState().setScratches(false)
    if (useUi.getState().projectId !== projectId) useUi.getState().setProject(projectId)
  }
  showToolWindow('files', 'left')
  useTreeStore.getState().update(projectId, (t) => {
    const expanded = new Set(t.expanded)
    let d = dirname(path)
    while (d && d !== '/') {
      expanded.add(d)
      d = dirname(d)
    }
    return { expanded: [...expanded], selected: path, filter: '' }
  })
  revealRequests.forEach((fn) => fn(projectId, path))
}

/** The tree listens to scroll the revealed row into view once it is loaded. */
export const revealRequests = new Set<(projectId: string, path: string) => void>()

/** Absolute filesystem path of a project file (for drag and drop, Copy Path), as the server's OS writes it. */
export function absolutePath(rootAbs: string | undefined, path: string): string {
  if (isAbsolutePath(path) || !rootAbs) return path
  return joinAbsolute(rootAbs, path)
}

export async function openTerminalAt(projectId: string, cwd: string) {
  try {
    const t = await api.post<TerminalInfo>('/api/terminals', { kind: 'shell', projectId, cwd })
    openPanel({ kind: 'terminal', id: `terminal:${t.id}`, title: t.title, params: { terminalId: t.id } })
  } catch (e) {
    toastError(e, 'Could not open a terminal')
  }
}

export function showHistory(projectId: string, path: string) {
  openPanel({ kind: 'gitlog', id: `gitlog:${projectId}`, title: 'Git Log', params: { projectId, path } })
}

export function compareWithHead(projectId: string, path: string) {
  openPanel({
    kind: 'diff',
    id: `diff:${projectId}:working::${path}`,
    title: `${basename(path)} (diff)`,
    params: { projectId, path, mode: 'working' },
  })
}

export function showCommit(projectId: string, sha: string) {
  openPanel({ kind: 'commit', id: `commit:${projectId}:${sha}`, title: sha.slice(0, 8), params: { projectId, sha } })
}

export function askAboutFile(projectId: string | null, path: string) {
  void askAgent({ projectId, prompt: `Regarding @${path}: `, submit: false })
}

export function askAboutSelection(projectId: string | null, path: string, start: number, end: number, snippet: string, lang: string) {
  const range = start === end ? `${start}` : `${start}-${end}`
  const code = snippet.length > 8000 ? snippet.slice(0, 8000) + '\n…' : snippet
  void askAgent({ projectId, prompt: `In @${path} (lines ${range}):\n\`\`\`${lang}\n${code}\n\`\`\`\n`, submit: false })
}

/** Panels (any kind) showing `path` of `projectId`. */
export function panelsFor(kind: string, projectId: string | null, path: string) {
  const dock = getDockApi()
  if (!dock) return []
  return dock.panels.filter((p) => {
    if (p.view.contentComponent !== kind) return false
    const params = (p.params ?? {}) as { projectId?: string | null; path?: string }
    return (params.projectId ?? null) === projectId && params.path === path
  })
}
