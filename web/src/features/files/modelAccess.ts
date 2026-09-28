// Models of project files for other features (lsp, debug): the files contract of
// docs/ARCHITECTURE.md "Editor models". A project file's Monaco model has the URI
// `file:///<projectId>/<project-relative path>` (`~abs` instead of the project id for
// absolute paths) and belongs to the files slice's buffers (`buffers.ts`): one model
// per file, shared by every editor that shows it, with its dirty state and saves.
//
// * `modelUriString` / `parseModelUri`: the URI of a file and back (pure); `modelFile`:
//   the file an editor's model shows (debug and git hook every editor with it).
// * `peekModel`: the open model of a file, if any editor has it (no loading).
// * `readText`: a file's text, unsaved edits included when it is open, else the disk's.
// * `ensureModel`: the file's model, creating the buffer (reading the file) when no
//   editor has it; pair with `release()`. Edits made to it show in every editor of the
//   file and make it dirty; nothing is written to disk until it is saved.
// * `saveModel`, `isDirty`: through the buffer (etag-checked save).

import type { editor, Uri } from 'monaco-editor'
import { filesApi } from './api'
import { acquireBuffer, getModel, releaseBuffer, saveBuffer, useBuffers } from './buffers'
import { bufferKey } from './paths'

export interface FileRef {
  /** `null` for a file outside every project (absolute `path`). */
  projectId: string | null
  /** Project-relative, or absolute when `projectId` is null. */
  path: string
}

/** Characters Monaco keeps as they are in a URI path. */
function keep(c: number): boolean {
  return (c >= 0x61 && c <= 0x7a) || (c >= 0x41 && c <= 0x5a) || (c >= 0x30 && c <= 0x39) || c === 0x2d || c === 0x2e || c === 0x5f || c === 0x7e || c === 0x2f
}

/** Percent-encode a path the way Monaco's `Uri.toString()` does (uppercase hex, UTF-8). */
export function encodeUriPath(p: string): string {
  let out = ''
  for (const byte of new TextEncoder().encode(p)) {
    out += keep(byte) ? String.fromCharCode(byte) : '%' + byte.toString(16).toUpperCase().padStart(2, '0')
  }
  return out
}

/** The model URI of a file, as a string (`Uri.toString()` of `modelUri`). */
export function modelUriString(projectId: string | null, path: string): string {
  const rel = path.startsWith('/') ? path : '/' + path
  return 'file://' + encodeUriPath(`/${projectId ?? '~abs'}${rel}`)
}

/** The file a `file:` model URI names; null for other URIs. */
export function parseModelUri(uri: string | Uri): FileRef | null {
  let path: string
  if (typeof uri === 'string') {
    if (!uri.startsWith('file:///')) return null
    const raw = uri.slice('file://'.length).split(/[?#]/)[0]
    try {
      path = decodeURIComponent(raw)
    } catch {
      return null
    }
  } else {
    if (uri.scheme !== 'file') return null
    path = uri.path
  }
  const m = /^\/([^/]+)(\/.*)?$/.exec(path)
  if (!m) return null
  const rest = m[2] ?? '/'
  if (m[1] === '~abs') return { projectId: null, path: rest }
  return { projectId: m[1], path: rest.slice(1) }
}

/**
 * The file a model shows, for features that hook every editor (debug breakpoints, git
 * "Show History for Selection"): `parseModelUri`, minus a project's root itself.
 * `projectOnly` also leaves out absolute files (`~abs`).
 */
export function modelFile(uri: string | Uri, projectOnly: true): { projectId: string; path: string } | null
export function modelFile(uri: string | Uri, projectOnly?: false): FileRef | null
export function modelFile(uri: string | Uri, projectOnly = false): FileRef | null {
  const f = parseModelUri(uri)
  if (!f || !f.path || f.path === '/') return null
  return projectOnly && f.projectId === null ? null : f
}

/** Monaco's URI of a file's model. */
export async function modelUriFor(projectId: string | null, path: string): Promise<Uri> {
  const { monaco } = await import('@/lib/monacoSetup')
  const { modelUri } = await import('./buffers')
  return modelUri(monaco, projectId, path)
}

/** The open model of a file (some editor, or an `ensureModel` holder, has it), else null. */
export function peekModel(projectId: string | null, path: string): editor.ITextModel | null {
  const m = getModel(bufferKey(projectId, path))
  return m && !m.isDisposed() ? m : null
}

/** A file's text: the open model's (with unsaved edits), else the disk's. Null for binary, too large or sensitive files. */
export async function readText(projectId: string | null, path: string): Promise<string | null> {
  const m = peekModel(projectId, path)
  if (m) return m.getValue()
  const f = await filesApi.read(projectId, path)
  return f.content
}

/**
 * The file's model, opened as a buffer when no editor has it. Call `release()` when
 * done (the buffer stays while an editor shows it). Fails for files that cannot be
 * edited as text (binary, too large, sensitive: those open only in an editor, after
 * the user confirms).
 */
export async function ensureModel(projectId: string | null, path: string): Promise<{ model: editor.ITextModel; release: () => void }> {
  const key = bufferKey(projectId, path)
  let released = false
  const release = () => {
    if (!released) {
      released = true
      releaseBuffer(key)
    }
  }
  const existing = getModel(key)
  if (existing && !existing.isDisposed()) {
    // Synchronous up to the reference count: the buffer cannot go away in between.
    const st = useBuffers.getState().buffers[key]
    const model = await acquireBuffer(projectId, path, {
      path,
      content: null,
      binary: false,
      size: 0,
      mtime: 0,
      etag: st?.etag ?? null,
      tooLarge: false,
      sensitive: st?.sensitive ?? false,
      encoding: st?.encoding ?? 'utf-8',
      mime: '',
      readOnly: st?.readOnly ?? false,
    }, st?.sensitive ?? false)
    return { model, release }
  }
  const meta = await filesApi.read(projectId, path)
  if (meta.content === null) throw new Error(`${path} cannot be edited as text`)
  const model = await acquireBuffer(projectId, path, meta, false)
  return { model, release }
}

export function isDirty(projectId: string | null, path: string): boolean {
  return !!useBuffers.getState().buffers[bufferKey(projectId, path)]?.dirty
}

export function isReadOnly(projectId: string | null, path: string): boolean {
  const b = useBuffers.getState().buffers[bufferKey(projectId, path)]
  return !projectId || !!b?.readOnly
}

/** Save the file's buffer (etag-checked). True when written. */
export function saveModel(projectId: string | null, path: string): Promise<boolean> {
  return saveBuffer(bufferKey(projectId, path))
}

/** Subscribe to buffer changes (dirty, saved, closed) of any file. */
export function onBuffersChange(fn: () => void): () => void {
  return useBuffers.subscribe(fn)
}

/** Called with a file each time its buffer was saved (or reloaded from disk). */
export function onBufferRevision(fn: (projectId: string | null, path: string) => void): () => void {
  let prev = useBuffers.getState().buffers
  return useBuffers.subscribe((s) => {
    const cur = s.buffers
    if (cur === prev) return
    for (const [k, b] of Object.entries(cur)) {
      const p = prev[k]
      if (p && b.revision > p.revision && !b.dirty) fn(b.projectId, b.path)
    }
    prev = cur
  })
}

/** Whether any editor buffers are open (Monaco is loaded then). */
export function hasOpenBuffers(): boolean {
  return Object.keys(useBuffers.getState().buffers).length > 0
}
