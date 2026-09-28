// The `localHistory` panel (CLion's Local History): a file's versions, or the
// changes of the files in a folder (the whole project: Recent Changes), grouped
// by day, with a Monaco diff of the selected version.
//
// File history compares the version with the current buffer (the shared editor
// model, so unsaved edits show and Revert lands in it: dirty and undoable;
// Ctrl+S saves). The diff's gutter arrows revert single changes, and "Revert
// Selected Lines" the changes inside the selection. Folder history shows what
// each change did (the version before → this one).

import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent, type MouseEvent } from 'react'
import { useInfiniteQuery, useQuery, useQueryClient } from '@tanstack/react-query'
import type { editor } from 'monaco-editor'
import {
  AlertTriangle,
  Bot,
  Clock,
  Copy,
  ExternalLink,
  FileText,
  FileX,
  GitBranch,
  HardDrive,
  ListTree,
  RefreshCw,
  RotateCcw,
  Save,
  Tag,
  Undo2,
} from 'lucide-react'
import { ApiError } from '@/api/client'
import { useInvalidateOn } from '@/api/events'
import { toast, toastError } from '@/shell/actions'
import type { PanelProps } from '@/shell/types'
import { useUi } from '@/state/store'
import { Button, EmptyState, ErrorBox, IconButton, Loading, MonacoDiffEditor, showMenu, Splitter, formatBytes, type MenuEntry } from '@/ui'
import { filesApi } from '../api'
import { acquireBuffer, applyText, bufferKey, keepMine, releaseBuffer, reloadBuffer, saveBuffer, useBuffers, type BufferState } from '../buffers'
import { openFile, revealInTree } from '../openers'
import { basename, dirname } from '../paths'
import { copyText } from '../text'
import { historyApi, hk, type HistoryEntry, type HistoryKind } from './api'
import { clockTime, describeEntry, groupByDay, hasContent, historyTitle, isLabel, keepSelection, revertLines, shortWhen, UNTRACKED_TEXT } from './model'
import { putLabel, showLocalHistory, type LocalHistoryParams } from './open'

type Monaco = typeof import('monaco-editor')
type Compare = 'current' | 'previous'

const KIND_ICON: Record<HistoryKind, typeof Save> = {
  save: Save,
  disk: HardDrive,
  agent: Bot,
  base: FileText,
  deleted: FileX,
  label: Tag,
  auto: GitBranch,
}

function KindIcon({ kind }: { kind: HistoryKind }) {
  const I = KIND_ICON[kind] ?? FileText
  return <I size={14} className={`wb-lh-kind k-${kind}`} />
}

export function LocalHistoryPanel({ params, setTitle }: PanelProps<LocalHistoryParams>) {
  const { projectId, path } = params
  const dir = !!params.dir
  const qc = useQueryClient()
  useEffect(() => setTitle(historyTitle(path, dir)), [path, dir, setTitle])

  const list = useInfiniteQuery({
    queryKey: hk.list(projectId, path, dir),
    queryFn: ({ pageParam, signal }) => (dir ? historyApi.dir : historyApi.file)(projectId, path, pageParam, signal),
    initialPageParam: undefined as number | undefined,
    getNextPageParam: (last) => (last.truncated ? last.entries[last.entries.length - 1]?.id : undefined),
    enabled: !!projectId,
  })
  useInvalidateOn(qc, ['files.history'], (ev) => (ev.projectId === projectId ? hk.list(projectId, path, dir) : null))
  const entries = useMemo(() => list.data?.pages.flatMap((p) => p.entries) ?? [], [list.data])
  const untracked = list.data?.pages[0]?.untracked

  // The file on disk now: the first selection skips versions identical to it.
  const disk = useQuery({
    queryKey: ['files', 'history-stat', projectId, path],
    queryFn: () => filesApi.stat(projectId, path),
    enabled: !dir && !!path,
    staleTime: 2_000,
    retry: false,
  })
  const [selected, setSelected] = useState<number | null>(params.id ?? null)
  useEffect(() => {
    if (params.id !== undefined) setSelected(params.id)
  }, [params.id])
  const waitForDisk = !dir && disk.isLoading
  useEffect(() => {
    if (!waitForDisk) setSelected((s) => keepSelection(entries, s, disk.data?.etag))
  }, [entries, waitForDisk, disk.data?.etag])
  const [compare, setCompare] = useState<Compare>(dir ? 'previous' : 'current')
  const [listWidth, setListWidth] = useState(300)
  const startWidth = useRef(300)
  const listRef = useRef<HTMLDivElement>(null)
  const entry = entries.find((e) => e.id === selected) ?? null

  const groups = useMemo(() => groupByDay(entries), [entries])
  const selectable = useMemo(() => entries.filter((e) => !isLabel(e)), [entries])

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return
    e.preventDefault()
    const i = selectable.findIndex((x) => x.id === selected)
    const next = selectable[Math.max(0, Math.min(selectable.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1)))]
    if (next) {
      setSelected(next.id)
      listRef.current?.querySelector(`[data-entry="${next.id}"]`)?.scrollIntoView({ block: 'nearest' })
    }
  }

  const rowMenu = (ev: MouseEvent, e: HistoryEntry) => {
    if (isLabel(e)) return
    setSelected(e.id)
    const items: MenuEntry[] = dir
      ? [
          { label: 'Show File History', icon: Clock, run: () => showLocalHistory(projectId, e.path, false, e.id) },
          { label: 'Open File', icon: ExternalLink, disabled: e.kind === 'deleted', run: () => openFile({ projectId, path: e.path }) },
          { label: 'Reveal in Project Tree', icon: ListTree, disabled: e.kind === 'deleted', run: () => revealInTree(projectId, e.path) },
          'separator',
          { label: 'Copy Path', icon: Copy, run: () => void copyText(e.path) },
        ]
      : [
          { label: 'Compare with Current', icon: RotateCcw, run: () => setCompare('current') },
          { label: 'Compare with Previous Version', icon: Clock, run: () => setCompare('previous') },
          'separator',
          { label: 'Copy Version Text', icon: Copy, disabled: !hasContent(e), run: () => void copyRevision(projectId, e.id) },
        ]
    showMenu(ev, items)
  }

  const stats = useQuery({
    queryKey: hk.stats(projectId),
    queryFn: ({ signal }) => historyApi.stats(projectId, signal),
    enabled: dir && !path,
    staleTime: 30_000,
  })

  return (
    <div className="wb-fill wb-lh">
      <div className="wb-toolbar wb-lh-toolbar">
        <Clock size={15} className="wb-lh-title-icon" />
        <span className="title wb-ellipsis" title={path || 'The whole project'}>
          {dir && !path ? 'Recent Changes' : path}
        </span>
        <span className="spacer" />
        <span className="wb-lh-seg" role="group" aria-label="Compare with">
          <button className={compare === 'current' ? 'active' : ''} onClick={() => setCompare('current')} title="The version against the file now (your buffer)">
            vs Current
          </button>
          <button className={compare === 'previous' ? 'active' : ''} onClick={() => setCompare('previous')} title="What this version changed">
            vs Previous
          </button>
        </span>
        <IconButton icon={Tag} size="small" label={path ? `Put Label on ${dir ? 'this folder' : 'this file'}…` : 'Put Label…'} onClick={() => void putLabel(projectId, path)} />
        <IconButton icon={RefreshCw} size="small" label="Refresh" onClick={() => void list.refetch()} />
      </div>
      <div className="wb-lh-body">
        <div className="wb-lh-list" style={{ width: listWidth }} ref={listRef} tabIndex={0} onKeyDown={onKeyDown} aria-label="Versions">
          {list.error ? (
            <ErrorBox error={list.error} onRetry={() => void list.refetch()} />
          ) : list.isLoading ? (
            <Loading label="Loading history…" />
          ) : untracked ? (
            <EmptyState icon={Clock} title="Not recorded">
              {UNTRACKED_TEXT[untracked]}
            </EmptyState>
          ) : entries.length === 0 ? (
            <EmptyState icon={Clock} title={dir ? 'No recent changes' : 'No local history yet'}>
              {dir
                ? 'Saves in Workbench and changes on disk (yours or an agent’s) show here as they happen. History is kept for 7 days.'
                : 'Versions appear when you save this file in Workbench or it changes on disk. History is kept for 7 days.'}
            </EmptyState>
          ) : (
            <>
              {groups.map((g) => (
                <div key={g.key} className="wb-lh-group">
                  <div className="wb-lh-day">{g.label}</div>
                  {g.entries.map((e) =>
                    isLabel(e) ? (
                      <div key={e.id} className={`wb-lh-label k-${e.kind}`} title={`${e.kind === 'auto' ? 'Automatic label' : 'Label'} · ${new Date(e.ts).toLocaleString()}${e.path ? ` · on ${e.path}` : ''}`}>
                        <KindIcon kind={e.kind} />
                        <span className="wb-ellipsis">{e.label}</span>
                        <span className="wb-lh-time">{clockTime(e.ts)}</span>
                      </div>
                    ) : (
                      <div
                        key={e.id}
                        data-entry={e.id}
                        className={['wb-list-row', 'wb-lh-row', dir && 'two', e.id === selected && 'selected'].filter(Boolean).join(' ')}
                        onClick={() => setSelected(e.id)}
                        onDoubleClick={() => (dir ? showLocalHistory(projectId, e.path, false, e.id) : e.kind !== 'deleted' && openFile({ projectId, path }))}
                        onContextMenu={(ev) => rowMenu(ev, e)}
                        title={`${new Date(e.ts).toLocaleString()} · ${describeEntry(e)}${hasContent(e) ? ` · ${formatBytes(e.size)}` : ''}`}
                      >
                        <KindIcon kind={e.kind} />
                        {dir ? (
                          <span className="wb-lh-main">
                            <span className="wb-lh-line wb-ellipsis">
                              <span className={e.kind === 'deleted' ? 'wb-lh-file wb-vcs-deleted' : 'wb-lh-file'}>{basename(e.path)}</span>
                              <span className="wb-lh-dir">{dirname(e.path) || ''}</span>
                            </span>
                            <span className="wb-lh-line wb-lh-sub wb-ellipsis">
                              {clockTime(e.ts)} · {describeEntry(e)}
                            </span>
                          </span>
                        ) : (
                          <>
                            <span className="wb-lh-time">{clockTime(e.ts)}</span>
                            <span className="wb-lh-what wb-ellipsis">{describeEntry(e)}</span>
                            {hasContent(e) && <span className="wb-lh-size">{formatBytes(e.size)}</span>}
                          </>
                        )}
                      </div>
                    ),
                  )}
                </div>
              ))}
              {list.hasNextPage && (
                <div className="wb-lh-more">
                  <Button size="small" loading={list.isFetchingNextPage} onClick={() => void list.fetchNextPage()}>
                    Load older
                  </Button>
                </div>
              )}
              {dir && !path && stats.data && (
                <div className="wb-lh-foot" title={`Kept ${stats.data.retentionDays} days, at most ${stats.data.maxVersions} versions per file, files up to ${formatBytes(stats.data.maxFileBytes)}`}>
                  {stats.data.files} file{stats.data.files === 1 ? '' : 's'} · {formatBytes(stats.data.bytes)} of {formatBytes(stats.data.maxBytes)} · kept {stats.data.retentionDays} days
                </div>
              )}
            </>
          )}
        </div>
        <Splitter
          direction="v"
          onResizeStart={() => (startWidth.current = listWidth)}
          onResize={(d) => setListWidth(Math.max(220, Math.min(720, startWidth.current + d)))}
        />
        <div className="wb-lh-diff">
          {entry && !isLabel(entry) ? (
            <RevisionView key={dir ? entry.path : path} projectId={projectId} entry={entry} compare={compare} fileMode={!dir} />
          ) : (
            <EmptyState icon={Clock} title={entries.length ? 'Select a version' : 'Nothing to compare'} />
          )}
        </div>
      </div>
    </div>
  )
}

async function copyRevision(projectId: string, id: number) {
  try {
    const r = await historyApi.revision(projectId, id)
    const ok = r.content !== null && (await copyText(r.content))
    toast(ok ? 'success' : 'error', ok ? 'Copied the version text' : 'Could not copy', { timeout: 2000 })
  } catch (e) {
    toastError(e, 'Could not read the version')
  }
}

// ---------------------------------------------------------------- the diff side

/** The file's shared editor buffer (null while loading; `missing` when the file is gone). */
export function useFileBuffer(projectId: string, path: string, enabled: boolean) {
  const [nonce, setNonce] = useState(0)
  const [state, setState] = useState<{ model: editor.ITextModel | null; missing: boolean; error: unknown; readOnly: boolean }>({
    model: null,
    missing: false,
    error: null,
    readOnly: false,
  })
  useEffect(() => {
    if (!enabled) return
    let cancelled = false
    let acquired = false
    const key = bufferKey(projectId, path)
    setState({ model: null, missing: false, error: null, readOnly: false })
    filesApi.read(projectId, path).then(
      async (meta) => {
        if (cancelled) return
        if (meta.content === null) {
          setState({ model: null, missing: false, error: new Error('The file on disk cannot be shown as text.'), readOnly: true })
          return
        }
        const m = await acquireBuffer(projectId, path, meta, false)
        if (cancelled) releaseBuffer(key)
        else {
          acquired = true
          setState({ model: m, missing: false, error: null, readOnly: meta.readOnly })
        }
      },
      (e) => {
        if (cancelled) return
        if (e instanceof ApiError && e.status === 404) setState({ model: null, missing: true, error: null, readOnly: true })
        else setState({ model: null, missing: false, error: e, readOnly: true })
      },
    )
    return () => {
      cancelled = true
      // After the diff widget that shows the model let go of it (it unmounts in the
      // same commit; disposing the model first throws in Monaco).
      if (acquired) window.setTimeout(() => releaseBuffer(key), 0)
    }
  }, [projectId, path, enabled, nonce])
  return { ...state, reload: () => setNonce((n) => n + 1) }
}

/**
 * Save the shared buffer from this panel. When the disk moved on, the save is
 * refused and the buffer gets a conflict: say so here (the editor, which shows
 * conflicts, may not be open), and the banner above the diff offers the way out.
 */
export async function saveFromHistory(key: string, path: string) {
  if (await saveBuffer(key)) return
  if (useBuffers.getState().buffers[key]?.conflict) {
    toast('warning', `Not saved: ${basename(path)} changed on disk`, { detail: 'Reload it or keep yours first (see above the diff).' })
  }
}

/** The editor's conflict choices, for a buffer this panel edits. */
export function ConflictBanner({ conflict, bufKey, readOnly, onOpen }: { conflict: NonNullable<BufferState['conflict']>; bufKey: string; readOnly: boolean; onOpen: () => void }) {
  if (conflict.kind === 'deleted') {
    return (
      <div className="wb-banner warning">
        <FileX size={14} />
        <span className="wb-grow">The file was deleted or moved on disk.</span>
        {!readOnly && <Button size="small" onClick={() => keepMine(bufKey)}>Keep (Save Recreates It)</Button>}
      </div>
    )
  }
  const saveRefused = conflict.kind === 'save'
  const message = saveRefused ? 'Not saved: the file changed on disk since it was loaded.' : 'The file changed on disk, and the buffer has unsaved changes.'
  return (
    <div className={`wb-banner ${saveRefused ? 'danger' : 'warning'}`}>
      <AlertTriangle size={14} />
      <span className="wb-grow wb-ellipsis" title={message}>
        {message}
      </span>
      <Button size="small" onClick={() => void reloadBuffer(bufKey)} title="Discard the buffer's changes and load the file on disk">
        {saveRefused ? 'Discard Mine' : 'Reload from Disk'}
      </Button>
      {saveRefused ? (
        <Button size="small" onClick={() => void saveBuffer(bufKey, { force: true })} title="Write the buffer over the file on disk">
          Overwrite
        </Button>
      ) : (
        <Button size="small" onClick={() => keepMine(bufKey)} title="Keep the buffer; the next save overwrites the file on disk">
          Keep Mine
        </Button>
      )}
      <Button size="small" icon={ExternalLink} onClick={onOpen} title="Open the file in the editor (Compare shows both)">
        Open in Editor
      </Button>
    </div>
  )
}

export function useRevisionText(projectId: string, id: number | null) {
  return useQuery({
    queryKey: hk.revision(projectId, id ?? 0),
    queryFn: ({ signal }) => historyApi.revision(projectId, id!, signal),
    enabled: id !== null,
    staleTime: Infinity,
    retry: false,
  })
}

function RevisionView({ projectId, entry, compare, fileMode }: { projectId: string; entry: HistoryEntry; compare: Compare; fileMode: boolean }) {
  const path = entry.path
  const rev = useRevisionText(projectId, entry.id)
  const prevEntry = rev.data?.previous ?? null
  // A deletion shows the last content it removed.
  const shownId = entry.kind === 'deleted' ? (prevEntry && hasContent(prevEntry) ? prevEntry.id : null) : entry.id
  const shown = useRevisionText(projectId, entry.kind === 'deleted' ? shownId : null)
  const text = entry.kind === 'deleted' ? (shown.data?.content ?? '') : (rev.data?.content ?? null)
  const prevForDiff = compare === 'previous' && entry.kind !== 'deleted' && prevEntry && hasContent(prevEntry) ? prevEntry.id : null
  const prev = useRevisionText(projectId, prevForDiff)
  const wantBuffer = compare === 'current'
  const buf = useFileBuffer(projectId, path, wantBuffer)
  const key = bufferKey(projectId, path)
  const dirty = useBuffers((s) => s.buffers[key]?.dirty ?? false)
  const conflict = useBuffers((s) => s.buffers[key]?.conflict ?? null)

  const error = rev.error ?? shown.error ?? prev.error
  if (error) return <ErrorBox error={error} onRetry={() => void rev.refetch()} />
  if (rev.isLoading || text === null || (prevForDiff !== null && prev.isLoading) || (shownId !== null && entry.kind === 'deleted' && shown.isLoading)) {
    return <Loading label="Loading version…" />
  }

  const when = `${shortWhen(entry.ts)} · ${describeEntry(entry)}`
  const restore = async () => {
    try {
      await filesApi.write(projectId, path, text, null)
      toast('success', `Restored ${basename(path)}`)
      buf.reload()
    } catch (e) {
      if (e instanceof ApiError && e.code === 'conflict') toast('warning', `${basename(path)} exists again: compare with it instead`)
      else toastError(e, `Could not restore ${basename(path)}`)
    }
  }

  // What this version changed: the previous version → this one (read-only).
  if (compare === 'previous') {
    const before = entry.kind === 'deleted' ? text : prevForDiff !== null ? (prev.data?.content ?? '') : ''
    const after = entry.kind === 'deleted' ? '' : text
    const beforeLabel = entry.kind === 'deleted' ? 'Last version' : prevEntry && hasContent(prevEntry) ? `${shortWhen(prevEntry.ts)} · ${describeEntry(prevEntry)}` : 'Nothing before'
    return (
      <div className="wb-fill">
        <div className="wb-lh-head">
          <span className="side wb-ellipsis">{beforeLabel}</span>
          <span className="side wb-ellipsis">{entry.kind === 'deleted' ? 'Deleted' : when}</span>
        </div>
        <HistoryDiff path={path} original={before} modified={after} />
        {fileMode ? null : (
          <div className="wb-lh-actions">
            <Button size="small" icon={Clock} onClick={() => showLocalHistory(projectId, path, false, entry.id)}>
              Show File History
            </Button>
            {entry.kind !== 'deleted' && (
              <Button size="small" icon={ExternalLink} onClick={() => openFile({ projectId, path })}>
                Open File
              </Button>
            )}
          </div>
        )}
      </div>
    )
  }

  // This version → the file now (the editor buffer, editable).
  if (buf.error) return <ErrorBox error={buf.error} onRetry={buf.reload} />
  if (buf.missing) {
    return (
      <div className="wb-fill">
        <div className="wb-lh-head">
          <span className="side wb-ellipsis">{when}</span>
          <span className="side wb-ellipsis wb-vcs-deleted">Not on disk now</span>
        </div>
        <HistoryDiff path={path} original={text} modified="" />
        <div className="wb-lh-actions">
          <Button size="small" variant="primary" icon={Undo2} onClick={() => void restore()} disabled={!text && entry.kind === 'deleted' && shownId === null}>
            Restore File
          </Button>
          <span className="wb-subtle wb-small">Recreates {basename(path)} with this version.</span>
        </div>
      </div>
    )
  }
  if (!buf.model) return <Loading label="Opening the file…" />
  const model = buf.model
  const revert = () => {
    if (model.isDisposed()) return
    applyText(model, text)
    toast('success', `Reverted ${basename(path)} to ${clockTime(entry.ts)}`, {
      detail: 'Not saved yet: Ctrl+Z undoes it, Ctrl+S saves.',
      action: { label: 'Save', run: () => void saveFromHistory(key, path) },
    })
  }
  const revertSelected = (from: number, to: number) => {
    if (model.isDisposed()) return
    const next = revertLines(model.getValue(), text, from, to)
    if (next === null) toast('info', 'No changes in the selected lines', { timeout: 2000 })
    else applyText(model, next)
  }
  const state = conflict?.kind === 'deleted' ? ' (deleted on disk)' : conflict ? ' (changed on disk)' : dirty ? ' (unsaved)' : ''
  return (
    <div className="wb-fill">
      {conflict && <ConflictBanner conflict={conflict} bufKey={key} readOnly={buf.readOnly} onOpen={() => openFile({ projectId, path })} />}
      <div className="wb-lh-head">
        <span className="side wb-ellipsis">{when}</span>
        <span className={`side wb-ellipsis${conflict ? ' wb-lh-conflict' : ''}`}>
          Current{state}
          {buf.readOnly ? ' · read-only' : ''}
        </span>
      </div>
      <HistoryDiff
        path={path}
        original={text}
        modified={model}
        readOnly={buf.readOnly}
        onSave={() => void saveFromHistory(key, path)}
        onRevertLines={buf.readOnly ? undefined : revertSelected}
      />
      <div className="wb-lh-actions">
        <Button size="small" variant="primary" icon={RotateCcw} disabled={buf.readOnly} onClick={revert} title="Replace the buffer with this version (undoable; not saved)">
          Revert to This Version
        </Button>
        {dirty && (
          <Button size="small" icon={Save} onClick={() => void saveFromHistory(key, path)}>
            Save
          </Button>
        )}
        {!fileMode && (
          <Button size="small" icon={Clock} onClick={() => showLocalHistory(projectId, path, false, entry.id)}>
            Show File History
          </Button>
        )}
        <span className="wb-subtle wb-small wb-ellipsis">Arrows between the sides revert one change · Ctrl+Alt+Z reverts the selected lines</span>
      </div>
    </div>
  )
}

/**
 * Monaco diff of a version (left, read-only) with a text or the live buffer model
 * (right). Models made here are disposed once the widget let go of them; the
 * buffer model belongs to the buffers module.
 */
export function HistoryDiff({
  path,
  original,
  modified,
  readOnly = true,
  onSave,
  onRevertLines,
}: {
  path: string
  original: string
  modified: string | editor.ITextModel
  readOnly?: boolean
  onSave?: () => void
  onRevertLines?: (from: number, to: number) => void
}) {
  const theme = useUi((s) => s.prefs.theme)
  const fontSize = useUi((s) => s.prefs.editorFontSize)
  const [inst, setInst] = useState<{ ed: editor.IStandaloneDiffEditor; monaco: Monaco } | null>(null)
  const initial = useRef<editor.IDiffEditorModel | null>(null)
  const live = useRef({ onSave, onRevertLines })
  live.current = { onSave, onRevertLines }

  useEffect(() => {
    if (!inst) return
    let disposed = false
    const { ed, monaco } = inst
    const made: editor.ITextModel[] = []
    void import('@/lib/monacoSetup').then(({ languageFor }) => {
      if (disposed) return
      const lang = typeof modified === 'string' ? languageFor(path) : modified.getLanguageId()
      const orig = monaco.editor.createModel(original, lang)
      made.push(orig)
      let mod: editor.ITextModel
      if (typeof modified === 'string') {
        mod = monaco.editor.createModel(modified, lang)
        made.push(mod)
      } else {
        mod = modified
      }
      if (mod.isDisposed()) return
      ed.setModel({ original: orig, modified: mod })
      const first = initial.current
      initial.current = null
      if (first) {
        first.original.dispose()
        if (!first.modified.isDisposed() && first.modified !== mod) first.modified.dispose()
      }
    })
    return () => {
      disposed = true
      // After the widget switched to the next models (or was disposed).
      const release = () => made.forEach((m) => !m.isDisposed() && !m.isAttachedToEditor() && m.dispose())
      window.setTimeout(release, 0)
      window.setTimeout(release, 1000)
    }
  }, [inst, original, modified, path])

  const options = useMemo<editor.IDiffEditorConstructionOptions>(
    () => ({
      automaticLayout: true,
      renderSideBySide: true,
      originalEditable: false,
      readOnly,
      minimap: { enabled: false },
      fontSize,
      scrollBeyondLastLine: false,
      renderMarginRevertIcon: !readOnly,
      ignoreTrimWhitespace: false,
    }),
    [readOnly, fontSize],
  )

  const onMount = useCallback((ed: editor.IStandaloneDiffEditor, monaco: Monaco) => {
    initial.current = ed.getModel()
    const mod = ed.getModifiedEditor()
    const actions = [
      mod.addAction({ id: 'wb.save', label: 'Save', keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS], run: () => live.current.onSave?.() }),
      mod.addAction({
        id: 'wb.lh.revertLines',
        label: 'Revert Selected Lines to This Version',
        contextMenuGroupId: 'navigation',
        contextMenuOrder: 0,
        keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyMod.Alt | monaco.KeyCode.KeyZ],
        run: (e) => {
          const s = e.getSelection()
          if (!s || !live.current.onRevertLines) return
          const end = s.endColumn === 1 && s.endLineNumber > s.startLineNumber ? s.endLineNumber - 1 : s.endLineNumber
          live.current.onRevertLines(s.startLineNumber, end)
        },
      }),
    ]
    mod.onDidDispose(() => actions.forEach((a) => a.dispose()))
    setInst({ ed, monaco })
  }, [])

  return (
    <div className="wb-lh-editor">
      <MonacoDiffEditor
        theme={theme === 'dark' ? 'workbench-dark' : 'workbench-light'}
        options={options}
        keepCurrentOriginalModel
        keepCurrentModifiedModel
        onMount={onMount}
      />
    </div>
  )
}
