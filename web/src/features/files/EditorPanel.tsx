// The `editor` panel: Monaco on a shared buffer (one model per file), with save
// (Ctrl+S, etag-checked), external-change handling, git change markers and
// blame, Markdown/SVG preview, disk-vs-buffer compare, and viewers for media,
// binary, large and sensitive files. Markdown opens as a rendered page (Read),
// side by side with its source (Split), or as source only (Edit); Monaco stays
// mounted in Read mode so the buffer, dirty state and Ctrl+S carry over.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { flushSync } from 'react-dom'
import type { editor, IDisposable } from 'monaco-editor'
import {
  AlertTriangle,
  BookOpen,
  Bot,
  Clock,
  Code,
  Columns2,
  Crosshair,
  Eye,
  FileDown,
  FileX,
  GitCompare,
  History,
  Lock,
  MoreHorizontal,
  RotateCcw,
  Save,
  ScanSearch,
  UserRound,
} from 'lucide-react'
import { ApiError } from '@/api/client'
import { getDockApi, toast } from '@/shell/actions'
import type { PanelProps } from '@/shell/types'
import { useUi } from '@/state/store'
import { cssVar } from '@/theme/palette'
import { Button, copyText, EmptyState, ErrorBox, IconButton, Loading, MonacoDiffEditor, MonacoEditor, showMenu, showMenuAt, Splitter, timeAgo, type MenuEntry } from '@/ui'
import { filesApi, type FileContent, type GitBlame } from './api'
import {
  acquireBuffer,
  applyText,
  checkDisk,
  dismissRestored,
  keepMine,
  onBufferText,
  releaseBuffer,
  reloadBuffer,
  saveBuffer,
  useBuffers,
  type BufferState,
} from './buffers'
import { exportAsHtml } from './export/ExportDialog'
import { blameTime, blockAt, useBlame, useChangeMarkers, useGitBase } from './gutter'
import { showLocalHistory } from './history/open'
import { useVcsIndex } from './hooks'
import { rollbackBlock } from './lineDiff'
import { MarkdownView } from './MarkdownView'
import { attachBookmarks } from './bookmarkEditor'
import { useBookmarkPopup } from './BookmarkPopups'
import { useBookmarks } from './bookmarks'
import { navigateBack, navigateForward } from './navigation'
import { navHistory } from './navHistory'
import {
  askAboutFile,
  askAboutSelection,
  compareWithClipboard,
  compareWithHead,
  compareWithPicked,
  openFile,
  revealInTree,
  showCommit,
  showHistory,
  showSearch,
  type EditorParams,
  type MarkdownMode,
} from './openers'
import { basename, bufferKey, editorPanelId, hljsLanguage, isMarkdown, isSvg, mediaKind, segments, tabTitle, viewerFor } from './paths'
import { useActiveEditor } from './store'
import { vcsKindOf, VCS_LABEL } from './vcs'
import { BinaryNotice, ImageViewer, MediaViewer, PdfViewer, SensitiveNotice } from './viewers'

type Monaco = typeof import('monaco-editor')

export function EditorPanel(props: PanelProps<EditorParams>) {
  const { params, id, close } = props
  const projectId = params.projectId ?? null
  const path = params.path
  useDedupe(id, projectId, path, params, close)
  const [allowSensitive, setAllowSensitive] = useState(false)
  const [nonce, setNonce] = useState(0)
  const meta = useFileMeta(projectId, path, allowSensitive, nonce)

  useEffect(() => {
    if (!path) return
    props.setTitle(tabTitle(path))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path])

  if (!path) return <EmptyState title="No file" />
  if (meta.error) {
    const missing = meta.error instanceof ApiError && meta.error.status === 404
    return missing ? (
      <EmptyState icon={FileX} title="File not found" action={<Button size="small" onClick={close}>Close</Button>}>
        {path}
      </EmptyState>
    ) : (
      <ErrorBox error={meta.error} onRetry={() => setNonce((n) => n + 1)} />
    )
  }
  if (!meta.data) return <Loading label={`Opening ${basename(path)}…`} />
  const kind = viewerFor(path, meta.data)
  switch (kind) {
    case 'sensitive':
      return <SensitiveNotice path={path} onReveal={() => setAllowSensitive(true)} />
    case 'image':
      return <ImageViewer projectId={projectId} path={path} version={meta.data.mtime} />
    case 'video':
    case 'audio':
      return <MediaViewer projectId={projectId} path={path} version={meta.data.mtime} kind={kind} />
    case 'pdf':
      return <PdfViewer projectId={projectId} path={path} version={meta.data.mtime} />
    case 'binary':
    case 'tooLarge':
      return <BinaryNotice projectId={projectId} path={path} size={meta.data.size} tooLarge={kind === 'tooLarge'} />
    default:
      return <TextEditor key={bufferKey(projectId, path)} {...props} meta={meta.data} sensitive={allowSensitive} />
  }
}

/** Another slice may open an editor with its own id; converge on the conventional one. */
function useDedupe(id: string, projectId: string | null, path: string, params: EditorParams, close: () => void) {
  useEffect(() => {
    const conventional = editorPanelId(projectId, path)
    if (id === conventional || id.includes('#')) return
    const other = getDockApi()?.getPanel(conventional)
    if (other) {
      other.api.updateParameters({ ...params } as unknown as Record<string, unknown>)
      other.api.setActive()
      close()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
}

function useFileMeta(projectId: string | null, path: string, allowSensitive: boolean, nonce: number) {
  const [state, setState] = useState<{ data?: FileContent; error?: unknown }>({})
  useEffect(() => {
    let cancelled = false
    setState({})
    const media = mediaKind(path)
    const load: Promise<FileContent> = media
      ? filesApi.stat(projectId, path, allowSensitive).then((s) => {
          if (!s.exists) throw new ApiError(404, 'not_found', `${path} does not exist`)
          return { path, content: null, binary: true, size: s.size, mtime: s.mtime, etag: s.etag, tooLarge: false, sensitive: false, encoding: null, mime: '', readOnly: true }
        })
      : filesApi.read(projectId, path, allowSensitive)
    load.then(
      (data) => !cancelled && setState({ data }),
      (error) => !cancelled && setState({ error }),
    )
    return () => {
      cancelled = true
    }
  }, [projectId, path, allowSensitive, nonce])
  return state
}

// ---------------------------------------------------------------- text editor

const VIEW_STATE_PREFIX = 'wb.files.view.'

function TextEditor({
  id,
  params,
  setParams,
  setTitle,
  active,
  close,
  meta,
  sensitive,
}: PanelProps<EditorParams> & { meta: FileContent; sensitive: boolean }) {
  const projectId = params.projectId ?? null
  const path = params.path
  const key = bufferKey(projectId, path)
  const buf = useBuffers((s) => s.buffers[key])
  const theme = useUi((s) => s.prefs.theme)
  const fontSize = useUi((s) => s.prefs.editorFontSize)
  const vcs = useVcsIndex(projectId)
  const vcsKind = projectId ? vcsKindOf(vcs, path) : null

  const [model, setModel] = useState<editor.ITextModel | null>(null)
  const [ed, setEd] = useState<editor.IStandaloneCodeEditor | null>(null)
  const monacoRef = useRef<Monaco | null>(null)
  const [loadError, setLoadError] = useState<unknown>(null)
  const [cursor, setCursor] = useState({ line: 1, column: 1, selected: 0 })
  const md = isMarkdown(path)
  const svg = isSvg(path)
  const prefMode = useUi((s) => s.prefs.markdownMode ?? 'read')
  // Markdown: Read / Split / Edit. SVG: Split (preview shown) or Edit.
  const [mode, setMode] = useState<MarkdownMode>(() => (md ? (params.mode ?? (params.line ? 'edit' : prefMode)) : svg ? 'split' : 'edit'))
  const preview = mode !== 'edit'
  const reading = mode === 'read'
  const changeMode = (m: MarkdownMode) => {
    setMode(m)
    if (md) setParams({ ...params, mode: m })
  }
  const changeModeRef = useRef(changeMode)
  changeModeRef.current = changeMode
  const readingRef = useRef(reading)
  readingRef.current = reading
  // An explicit mode from an opener (a link, Edit Source, the preview panel's Edit).
  useEffect(() => {
    if (md && params.mode) setMode(params.mode)
  }, [md, params.mode])
  /** Preview pane width in px once dragged; half the editor until then. */
  const [previewWidth, setPreviewWidth] = useState<number | null>(null)
  const previewEl = useRef<HTMLDivElement>(null)
  const [blame, setBlame] = useState<GitBlame | null>(null)
  const [compare, setCompare] = useState<string | null>(null)

  // Attach the model and restore the last view state of this panel.
  useEffect(() => {
    if (!ed || !model) return
    ed.setModel(model)
    try {
      const saved = sessionStorage.getItem(VIEW_STATE_PREFIX + id)
      if (saved && !params.line) ed.restoreViewState(JSON.parse(saved))
    } catch {
      /* ignore */
    }
    const save = () => {
      try {
        const vs = ed.saveViewState()
        if (vs) sessionStorage.setItem(VIEW_STATE_PREFIX + id, JSON.stringify(vs))
      } catch {
        /* quota */
      }
    }
    window.addEventListener('pagehide', save)
    return () => {
      save()
      window.removeEventListener('pagehide', save)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ed, model, id])

  // Acquire the shared buffer for this file. Declared after the attach effect so that
  // on unmount the view state is saved before the model may be disposed.
  useEffect(() => {
    let cancelled = false
    let acquired = false
    acquireBuffer(projectId, path, meta, sensitive).then(
      (m) => {
        if (cancelled) releaseBuffer(key)
        else {
          acquired = true
          setModel(m)
        }
      },
      (e) => !cancelled && setLoadError(e),
    )
    return () => {
      cancelled = true
      if (acquired) releaseBuffer(key)
    }
    // meta/sensitive are the initial load only; the buffer follows the disk itself.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key])

  // Dirty marker in the tab title.
  useEffect(() => {
    setTitle((buf?.dirty ? '*' : '') + tabTitle(path))
  }, [buf?.dirty, path, setTitle])

  // Reveal params.line (search results, go to file :line, MCP open_file). A
  // navigation to a line of a file shown as a page switches to its source; the
  // line a page was restored with (a reloaded layout) counts as seen.
  const lineKey = params.line ? `${params.line}:${params.column ?? ''}:${params.endColumn ?? ''}:${params.t ?? ''}` : ''
  const revealed = useRef(reading ? lineKey : '')
  useEffect(() => {
    if (!ed || !model || !params.line || revealed.current === lineKey) return
    if (reading) {
      changeModeRef.current('edit')
      return
    }
    revealed.current = lineKey
    ed.layout()
    const line = Math.min(Math.max(1, params.line), model.getLineCount())
    const col = Math.max(1, params.column ?? 1)
    const endCol = params.endColumn && params.endColumn > col ? params.endColumn : col
    ed.setSelection({ startLineNumber: line, startColumn: col, endLineNumber: line, endColumn: endCol })
    ed.revealLineInCenter(line)
    const flash = ed.createDecorationsCollection([
      { range: { startLineNumber: line, startColumn: 1, endLineNumber: line, endColumn: 1 }, options: { isWholeLine: true, className: 'wb-line-flash' } },
    ])
    const t = window.setTimeout(() => flash.clear(), 1600)
    ed.focus()
    return () => {
      window.clearTimeout(t)
      flash.clear()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ed, model, lineKey, reading])

  // Focus when the tab is activated; register as the active editor.
  useEffect(() => {
    if (!active || !ed) return
    // A hidden editor must not take the keyboard (typing would edit unseen text).
    if (!reading) ed.focus()
    // Once the file's model is in (it attaches after the editor mounts).
    const pos = model && ed.getModel() === model ? ed.getPosition() : null
    if (pos) navHistory.record({ projectId, path, line: pos.lineNumber, column: pos.column }, Date.now())
    useActiveEditor.getState().set({
      panelId: id,
      projectId,
      path,
      selection: () => {
        const sel = ed.getSelection()
        const m = ed.getModel()
        return sel && m ? m.getValueInRange(sel) : ''
      },
      goto: (line, column) => {
        if (readingRef.current) {
          flushSync(() => changeModeRef.current('edit'))
          ed.layout()
        }
        ed.setPosition({ lineNumber: line, column: column ?? 1 })
        ed.revealLineInCenter(line)
        ed.focus()
      },
    })
  }, [active, ed, model, id, projectId, path, reading])
  useEffect(
    () => () => {
      if (useActiveEditor.getState().current?.panelId === id) useActiveEditor.getState().set(null)
    },
    [id],
  )

  // Git change markers and blame.
  const gitEnabled = !!projectId && vcsKind !== 'untracked' && vcsKind !== 'ignored' && vcsKind !== 'added'
  const base = useGitBase(projectId, path, gitEnabled, 0)
  const blocks = useChangeMarkers(ed, model, base)
  const baseRef = useRef(base)
  baseRef.current = base
  const previewStart = useRef(0)
  const blameInfo = useBlame(blame)
  const blameLine = blameInfo?.byLine.get(cursor.line) ?? null

  const toggleBlame = useCallback(async () => {
    if (blame) {
      setBlame(null)
      return
    }
    if (!projectId) return
    try {
      setBlame(await filesApi.gitBlame(projectId, path))
    } catch (e) {
      toast('warning', 'Blame is not available', { detail: e instanceof Error ? e.message : String(e) })
    }
  }, [blame, projectId, path])

  const openCompare = useCallback(async () => {
    try {
      const disk = await filesApi.read(projectId, path, sensitive)
      if (disk.content === null) {
        toast('warning', 'The disk version cannot be shown as text')
        return
      }
      setCompare(disk.content)
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) setCompare('')
      else toast('error', 'Could not read the disk version', { detail: String(e) })
    }
  }, [projectId, path, sensitive])

  // Keep a fresh handle to the latest state for Monaco actions registered once.
  const live = useRef({ id, projectId, path, blameInfo, toggleBlame, openCompare, key })
  live.current = { id, projectId, path, blameInfo, toggleBlame, openCompare, key }

  const readOnly = !!buf?.readOnly || meta.readOnly || !projectId
  const options = useMemo<editor.IStandaloneEditorConstructionOptions>(
    () => ({
      automaticLayout: true,
      minimap: { enabled: false },
      fontSize,
      fontFamily: cssVar('--font-mono', 'monospace'),
      lineNumbers: blameInfo ? blameInfo.render : 'on',
      lineNumbersMinChars: blameInfo ? blameInfo.minChars : 4,
      lineDecorationsWidth: 12,
      glyphMargin: true,
      scrollBeyondLastLine: false,
      renderWhitespace: 'selection',
      readOnly,
      readOnlyMessage: { value: projectId ? 'This file is read-only' : 'Files outside a project are read-only' },
      fixedOverflowWidgets: true,
      bracketPairColorization: { enabled: true },
      stickyScroll: { enabled: true },
      wordWrap: md ? 'on' : 'off',
      padding: { top: 4 },
      scrollbar: { verticalScrollbarSize: 10, horizontalScrollbarSize: 10 },
      unicodeHighlight: { ambiguousCharacters: false },
    }),
    [fontSize, blameInfo, readOnly, projectId, md],
  )

  const onMount = useCallback(
    (e: editor.IStandaloneCodeEditor, monaco: Monaco) => {
      monacoRef.current = monaco
      // The wrapper creates a throwaway in-memory model; ours (file://) is attached in an effect.
      const initial = e.getModel()
      setEd(e)
      if (initial) queueMicrotask(() => !initial.isDisposed() && initial.uri.scheme === 'inmemory' && initial.dispose())

      // Actions, not `addCommand`: a command's keybinding is global to every editor on
      // the page (the last one mounted wins) and can never be removed. An action's
      // keybinding only applies while its own editor has focus. Monaco does not tie
      // actions to the editor's lifetime, so they are disposed with it; otherwise the
      // global command registry keeps every closed editor alive.
      const actions: IDisposable[] = []
      const addAction = (a: editor.IActionDescriptor) => actions.push(e.addAction(a))
      e.onDidDispose(() => actions.splice(0).forEach((d) => d.dispose()))
      // Navigation history (Navigate Back / Forward, Recent Locations): the caret of
      // the active editor.
      e.onDidChangeCursorPosition((ev) => {
        const l = live.current
        if (useActiveEditor.getState().current?.panelId !== l.id) return
        navHistory.record({ projectId: l.projectId, path: l.path, line: ev.position.lineNumber, column: ev.position.column }, Date.now())
      })
      // Bookmarks: F11 toggles, Ctrl+F11 with a mnemonic, Shift+F11 lists them.
      actions.push(attachBookmarks(e, monaco, () => live.current))
      const caretLine = () => e.getPosition()?.lineNumber ?? 1
      addAction({
        id: 'wb.toggleBookmark',
        label: 'Toggle Bookmark',
        contextMenuGroupId: 'navigation',
        contextMenuOrder: 0.5,
        keybindings: [monaco.KeyCode.F11],
        run: () => void useBookmarks.getState().toggle(live.current.projectId, live.current.path, caretLine()),
      })
      addAction({
        id: 'wb.toggleBookmarkMnemonic',
        label: 'Toggle Bookmark with Mnemonic',
        keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyCode.F11],
        run: () => useBookmarkPopup.getState().choose({ projectId: live.current.projectId, path: live.current.path, line: caretLine() }),
      })
      addAction({
        id: 'wb.showBookmarks',
        label: 'Show Bookmarks',
        keybindings: [monaco.KeyMod.Shift | monaco.KeyCode.F11],
        run: () => useBookmarkPopup.getState().showList(),
      })
      addAction({
        id: 'wb.navigateBack',
        label: 'Navigate Back',
        // CLion's Ctrl+Alt+Left (desktops often take it for workspaces), and Alt+Left.
        keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyMod.Alt | monaco.KeyCode.LeftArrow, monaco.KeyMod.Alt | monaco.KeyCode.LeftArrow],
        run: () => navigateBack(),
      })
      addAction({
        id: 'wb.navigateForward',
        label: 'Navigate Forward',
        keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyMod.Alt | monaco.KeyCode.RightArrow, monaco.KeyMod.Alt | monaco.KeyCode.RightArrow],
        run: () => navigateForward(),
      })
      addAction({
        id: 'wb.save',
        label: 'Save',
        keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS],
        run: () => void saveBuffer(live.current.key),
      })
      const sel = () => {
        const s = e.getSelection()
        const m = e.getModel()
        if (!s || !m) return null
        const start = s.startLineNumber
        const end = s.endColumn === 1 && s.endLineNumber > s.startLineNumber ? s.endLineNumber - 1 : s.endLineNumber
        const text = s.isEmpty() ? m.getLineContent(start) : m.getValueInRange(s)
        return { start, end: s.isEmpty() ? start : end, text }
      }
      addAction({
        id: 'wb.askAgent',
        label: 'Ask Agent About Selection',
        contextMenuGroupId: 'navigation',
        contextMenuOrder: 0,
        keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyMod.Shift | monaco.KeyCode.KeyA],
        run: () => {
          const s = sel()
          if (s) askAboutSelection(live.current.projectId, live.current.path, s.start, s.end, s.text, hljsLanguage(live.current.path))
        },
      })
      addAction({
        id: 'wb.compareClipboard',
        label: 'Compare with Clipboard',
        contextMenuGroupId: '9_cutcopypaste',
        contextMenuOrder: 11,
        run: () => void compareWithClipboard(live.current.projectId, live.current.path),
      })
      addAction({
        id: 'wb.compareWith',
        label: 'Compare With…',
        run: () => {
          const l = live.current
          if (l.projectId) compareWithPicked(l.projectId, l.path)
        },
      })
      // CLion's Column Selection Mode: selections are rectangles while it is on.
      addAction({
        id: 'wb.columnSelection',
        label: 'Toggle Column Selection Mode',
        keybindings: [monaco.KeyMod.Alt | monaco.KeyMod.Shift | monaco.KeyCode.Insert],
        run: () => {
          const on = !e.getOption(monaco.editor.EditorOption.columnSelection)
          e.updateOptions({ columnSelection: on })
          toast('info', `Column selection mode ${on ? 'on' : 'off'}`, { timeout: 1500 })
        },
      })
      addAction({
        id: 'wb.copyPathLine',
        label: 'Copy Path:Line',
        contextMenuGroupId: '9_cutcopypaste',
        contextMenuOrder: 10,
        run: () => {
          const s = sel()
          if (!s) return
          const ref = `${live.current.path}:${s.start === s.end ? s.start : `${s.start}-${s.end}`}`
          void copyText(ref).then((ok) => toast(ok ? 'success' : 'error', ok ? `Copied ${ref}` : 'Could not copy', { timeout: 2000 }))
        },
      })
      addAction({
        id: 'wb.openSide',
        label: 'Open to the Side',
        contextMenuGroupId: 'z_workbench',
        contextMenuOrder: 1,
        run: () => openFile({ projectId: live.current.projectId, path: live.current.path, side: true, line: e.getPosition()?.lineNumber }),
      })
      addAction({
        id: 'wb.reveal',
        label: 'Reveal in Project Tree',
        contextMenuGroupId: 'z_workbench',
        contextMenuOrder: 2,
        run: () => revealInTree(live.current.projectId, live.current.path),
      })
      addAction({
        id: 'wb.findInFiles',
        label: 'Find Selection in Files',
        contextMenuGroupId: 'z_workbench',
        contextMenuOrder: 3,
        run: () => {
          const s = e.getSelection()
          const m = e.getModel()
          const text = s && m && !s.isEmpty() && s.startLineNumber === s.endLineNumber ? m.getValueInRange(s) : undefined
          showSearch(live.current.projectId, text)
        },
      })
      addAction({
        id: 'wb.blame',
        label: 'Toggle Blame (Annotate)',
        contextMenuGroupId: 'z_workbench',
        contextMenuOrder: 4,
        run: () => void live.current.toggleBlame(),
      })
      e.onDidChangeCursorSelection((ev) => {
        const s = ev.selection
        const m = e.getModel()
        const selected = m && !s.isEmpty() ? m.getValueLengthInRange(s) : 0
        setCursor({ line: s.positionLineNumber, column: s.positionColumn, selected })
      })
      // Gutter: click a change marker for Rollback / diff; click a blame annotation for its commit.
      e.onMouseDown((ev) => {
        const t = ev.target
        const line = t.position?.lineNumber
        if (!line) return
        if (t.type === monaco.editor.MouseTargetType.GUTTER_LINE_DECORATIONS) {
          const b = blockAt(blocks.current, line)
          if (!b) return
          const items: MenuEntry[] = [
            {
              label: b.kind === 'added' ? 'Rollback (remove added lines)' : 'Rollback Lines',
              icon: RotateCcw,
              run: () => {
                const m = e.getModel()
                const baseLines = baseRef.current
                if (!m || !baseLines) return
                applyText(m, rollbackBlock(m.getLinesContent(), baseLines, b).join(m.getEOL()))
              },
            },
            { label: 'Show Diff', icon: GitCompare, run: () => live.current.projectId && compareWithHead(live.current.projectId, live.current.path) },
          ]
          showMenu({ clientX: ev.event.posx, clientY: ev.event.posy }, items)
        } else if (t.type === monaco.editor.MouseTargetType.GUTTER_LINE_NUMBERS && live.current.blameInfo) {
          const b = live.current.blameInfo.byLine.get(line)
          if (b && live.current.projectId && !/^0+$/.test(b.sha)) showCommit(live.current.projectId, b.sha)
        }
      })
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  )
  // Live text for the preview pane.
  const [previewText, setPreviewText] = useState('')
  useEffect(() => {
    if (!preview || !model) return
    setPreviewText(model.getValue())
    return onBufferText(key, setPreviewText)
  }, [preview, model, key])

  // Re-check the disk when the tab becomes visible again (cheap stat).
  useEffect(() => {
    if (active) void checkDisk(key)
  }, [active, key])

  if (loadError) return <ErrorBox error={loadError} />

  const eol = model && !model.isDisposed() ? (model.getEOL() === '\r\n' ? 'CRLF' : 'LF') : ''
  const language = model && !model.isDisposed() ? languageName(monacoRef.current, model.getLanguageId()) : ''

  const moreMenu = (el: HTMLElement) => {
    const items: MenuEntry[] = [
      { label: 'Save', icon: Save, shortcut: 'Ctrl+S', disabled: readOnly, run: () => void saveBuffer(key) },
      { label: 'Reload from Disk', icon: RotateCcw, run: () => void reloadBuffer(key) },
      { label: 'Compare with Disk', icon: Columns2, disabled: !projectId, run: () => void openCompare() },
      'separator',
      { label: 'Open to the Side', icon: Columns2, run: () => openFile({ projectId, path, side: true }) },
      { label: 'Reveal in Project Tree', icon: Crosshair, shortcut: 'Alt+F1', disabled: !projectId, run: () => revealInTree(projectId, path) },
      { label: 'Find in Files…', icon: ScanSearch, disabled: !projectId, run: () => showSearch(projectId) },
      'separator',
      { label: 'Show History', icon: History, disabled: !projectId, run: () => projectId && showHistory(projectId, path) },
      { label: 'Show Local History', icon: Clock, disabled: !projectId, run: () => projectId && showLocalHistory(projectId, path) },
      { label: 'Compare with HEAD', icon: GitCompare, disabled: !projectId, run: () => projectId && compareWithHead(projectId, path) },
      { label: blame ? 'Hide Blame' : 'Blame (Annotate)', icon: UserRound, disabled: !projectId, run: () => void toggleBlame() },
      'separator',
      ...(md ? [{ label: 'Export as HTML…', icon: FileDown, run: () => exportAsHtml(projectId, path) }, 'separator' as const] : []),
      { label: 'Ask Agent About This File', icon: Bot, run: () => askAboutFile(projectId, path) },
    ]
    showMenuAt(el, items)
  }

  return (
    <div className="wb-fill wb-editor">
      <div className="wb-editor-bar">
        <Breadcrumb projectId={projectId} path={path} />
        <span style={{ flex: 1 }} />
        {md && (
          <span className="wb-editor-modes" role="group" aria-label="Markdown view">
            <IconButton icon={BookOpen} size="small" label="Read (rendered page)" active={mode === 'read'} onClick={() => changeMode('read')} />
            <IconButton icon={Columns2} size="small" label="Split (source and page)" active={mode === 'split'} onClick={() => changeMode('split')} />
            <IconButton icon={Code} size="small" label="Edit source" active={mode === 'edit'} onClick={() => changeMode('edit')} />
          </span>
        )}
        {svg && <IconButton icon={Eye} size="small" label={preview ? 'Hide preview' : 'Show preview'} active={preview} onClick={() => changeMode(preview ? 'edit' : 'split')} />}
        {projectId && <IconButton icon={UserRound} size="small" label={blame ? 'Hide blame' : 'Blame (annotate)'} active={!!blame} onClick={() => void toggleBlame()} />}
        {projectId && <IconButton icon={GitCompare} size="small" label="Compare with HEAD" onClick={() => compareWithHead(projectId, path)} />}
        <IconButton icon={MoreHorizontal} size="small" label="More" onClick={(e) => moreMenu(e.currentTarget)} />
      </div>
      {buf && <Banners buf={buf} readOnly={readOnly} projectId={projectId} sensitive={sensitive} encoding={meta.encoding} onCompare={() => void openCompare()} onClose={close} />}
      <div className={reading ? 'wb-editor-body reading' : 'wb-editor-body'}>
        <div className="wb-editor-main">
          <MonacoEditor theme={theme === 'dark' ? 'workbench-dark' : 'workbench-light'} options={options} onMount={onMount} keepCurrentModel loading={<Loading label="Loading editor…" />} />
          {compare !== null && model && (
            <CompareOverlay
              model={model}
              disk={compare}
              theme={theme}
              fontSize={fontSize}
              readOnly={readOnly}
              onClose={() => setCompare(null)}
              onSave={() => void saveBuffer(key)}
              onUseDisk={async () => {
                await reloadBuffer(key)
                setCompare(null)
              }}
              onOverwrite={async () => {
                if (await saveBuffer(key, { force: true })) setCompare(null)
              }}
            />
          )}
        </div>
        {preview && (
          <>
            {!reading && (
              <Splitter
                direction="v"
                onResizeStart={() => (previewStart.current = previewEl.current?.offsetWidth ?? 420)}
                onResize={(d) => setPreviewWidth(Math.max(200, Math.min(1400, previewStart.current - d)))}
              />
            )}
            <div ref={previewEl} className="wb-editor-preview" style={reading ? undefined : { width: previewWidth ?? '50%' }}>
              {md ? (
                <MarkdownView projectId={projectId} path={path} text={previewText} page={reading} />
              ) : (
                <div className="wb-svg-preview">
                  <img alt="" src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(previewText)}`} />
                </div>
              )}
            </div>
          </>
        )}
      </div>
      <div className="wb-editor-footer">
        {vcsKind && <span className={`wb-editor-footer-item wb-vcs-${vcsKind}`}>{VCS_LABEL[vcsKind]}</span>}
        {blameLine && projectId && (
          <button className="wb-editor-footer-item link wb-ellipsis" title={blameLine.summary} onClick={() => showCommit(projectId, blameLine.sha)}>
            {blameLine.sha.slice(0, 8)} · {blameLine.author} · {timeAgo(blameTime(blameLine.time))} · {blameLine.summary}
          </button>
        )}
        <span style={{ flex: 1 }} />
        {buf?.saving && <span className="wb-editor-footer-item">Saving…</span>}
        {readOnly && (
          <span className="wb-editor-footer-item" title="Read-only">
            <Lock size={11} /> Read-only
          </span>
        )}
        {!reading && (
          <span className="wb-editor-footer-item">
            {cursor.line}:{cursor.column}
            {cursor.selected ? ` (${cursor.selected} chars)` : ''}
          </span>
        )}
        <span className="wb-editor-footer-item">{eol}</span>
        <span className="wb-editor-footer-item">{meta.encoding === 'utf-8-bom' ? 'UTF-8 BOM' : meta.encoding === 'unknown' ? 'Unknown encoding' : 'UTF-8'}</span>
        <span className="wb-editor-footer-item">{language}</span>
      </div>
    </div>
  )
}

function languageName(monaco: Monaco | null, id: string): string {
  const lang = monaco?.languages.getLanguages().find((l) => l.id === id)
  return lang?.aliases?.[0] ?? id
}

function Breadcrumb({ projectId, path }: { projectId: string | null; path: string }) {
  const parts = segments(path)
  return (
    <div className="wb-editor-crumbs wb-ellipsis" title={path}>
      {parts.map((p, i) => {
        const sub = (path.startsWith('/') ? '/' : '') + parts.slice(0, i + 1).join('/')
        return (
          <span key={i}>
            {i > 0 && <span className="sep">›</span>}
            <button className={i === parts.length - 1 ? 'crumb last' : 'crumb'} onClick={() => projectId && revealInTree(projectId, sub)}>
              {p}
            </button>
          </span>
        )
      })}
    </div>
  )
}

function Banners({
  buf,
  readOnly,
  projectId,
  sensitive,
  encoding,
  onCompare,
  onClose,
}: {
  buf: BufferState
  readOnly: boolean
  projectId: string | null
  sensitive: boolean
  encoding: FileContent['encoding']
  onCompare: () => void
  onClose: () => void
}) {
  const c = buf.conflict
  return (
    <>
      {c?.kind === 'changed' && (
        <div className="wb-banner warning">
          <AlertTriangle size={14} />
          <span className="wb-grow">The file changed on disk, and you have unsaved changes.</span>
          <Button size="small" onClick={() => void reloadBuffer(buf.key)}>Reload from Disk</Button>
          <Button size="small" onClick={() => keepMine(buf.key)}>Keep Mine</Button>
          <Button size="small" onClick={onCompare}>Compare</Button>
        </div>
      )}
      {c?.kind === 'save' && (
        <div className="wb-banner danger">
          <AlertTriangle size={14} />
          <span className="wb-grow">Not saved: the file changed on disk since it was loaded.</span>
          <Button size="small" onClick={onCompare}>Compare</Button>
          <Button size="small" onClick={() => void saveBuffer(buf.key, { force: true })}>Overwrite</Button>
          <Button size="small" onClick={() => void reloadBuffer(buf.key)}>Discard Mine</Button>
        </div>
      )}
      {c?.kind === 'deleted' && (
        <div className="wb-banner warning">
          <FileX size={14} />
          <span className="wb-grow">The file was deleted or moved on disk.</span>
          {!readOnly && <Button size="small" onClick={() => keepMine(buf.key)}>Keep (Save Recreates It)</Button>}
          <Button size="small" onClick={onClose}>Close</Button>
        </div>
      )}
      {buf.restoredDraft && !c && (
        <div className="wb-banner info">
          <RotateCcw size={14} />
          <span className="wb-grow">Restored unsaved changes from your last session.</span>
          <Button size="small" onClick={() => dismissRestored(buf.key)}>Keep</Button>
          <Button size="small" onClick={() => void reloadBuffer(buf.key)}>Discard</Button>
        </div>
      )}
      {sensitive && (
        <div className="wb-banner danger subtle">
          <Lock size={14} />
          <span className="wb-grow">Sensitive file: mind shared screens and screenshots.</span>
        </div>
      )}
      {!projectId && (
        <div className="wb-banner info subtle">
          <Lock size={14} />
          <span className="wb-grow">Outside the project: opened read-only.</span>
        </div>
      )}
      {encoding === 'unknown' && projectId && (
        <div className="wb-banner warning subtle">
          <AlertTriangle size={14} />
          <span className="wb-grow">Not valid UTF-8: shown read-only so saving cannot corrupt it.</span>
        </div>
      )}
    </>
  )
}

function CompareOverlay({
  model,
  disk,
  theme,
  fontSize,
  readOnly,
  onClose,
  onSave,
  onUseDisk,
  onOverwrite,
}: {
  model: editor.ITextModel
  disk: string
  theme: string
  fontSize: number
  readOnly: boolean
  onClose: () => void
  onSave: () => void
  onUseDisk: () => void
  onOverwrite: () => void
}) {
  const original = useRef<editor.ITextModel | null>(null)
  useEffect(
    () => () => {
      const m = original.current
      // After the diff widget itself is gone.
      window.setTimeout(() => m?.dispose(), 0)
    },
    [],
  )
  const options = useMemo<editor.IDiffEditorConstructionOptions>(
    () => ({ automaticLayout: true, renderSideBySide: true, originalEditable: false, readOnly, minimap: { enabled: false }, fontSize, scrollBeyondLastLine: false }),
    [readOnly, fontSize],
  )
  return (
    <div className="wb-compare-overlay">
      <div className="wb-banner info">
        <Columns2 size={14} />
        <span className="wb-grow">Disk version (left) ↔ your version (right, editable)</span>
        <Button size="small" onClick={onUseDisk}>Use Disk Version</Button>
        {!readOnly && <Button size="small" variant="primary" onClick={onOverwrite}>Save Mine</Button>}
        <Button size="small" onClick={onClose}>Close</Button>
      </div>
      <div className="wb-grow" style={{ minHeight: 0 }}>
        <MonacoDiffEditor
          theme={theme === 'dark' ? 'workbench-dark' : 'workbench-light'}
          options={options}
          keepCurrentModifiedModel
          keepCurrentOriginalModel
          onMount={(diff, monaco) => {
            const initial = diff.getModel()
            const orig = monaco.editor.createModel(disk, model.getLanguageId())
            original.current = orig
            diff.setModel({ original: orig, modified: model })
            // Ctrl+S in the editable side: the usual etag-checked save (Save Mine forces).
            const mod = diff.getModifiedEditor()
            const save = mod.addAction({ id: 'wb.save', label: 'Save', keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS], run: () => onSave() })
            mod.onDidDispose(() => save.dispose())
            queueMicrotask(() => {
              initial?.original.dispose()
              if (initial?.modified !== model) initial?.modified.dispose()
            })
          }}
        />
      </div>
    </div>
  )
}
