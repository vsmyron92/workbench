// The `compare` panel: a project file (right, its editor buffer, editable) against
// another file (left, its buffer, editable too) or a text (the clipboard, read-only).
// CLion's Compare With… and Compare with Clipboard. Ctrl+S saves the side it is in.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { editor } from 'monaco-editor'
import { ArrowLeftRight, ClipboardPaste, Save } from 'lucide-react'
import type { PanelProps } from '@/shell/types'
import { useUi } from '@/state/store'
import { Button, EmptyState, ErrorBox, IconButton, Loading, MonacoDiffEditor } from '@/ui'
import { bufferKey, useBuffers } from './buffers'
import { saveFromHistory, useFileBuffer } from './history/LocalHistoryPanel'
import { basename } from './paths'

type Monaco = typeof import('monaco-editor')

export interface CompareParams {
  projectId: string
  /** The file on the right. */
  path: string
  /** The other side: a file of the same project, or a text kept in this browser tab. */
  left: { path: string } | { textKey: string; label: string }
}

/** Texts to compare against (the clipboard), by key: too big for persisted panel params. */
const texts = new Map<string, string>()

export function rememberCompareText(text: string): string {
  const key = `t${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`
  texts.set(key, text)
  return key
}

export function comparePanelId(p: CompareParams): string {
  return `compare:${p.projectId}:${p.path}:${'path' in p.left ? p.left.path : p.left.textKey}`
}

export function ComparePanel({ params, setTitle, setParams }: PanelProps<CompareParams>) {
  const { projectId, path, left } = params
  const leftPath = 'path' in left ? left.path : null
  const leftText = 'textKey' in left ? (texts.get(left.textKey) ?? null) : null
  const right = useFileBuffer(projectId, path, true)
  const other = useFileBuffer(projectId, leftPath ?? '', leftPath !== null)
  const leftLabel = leftPath ?? ('label' in left ? left.label : '')

  useEffect(() => setTitle(`${leftPath ? basename(leftPath) : leftLabel} ↔ ${basename(path)}`), [leftPath, leftLabel, path, setTitle])

  if ('textKey' in left && leftText === null) {
    return (
      <EmptyState icon={ClipboardPaste} title="The compared text is gone">
        Texts from the clipboard are kept only while this browser tab is open.
      </EmptyState>
    )
  }
  const error = right.error ?? (leftPath !== null ? other.error : null)
  if (error) return <ErrorBox error={error} onRetry={() => (right.error ? right.reload() : other.reload())} />
  if (right.missing || (leftPath !== null && other.missing)) {
    return <EmptyState title="File not found">{right.missing ? path : leftPath}</EmptyState>
  }
  if (!right.model || (leftPath !== null && !other.model)) return <Loading label="Opening the files…" />

  const swap = leftPath !== null ? () => setParams({ projectId, path: leftPath, left: { path } }) : undefined
  return (
    <div className="wb-fill wb-compare">
      <div className="wb-compare-bar">
        <span className="side wb-ellipsis" title={leftLabel}>
          {leftLabel}
          {leftPath === null && <span className="wb-subtle"> · read-only</span>}
          <DirtyMark projectId={projectId} path={leftPath} />
        </span>
        {swap && <IconButton icon={ArrowLeftRight} size="small" label="Swap sides" onClick={swap} />}
        <span className="side wb-ellipsis" title={path}>
          {path}
          <DirtyMark projectId={projectId} path={path} />
        </span>
      </div>
      <CompareDiff projectId={projectId} leftPath={leftPath} left={other.model ?? leftText!} right={right.model} rightPath={path} rightReadOnly={right.readOnly} leftReadOnly={leftPath === null || other.readOnly} />
    </div>
  )
}

function DirtyMark({ projectId, path }: { projectId: string; path: string | null }) {
  const dirty = useBuffers((s) => (path ? (s.buffers[bufferKey(projectId, path)]?.dirty ?? false) : false))
  if (!dirty || !path) return null
  return (
    <Button size="small" icon={Save} onClick={() => void saveFromHistory(bufferKey(projectId, path), path)}>
      Save
    </Button>
  )
}

function CompareDiff({
  projectId,
  leftPath,
  left,
  right,
  rightPath,
  leftReadOnly,
  rightReadOnly,
}: {
  projectId: string
  leftPath: string | null
  left: editor.ITextModel | string
  right: editor.ITextModel
  rightPath: string
  leftReadOnly: boolean
  rightReadOnly: boolean
}) {
  const theme = useUi((s) => s.prefs.theme)
  const fontSize = useUi((s) => s.prefs.editorFontSize)
  const made = useRef<editor.ITextModel | null>(null)
  const live = useRef({ projectId, leftPath, rightPath })
  live.current = { projectId, leftPath, rightPath }
  const [inst, setInst] = useState<{ ed: editor.IStandaloneDiffEditor; monaco: Monaco } | null>(null)
  const options = useMemo<editor.IDiffEditorConstructionOptions>(
    () => ({ automaticLayout: true, renderSideBySide: true, originalEditable: !leftReadOnly, readOnly: rightReadOnly, minimap: { enabled: false }, fontSize, scrollBeyondLastLine: false }),
    [leftReadOnly, rightReadOnly, fontSize],
  )
  useEffect(() => {
    if (!inst) return
    const { ed, monaco } = inst
    const initial = ed.getModel()
    let original: editor.ITextModel
    if (typeof left === 'string') {
      original = monaco.editor.createModel(left, right.getLanguageId())
      made.current = original
    } else original = left
    ed.setModel({ original, modified: right })
    queueMicrotask(() => {
      if (initial?.original !== original && initial?.original !== right) initial?.original.dispose()
      if (initial?.modified !== right && initial?.modified !== original) initial?.modified.dispose()
    })
    return () => {
      const m = made.current
      made.current = null
      // After the widget let go of it.
      if (m) window.setTimeout(() => m.dispose(), 0)
    }
  }, [inst, left, right])
  useEffect(
    () => () => {
      const m = made.current
      if (m) window.setTimeout(() => m.dispose(), 0)
    },
    [],
  )
  const onMount = useCallback((ed: editor.IStandaloneDiffEditor, monaco: Monaco) => {
    // Ctrl+S saves the side it is pressed in.
    const save = (side: 'left' | 'right') => {
      const l = live.current
      const path = side === 'left' ? l.leftPath : l.rightPath
      if (path) void saveFromHistory(bufferKey(l.projectId, path), path)
    }
    const keys = [monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS]
    const a = ed.getModifiedEditor().addAction({ id: 'wb.save', label: 'Save', keybindings: keys, run: () => save('right') })
    const b = ed.getOriginalEditor().addAction({ id: 'wb.save', label: 'Save', keybindings: keys, run: () => save('left') })
    ed.getModifiedEditor().onDidDispose(() => {
      a.dispose()
      b.dispose()
    })
    setInst({ ed, monaco })
  }, [])
  return (
    <div className="wb-compare-editor">
      <MonacoDiffEditor theme={theme === 'dark' ? 'workbench-dark' : 'workbench-light'} options={options} keepCurrentOriginalModel keepCurrentModifiedModel onMount={onMount} />
    </div>
  )
}
