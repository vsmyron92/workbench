// Open-file buffers: one Monaco model per file, shared by every editor panel that
// shows it (e.g. "Open to the side"), with its saved revision (sha256 etag) and
// dirty / conflict state.
//
// External changes: `fs.changed` for a buffer's path (or a resync) triggers a
// stat. A clean buffer silently reloads with a minimal edit (cursor and scroll
// stay put); a dirty one gets a conflict the editor shows as a banner
// (Reload / Keep mine / Compare). Saves send the etag they are based on; a 409
// becomes a `save` conflict. Unsaved text survives closing the tab and reloading
// the page as a draft that is restored if the file did not change meanwhile.

import type { editor, IDisposable } from 'monaco-editor'
import { create } from 'zustand'
import { ApiError } from '@/api/client'
import { subscribe } from '@/api/events'
import { toastError } from '@/shell/actions'
import { filesApi, type FileContent } from './api'
import { bufferKey } from './paths'
import { minimalEdit } from './text'

export type ConflictKind = 'changed' | 'deleted' | 'save'

export interface BufferState {
  key: string
  projectId: string | null
  path: string
  etag: string | null
  dirty: boolean
  saving: boolean
  readOnly: boolean
  encoding: FileContent['encoding']
  /** Opened with `allowSensitive` (reloads must ask again). */
  sensitive: boolean
  conflict: { kind: ConflictKind; diskEtag: string | null } | null
  restoredDraft: boolean
  /** Bumped on every save/reload, for views that follow the disk version. */
  revision: number
}

interface Entry {
  model: editor.ITextModel
  refs: number
  savedVersion: number
  disposables: IDisposable[]
  listeners: Set<(text: string) => void>
  notifyTimer?: number
}

export const useBuffers = create<{ buffers: Record<string, BufferState> }>()(() => ({ buffers: {} }))

const entries = new Map<string, Entry>()
const pending = new Map<string, Promise<editor.ITextModel>>()

function state(key: string): BufferState | undefined {
  return useBuffers.getState().buffers[key]
}

function patch(key: string, p: Partial<BufferState>) {
  useBuffers.setState((s) => {
    const cur = s.buffers[key]
    if (!cur) return s
    return { buffers: { ...s.buffers, [key]: { ...cur, ...p } } }
  })
}

// ---------------------------------------------------------------- drafts

const DRAFT_PREFIX = 'wb.files.draft.'
const MAX_DRAFT = 1_000_000

interface Draft {
  content: string
  etag: string | null
}

const drafts = new Map<string, Draft>()

function takeDraft(key: string): Draft | null {
  const mem = drafts.get(key)
  drafts.delete(key)
  let stored: Draft | null = null
  try {
    const raw = sessionStorage.getItem(DRAFT_PREFIX + key)
    if (raw) stored = JSON.parse(raw) as Draft
    sessionStorage.removeItem(DRAFT_PREFIX + key)
  } catch {
    /* unavailable */
  }
  return mem ?? stored
}

/** Keep unsaved buffers across a page reload (called on `pagehide`). */
export function stashDrafts() {
  for (const [key, e] of entries) {
    const st = state(key)
    if (!st?.dirty) continue
    const content = e.model.getValue()
    if (content.length > MAX_DRAFT) continue
    try {
      sessionStorage.setItem(DRAFT_PREFIX + key, JSON.stringify({ content, etag: st.etag } satisfies Draft))
    } catch {
      /* quota: best effort */
    }
  }
}

export function hasDirtyBuffers(): boolean {
  return Object.values(useBuffers.getState().buffers).some((b) => b.dirty)
}

// ---------------------------------------------------------------- lifecycle

async function monacoNs() {
  return (await import('@/lib/monacoSetup')).monaco
}

export function modelUri(monaco: Awaited<ReturnType<typeof monacoNs>>, projectId: string | null, path: string) {
  const rel = path.startsWith('/') ? path : '/' + path
  return monaco.Uri.from({ scheme: 'file', path: `/${projectId ?? '~abs'}${rel}` })
}

/**
 * Get (or create) the model for a file. Every call must be paired with
 * `releaseBuffer(key)`.
 */
export async function acquireBuffer(projectId: string | null, path: string, meta: FileContent, sensitive: boolean): Promise<editor.ITextModel> {
  const key = bufferKey(projectId, path)
  const existing = entries.get(key)
  if (existing) {
    existing.refs++
    return existing.model
  }
  const inflight = pending.get(key)
  if (inflight) {
    const model = await inflight
    entries.get(key)!.refs++
    return model
  }
  const create = (async () => {
    const monaco = await monacoNs()
    const { languageFor } = await import('@/lib/monacoSetup')
    const uri = modelUri(monaco, projectId, path)
    monaco.editor.getModel(uri)?.dispose()
    const model = monaco.editor.createModel(meta.content ?? '', languageFor(path), uri)
    const entry: Entry = { model, refs: 0, savedVersion: model.getAlternativeVersionId(), disposables: [], listeners: new Set() }
    const draft = takeDraft(key)
    let restored = false
    if (draft && draft.etag === meta.etag && draft.content !== meta.content && !meta.readOnly) {
      applyText(model, draft.content)
      restored = true
    }
    entry.disposables.push(
      model.onDidChangeContent(() => {
        const dirty = model.getAlternativeVersionId() !== entry.savedVersion
        if (state(key)?.dirty !== dirty) patch(key, { dirty })
        scheduleNotify(entry)
      }),
    )
    entries.set(key, entry)
    useBuffers.setState((s) => ({
      buffers: {
        ...s.buffers,
        [key]: {
          key,
          projectId,
          path,
          etag: meta.etag,
          dirty: restored,
          saving: false,
          readOnly: meta.readOnly || !projectId,
          encoding: meta.encoding,
          sensitive,
          conflict: null,
          restoredDraft: restored,
          revision: 0,
        },
      },
    }))
    ensureWatching()
    return model
  })()
  pending.set(key, create)
  try {
    const model = await create
    entries.get(key)!.refs++
    return model
  } finally {
    pending.delete(key)
  }
}

export function releaseBuffer(key: string) {
  const e = entries.get(key)
  if (!e) return
  if (--e.refs > 0) return
  const st = state(key)
  if (st?.dirty) {
    const content = e.model.getValue()
    if (content.length <= MAX_DRAFT * 5) drafts.set(key, { content, etag: st.etag })
  }
  window.clearTimeout(e.notifyTimer)
  e.disposables.forEach((d) => d.dispose())
  e.model.dispose()
  entries.delete(key)
  useBuffers.setState((s) => {
    const { [key]: _gone, ...rest } = s.buffers
    return { buffers: rest }
  })
}

export function getModel(key: string): editor.ITextModel | null {
  return entries.get(key)?.model ?? null
}

/** Live text of an open buffer (Markdown previews follow unsaved edits). */
export function onBufferText(key: string, fn: (text: string) => void): () => void {
  const e = entries.get(key)
  if (!e) return () => {}
  e.listeners.add(fn)
  return () => e.listeners.delete(fn)
}

function scheduleNotify(e: Entry) {
  if (!e.listeners.size || e.notifyTimer) return
  e.notifyTimer = window.setTimeout(() => {
    e.notifyTimer = undefined
    if (e.model.isDisposed()) return
    const text = e.model.getValue()
    e.listeners.forEach((l) => l(text))
  }, 200)
}

/** Replace the model text with a minimal edit (undoable; cursors outside the change stay). */
export function applyText(model: editor.ITextModel, text: string) {
  const cur = model.getValue()
  const edit = minimalEdit(cur, text)
  if (!edit) return
  const start = model.getPositionAt(edit.start)
  const end = model.getPositionAt(edit.end)
  model.pushStackElement()
  model.pushEditOperations(
    [],
    [{ range: { startLineNumber: start.lineNumber, startColumn: start.column, endLineNumber: end.lineNumber, endColumn: end.column }, text: edit.text }],
    () => null,
  )
  model.pushStackElement()
}

// ---------------------------------------------------------------- save / reload / conflicts

/** Save a buffer. Resolves to true when written. */
export async function saveBuffer(key: string, opts: { force?: boolean } = {}): Promise<boolean> {
  const e = entries.get(key)
  const st = state(key)
  if (!e || !st || !st.projectId || st.readOnly || st.saving) return false
  const version = e.model.getAlternativeVersionId()
  const content = e.model.getValue()
  patch(key, { saving: true })
  try {
    const r = await filesApi.write(st.projectId, st.path, content, st.etag, opts.force)
    e.savedVersion = version
    patch(key, {
      etag: r.etag,
      saving: false,
      conflict: null,
      restoredDraft: false,
      dirty: e.model.getAlternativeVersionId() !== version,
      revision: (state(key)?.revision ?? 0) + 1,
    })
    return true
  } catch (err) {
    patch(key, { saving: false })
    if (err instanceof ApiError && err.code === 'conflict') {
      const s = await filesApi.stat(st.projectId, st.path, st.sensitive).catch(() => null)
      patch(key, { conflict: { kind: s && !s.exists ? 'deleted' : 'save', diskEtag: s?.etag ?? null } })
    } else {
      toastError(err, `Could not save ${st.path}`)
    }
    return false
  }
}

/** Replace the buffer with the disk version (discarding unsaved edits). */
export async function reloadBuffer(key: string): Promise<void> {
  const e = entries.get(key)
  const st = state(key)
  if (!e || !st) return
  let meta: FileContent
  try {
    meta = await filesApi.read(st.projectId, st.path, st.sensitive)
  } catch (err) {
    if (err instanceof ApiError && err.status === 404) patch(key, { conflict: { kind: 'deleted', diskEtag: null } })
    else toastError(err, `Could not reload ${st.path}`)
    return
  }
  if (meta.content === null) return
  if (e.model.isDisposed()) return
  applyText(e.model, meta.content)
  e.savedVersion = e.model.getAlternativeVersionId()
  patch(key, { etag: meta.etag, dirty: false, conflict: null, restoredDraft: false, revision: (state(key)?.revision ?? 0) + 1 })
}

/** Keep the buffer; the next save overwrites the disk (or recreates a deleted file). */
export function keepMine(key: string) {
  const st = state(key)
  if (!st?.conflict) return
  patch(key, { etag: st.conflict.kind === 'deleted' ? null : st.conflict.diskEtag, conflict: null })
  const e = entries.get(key)
  if (e) {
    // Stays dirty until saved, even without further edits.
    e.savedVersion = -1
    patch(key, { dirty: true })
  }
}

export function dismissRestored(key: string) {
  patch(key, { restoredDraft: false })
}

const checking = new Set<string>()

/** Compare the buffer with the disk and reload / flag a conflict. */
export async function checkDisk(key: string): Promise<void> {
  const st = state(key)
  if (!st || checking.has(key)) return
  if (st.saving) {
    window.setTimeout(() => void checkDisk(key), 300)
    return
  }
  checking.add(key)
  try {
    const s = await filesApi.stat(st.projectId, st.path, st.sensitive).catch(() => null)
    const cur = state(key)
    if (!s || !cur) return
    if (!s.exists) {
      if (cur.etag !== null && cur.conflict?.kind !== 'deleted') patch(key, { conflict: { kind: 'deleted', diskEtag: null } })
      return
    }
    if (s.etag === null || s.etag === cur.etag) {
      if (cur.conflict?.kind === 'deleted') patch(key, { conflict: null })
      return
    }
    if (cur.conflict?.kind === 'save') return
    if (!cur.dirty) await reloadBuffer(key)
    else patch(key, { conflict: { kind: 'changed', diskEtag: s.etag } })
  } finally {
    checking.delete(key)
  }
}

export function checkAll(filter?: (b: BufferState) => boolean) {
  for (const b of Object.values(useBuffers.getState().buffers)) if (!filter || filter(b)) void checkDisk(b.key)
}

let watching = false

function ensureWatching() {
  if (watching) return
  watching = true
  subscribe('fs.changed', (ev) => {
    const data = ev.data as { paths?: string[]; overflow?: boolean }
    const paths = data.paths ?? []
    checkAll(
      (b) =>
        b.projectId === ev.projectId &&
        (!!data.overflow || paths.some((p) => b.path === p || b.path.startsWith(p + '/'))),
    )
  })
  subscribe('resync', () => checkAll())
  // Files outside projects are not watched: check them when the window regains focus.
  let last = 0
  const onFocus = () => {
    if (Date.now() - last < 3000) return
    last = Date.now()
    checkAll()
  }
  window.addEventListener('focus', onFocus)
  document.addEventListener('visibilitychange', () => document.visibilityState === 'visible' && onFocus())
}

export { bufferKey }
