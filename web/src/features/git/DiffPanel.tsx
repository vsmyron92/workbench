// The 'diff' panel: Monaco side-by-side / inline diff with CLion-style change
// navigation, per-hunk and per-line Stage / Unstage / Rollback (guarded by the
// server's diff fingerprint, so a file another agent just edited is never
// mis-staged), and line checkboxes: for staging, or, in the HEAD → working tree
// diff of the changelist view, for what the next commit includes (partial commit).

import { useEffect, useMemo, useRef, useState, type ComponentProps } from 'react'
import type { DiffOnMount } from '@monaco-editor/react'
import {
  ChevronDown,
  ChevronUp,
  Columns2,
  FileCode,
  FileWarning,
  FolderGit2,
  FoldVertical,
  GitCommitHorizontal,
  GitCompareArrows,
  GitMerge,
  History,
  ListChecks,
  Minus,
  Plus,
  RefreshCw,
  Rows2,
  Undo2,
  WrapText,
} from 'lucide-react'
import { ApiError } from '@/api/client'
import { confirmDialog, toast, toastError } from '@/shell/actions'
import type { PanelProps } from '@/shell/types'
import { useUi } from '@/state/store'
import { Button, Checkbox, EmptyState, ErrorBox, IconButton, Loading, MonacoDiffEditor, Toolbar } from '@/ui'
import { gitApi, useFileDiff, useGitStatus } from './api'
import { applyLines, compareWithBranch, openCommit, openConflict, openDiff, openFile, openGitLog, rollback, stagePaths, unstagePaths } from './actions'
import { useCommitSelection } from './Changelists'
import { countLabel, hunkKeys, lineKey, linesInRanges, type LineRange } from './lineSelection'
import { hunkAtLine, shortSha, splitPath } from './logic'
import { useInclusion } from './store'
import type { DiffLine, DiffPanelParams, GitFileDiff, GitHunk } from './types'

type DiffEditorT = Parameters<DiffOnMount>[0]
type CodeEditorT = ReturnType<DiffEditorT['getModifiedEditor']>
type Decorations = ReturnType<CodeEditorT['createDecorationsCollection']>
type Model = { dispose(): void; isDisposed(): boolean }

/**
 * The diff editor, with its models disposed only after the widget itself.
 * @monaco-editor/react disposes the models first when it unmounts, and Monaco
 * throws "TextModel got disposed before DiffEditorWidget model got reset" (e.g.
 * when staging the last hunk turns the panel into "No changes"). So the models
 * are kept (keepCurrent*Model) and disposed right after this editor unmounts;
 * a later remount then creates fresh ones instead of reusing stale text.
 */
function GitDiffEditor({ onMount, ...props }: ComponentProps<typeof MonacoDiffEditor>) {
  const editor = useRef<DiffEditorT | null>(null)
  const models = useRef(new Set<Model>())
  const track = () => {
    const m = editor.current?.getModel()
    if (m) {
      models.current.add(m.original)
      models.current.add(m.modified)
    }
  }
  useEffect(track)
  useEffect(() => {
    const tracked = models.current
    return () => {
      window.setTimeout(() => tracked.forEach((m) => m.isDisposed() || m.dispose()), 0)
    }
  }, [])
  return (
    <MonacoDiffEditor
      {...props}
      keepCurrentOriginalModel
      keepCurrentModifiedModel
      onMount={(ed, monaco) => {
        editor.current = ed
        track()
        onMount?.(ed, monaco)
      }}
    />
  )
}

function useLocalFlag(k: string, dflt: boolean): [boolean, (v: boolean) => void] {
  const [v, setV] = useState(() => {
    try {
      const s = localStorage.getItem(k)
      return s === null ? dflt : s === '1'
    } catch {
      return dflt
    }
  })
  return [
    v,
    (nv) => {
      setV(nv)
      try {
        localStorage.setItem(k, nv ? '1' : '0')
      } catch {
        /* private mode */
      }
    },
  ]
}

/** Selected line ranges of an editor; a caret counts only with `caret`. A selection ending at column 1 stops on the line above. */
function rangesOf(ed: CodeEditorT | undefined, caret: boolean): LineRange[] {
  return (ed?.getSelections() ?? []).flatMap((s) => {
    if (s.isEmpty() && !caret) return []
    const end = s.endLineNumber > s.startLineNumber && s.endColumn === 1 ? s.endLineNumber - 1 : s.endLineNumber
    return [{ start: s.startLineNumber, end }]
  })
}

export function DiffPanel({ id, params }: PanelProps<DiffPanelParams>) {
  if (!params?.projectId || !params.path || !params.mode) return <EmptyState icon={FileWarning} title="This diff has no file" />
  return <DiffView key={`${params.projectId}:${params.mode}:${params.path}:${params.sha ?? ''}:${params.base ?? ''}:${params.head ?? ''}`} panelId={id} p={params} />
}

/**
 * Line checkboxes of a HEAD → working tree diff: what the changelist view's next
 * commit includes of this file (all lines of an included file, none of an excluded
 * one, or a partial selection kept with the diff's fingerprint).
 */
function useIncludedLines(p: DiffPanelParams, d: GitFileDiff | undefined, enabled: boolean) {
  const status = useGitStatus(enabled ? p.projectId : null)
  const { included, partialSel } = useCommitSelection(p.projectId, enabled ? status.data : undefined, enabled)
  const all = useMemo(() => (d?.lines ?? []).map(lineKey), [d])
  const part = partialSel[p.path]
  const stale = !!part && !!d && part.fingerprint !== d.fingerprint
  const keys = useMemo(() => {
    if (part && !stale) return new Set(part.keys)
    return new Set(included.has(p.path) ? all : [])
  }, [part, stale, included, p.path, all])
  // Lines picked in an older version of the diff may point elsewhere now: start over.
  useEffect(() => {
    if (enabled && stale) {
      useInclusion.getState().setPartial(p.projectId, p.path, null)
      toast('info', `${splitPath(p.path).name} changed: its line selection was reset`)
    }
  }, [enabled, stale, p.projectId, p.path])
  const set = (next: Set<string>) => {
    if (!d) return
    const inc = useInclusion.getState()
    const files = new Set(included)
    if (next.size === 0) files.delete(p.path)
    else files.add(p.path)
    inc.setIncluded(p.projectId, [...files])
    inc.setPartial(p.projectId, p.path, next.size && next.size < all.length ? { fingerprint: d.fingerprint, keys: [...next] } : null)
  }
  return { keys, set, total: all.length }
}

function DiffView({ panelId, p }: { panelId: string; p: DiffPanelParams }) {
  const q = useFileDiff(p)
  const d = q.data
  const status = useGitStatus(p.mode === 'working' || p.mode === 'staged' ? p.projectId : null)
  const theme = useUi((s) => s.prefs.theme)
  const fontSize = useUi((s) => s.prefs.editorFontSize)
  const [sideBySide, setSideBySide] = useLocalFlag('wb.git.diff.sideBySide', true)
  const [ignoreWs, setIgnoreWs] = useLocalFlag('wb.git.diff.ignoreWs', false)
  const [collapse, setCollapse] = useLocalFlag('wb.git.diff.collapse', false)
  const [wrap, setWrap] = useLocalFlag('wb.git.diff.wrap', false)
  const [checkMode, setCheckMode] = useLocalFlag('wb.git.diff.lineChecks', false)
  const [lang, setLang] = useState('plaintext')
  const [cur, setCur] = useState(0)
  const [busy, setBusy] = useState(false)
  const [checked, setChecked] = useState<Set<string>>(new Set())
  const [selKeys, setSelKeys] = useState<string[]>([])
  // Bumped when Monaco mounts: decorations set before that have nowhere to go.
  const [mounted, setMounted] = useState(0)
  const editorRef = useRef<DiffEditorT | null>(null)
  const decoMod = useRef<Decorations | null>(null)
  const decoOrig = useRef<Decorations | null>(null)
  const boxMod = useRef<Decorations | null>(null)
  const boxOrig = useRef<Decorations | null>(null)
  const hunksRef = useRef<GitHunk[]>([])
  const linesRef = useRef<DiffLine[]>([])
  const revealedFor = useRef('')
  const hunks = useMemo(() => d?.hunks ?? [], [d])
  const lines = useMemo(() => (d?.canSelectLines ? (d.lines ?? []) : []), [d])
  hunksRef.current = hunks
  linesRef.current = lines

  // What line selection does here: stage (working / staged diffs) or include in the commit (HEAD → working tree).
  const headCompare = p.mode === 'compare' && p.base === 'HEAD' && !p.head
  const lineMode: 'stage' | 'include' | null = !lines.length ? null : p.mode === 'working' || p.mode === 'staged' ? 'stage' : headCompare ? 'include' : null
  const incl = useIncludedLines(p, d, lineMode === 'include')
  const showChecks = lineMode === 'include' || (lineMode === 'stage' && checkMode)
  const checkedKeys = lineMode === 'include' ? incl.keys : checked
  const setCheckedKeys = (next: Set<string>) => (lineMode === 'include' ? incl.set(next) : setChecked(next))
  const toggleRef = useRef<(keys: string[]) => void>(() => {})
  toggleRef.current = (keys) => {
    const next = new Set(checkedKeys)
    if (keys.every((k) => next.has(k))) keys.forEach((k) => next.delete(k))
    else keys.forEach((k) => next.add(k))
    setCheckedKeys(next)
  }
  const showChecksRef = useRef(showChecks)
  showChecksRef.current = showChecks

  useEffect(() => {
    let alive = true
    void import('@/lib/monacoSetup').then((m) => alive && setLang(m.languageFor(p.path)))
    return () => {
      alive = false
    }
  }, [p.path])

  // Lines move when the diff changes: a staging selection starts over.
  useEffect(() => {
    setChecked(new Set())
    setSelKeys([])
  }, [d?.fingerprint])

  const statusEntry = status.data?.files.find((f) => f.path === p.path)
  const hasStaged = !!statusEntry && statusEntry.index !== ' ' && statusEntry.index !== '?' && !statusEntry.conflict
  const hasUnstaged = !!statusEntry && statusEntry.worktree !== ' ' && !statusEntry.conflict

  const reveal = (i: number) => {
    const ed = editorRef.current
    const h = hunksRef.current[i]
    if (!ed || !h) return
    const mod = ed.getModifiedEditor()
    const line = Math.max(1, h.newStart)
    mod.revealLineInCenterIfOutsideViewport(line)
    mod.setPosition({ lineNumber: line, column: 1 })
  }

  const goTo = (i: number) => {
    const n = hunksRef.current.length
    if (!n) return
    const idx = ((i % n) + n) % n
    setCur(idx)
    reveal(idx)
  }

  // Keep the current hunk valid when the diff changes, and jump to it once per version.
  useEffect(() => {
    if (!d) return
    const idx = Math.max(0, Math.min(cur, hunks.length - 1))
    if (idx !== cur) setCur(idx)
    if (revealedFor.current !== d.fingerprint && editorRef.current) {
      revealedFor.current = d.fingerprint
      window.setTimeout(() => reveal(idx), 60)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [d?.fingerprint])

  // Highlight the current hunk on both sides.
  useEffect(() => {
    const h = hunks[cur]
    if (!decoMod.current || !decoOrig.current) return
    const opts = { isWholeLine: true, className: 'git-current-hunk', linesDecorationsClassName: 'git-current-hunk-gutter' }
    decoMod.current.set(h && h.newLines > 0 ? [{ range: { startLineNumber: h.newStart, startColumn: 1, endLineNumber: h.newStart + h.newLines - 1, endColumn: 1 }, options: opts }] : [])
    decoOrig.current.set(h && h.oldLines > 0 ? [{ range: { startLineNumber: h.oldStart, startColumn: 1, endLineNumber: h.oldStart + h.oldLines - 1, endColumn: 1 }, options: opts }] : [])
  }, [cur, hunks, d?.fingerprint, mounted])

  // Line checkboxes in the glyph margins: additions on the right, deletions on the left.
  useEffect(() => {
    if (!boxMod.current || !boxOrig.current) return
    if (!showChecks) {
      boxMod.current.set([])
      boxOrig.current.set([])
      return
    }
    const hover = { value: lineMode === 'include' ? 'Include this line in the commit · Shift+click: the whole change' : 'Select this line · Shift+click: the whole change' }
    const deco = (l: DiffLine) => ({
      range: { startLineNumber: l.line, startColumn: 1, endLineNumber: l.line, endColumn: 1 },
      options: { glyphMarginClassName: checkedKeys.has(lineKey(l)) ? 'git-lcb on' : 'git-lcb', glyphMarginHoverMessage: hover },
    })
    boxMod.current.set(lines.filter((l) => l.kind === 'add').map(deco))
    boxOrig.current.set(lines.filter((l) => l.kind === 'del').map(deco))
  }, [showChecks, lines, checkedKeys, lineMode, d?.fingerprint, mounted])

  /** Changed lines covered by the editors' selections (with `caret`, the caret lines of `only`). */
  const selectionKeys = (caret: boolean, only?: 'modified' | 'original') => {
    const ed = editorRef.current
    if (!ed) return []
    const mod = only === 'original' ? [] : rangesOf(ed.getModifiedEditor(), caret)
    const orig = only === 'modified' ? [] : rangesOf(ed.getOriginalEditor(), caret)
    return linesInRanges(linesRef.current, mod, orig)
  }

  const runLines = async (op: 'stage' | 'unstage' | 'discard', keys: Iterable<string>) => {
    if (!d) return
    setBusy(true)
    const ok = await applyLines(p.projectId, op, p.path, d.fingerprint, keys)
    setBusy(false)
    if (ok) {
      setChecked(new Set())
      await q.refetch()
    } else void q.refetch()
  }
  const runLinesRef = useRef(runLines)
  runLinesRef.current = runLines
  const includeRef = useRef<(keys: string[], on: boolean) => void>(() => {})
  includeRef.current = (keys, on) => {
    const next = new Set(checkedKeys)
    keys.forEach((k) => (on ? next.add(k) : next.delete(k)))
    setCheckedKeys(next)
  }

  const onMount: DiffOnMount = (editor, monaco) => {
    editorRef.current = editor
    setMounted((n) => n + 1)
    const mod = editor.getModifiedEditor()
    const orig = editor.getOriginalEditor()
    decoMod.current = mod.createDecorationsCollection()
    decoOrig.current = orig.createDecorationsCollection()
    boxMod.current = mod.createDecorationsCollection()
    boxOrig.current = orig.createDecorationsCollection()
    mod.onDidChangeCursorSelection((e) => {
      const i = hunkAtLine(hunksRef.current, e.selection.positionLineNumber)
      if (i >= 0) setCur(i)
    })
    const onSel = () => setSelKeys(selectionKeys(false))
    mod.onDidChangeCursorSelection(onSel)
    orig.onDidChangeCursorSelection(onSel)
    // Clicks on a line checkbox (glyph margin); Shift toggles the whole change.
    for (const [ed, kind] of [
      [mod, 'add'],
      [orig, 'del'],
    ] as const) {
      ed.onMouseDown((e) => {
        if (!showChecksRef.current || e.target.type !== monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN) return
        const n = e.target.position?.lineNumber
        const l = linesRef.current.find((x) => x.kind === kind && x.line === n)
        if (!l) return
        e.event.preventDefault()
        toggleRef.current(e.event.shiftKey ? hunkKeys(linesRef.current, l.hunk) : [lineKey(l)])
      })
      const side = kind === 'add' ? 'modified' : 'original'
      const addAction = (id: string, label: string, run: () => void, order: number) =>
        ed.addAction({ id: `wb.git.${id}`, label, contextMenuGroupId: '0_git', contextMenuOrder: order, run })
      if (p.mode === 'working') {
        addAction('stageLines', 'Stage Selected Lines', () => void runLinesRef.current('stage', selectionKeys(true, side)), 1)
        addAction('rollbackLines', 'Rollback Selected Lines…', () => void runLinesRef.current('discard', selectionKeys(true, side)), 2)
      } else if (p.mode === 'staged') {
        addAction('unstageLines', 'Unstage Selected Lines', () => void runLinesRef.current('unstage', selectionKeys(true, side)), 1)
      } else if (headCompare) {
        addAction('includeLines', 'Include Selected Lines in Commit', () => includeRef.current(selectionKeys(true, side), true), 1)
        addAction('excludeLines', 'Exclude Selected Lines from Commit', () => includeRef.current(selectionKeys(true, side), false), 2)
      }
    }
    if (d) {
      revealedFor.current = d.fingerprint
      window.setTimeout(() => reveal(Math.min(cur, hunksRef.current.length - 1)), 80)
    }
  }

  const hunkAction = async (op: 'stage-hunks' | 'unstage-hunks' | 'discard-hunks') => {
    if (!d || !hunks[cur]) return
    if (op === 'discard-hunks') {
      const h = hunks[cur]
      const ok = await confirmDialog({
        title: 'Roll back this change?',
        message: `Lines ${h.newStart}–${h.newStart + Math.max(0, h.newLines - 1)} of ${splitPath(p.path).name} go back to the staged version.`,
        confirmLabel: 'Rollback',
        danger: true,
      })
      if (!ok) return
    }
    setBusy(true)
    try {
      await gitApi.post(p.projectId, op, { path: p.path, hunkIndexes: [cur], fingerprint: d.fingerprint })
      await q.refetch()
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) {
        toast('warning', e.message)
        void q.refetch()
      } else toastError(e)
    } finally {
      setBusy(false)
    }
  }

  const onKeyDownCapture = (e: React.KeyboardEvent) => {
    if (e.key === 'F7' || (e.altKey && (e.key === 'ArrowDown' || e.key === 'ArrowUp'))) {
      e.preventDefault()
      e.stopPropagation()
      goTo(cur + (e.shiftKey || e.key === 'ArrowUp' ? -1 : 1))
    }
  }

  const readOnlyMode = p.mode === 'commit' || p.mode === 'compare'
  const title =
    p.mode === 'commit'
      ? `${d?.originalLabel ?? 'parent'} ↔ ${shortSha(p.sha)}`
      : p.mode === 'compare'
        ? `${p.base} ↔ ${p.head || 'working tree'}`
        : `${d?.originalLabel ?? (p.mode === 'staged' ? 'HEAD' : 'Index')} ↔ ${d?.modifiedLabel ?? (p.mode === 'staged' ? 'Index' : 'Working tree')}`

  let body: React.ReactNode
  if (q.isLoading) body = <Loading label="Loading diff…" />
  else if (q.error) body = <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
  else if (!d) body = null
  else if (d.conflict)
    body = (
      <EmptyState icon={GitMerge} title="This file has merge conflicts" action={<Button variant="primary" onClick={() => openConflict(p.projectId, p.path)}>Resolve…</Button>}>
        Resolve the conflict in the merge tool, then mark it resolved.
      </EmptyState>
    )
  else if (d.submodule)
    body = (
      <EmptyState icon={FolderGit2} title={`Submodule ${p.path}`}>
        <pre className="git-submodule-summary">{d.submoduleSummary || 'No change the superproject records.'}</pre>
        {p.mode === 'working' && (
          <div>
            Changes inside a submodule are committed (or reset) in the submodule itself.
            {d.submoduleNewCommit ? ' Its new commit can be staged here.' : ''}
          </div>
        )}
      </EmptyState>
    )
  else if (d.binary || d.tooLarge)
    body = (
      <EmptyState
        icon={FileWarning}
        title={d.binary ? (d.lfs ? 'Git LFS file' : 'Binary file') : 'File too large to compare'}
        action={
          !d.modifiedMissing && (
            <Button icon={FileCode} onClick={() => openFile(p.projectId, p.path)}>
              Open File
            </Button>
          )
        }
      >
        {d.binary ? 'The contents are not shown.' : 'One side is larger than 2 MB.'} {d.originalMissing ? 'The file is new.' : d.modifiedMissing ? 'The file was deleted.' : 'The file changed.'}
      </EmptyState>
    )
  else if (!hunks.length && d.original === d.modified && !d.modeChange)
    body = (
      <EmptyState
        icon={GitCommitHorizontal}
        title="No changes"
        action={
          p.mode === 'working' && hasStaged ? (
            <Button onClick={() => openDiff(p.projectId, p.path, 'staged')}>Show Staged Changes</Button>
          ) : p.mode === 'staged' && hasUnstaged ? (
            <Button onClick={() => openDiff(p.projectId, p.path, 'working')}>Show Unstaged Changes</Button>
          ) : undefined
        }
      >
        {p.mode === 'working' ? 'Nothing unstaged in this file.' : p.mode === 'staged' ? 'Nothing staged in this file.' : 'The file is identical on both sides.'}
      </EmptyState>
    )
  else
    body = (
      <GitDiffEditor
        original={d.original}
        modified={d.modified}
        language={lang}
        theme={theme === 'light' ? 'workbench-light' : 'workbench-dark'}
        originalModelPath={`inmemory://git-diff/${encodeURIComponent(panelId)}/original/${p.oldPath ?? p.path}`}
        modifiedModelPath={`inmemory://git-diff/${encodeURIComponent(panelId)}/modified/${p.path}`}
        onMount={onMount}
        options={{
          readOnly: true,
          originalEditable: false,
          // Deleted lines need the original editor for their checkboxes.
          renderSideBySide: sideBySide || showChecks,
          useInlineViewWhenSpaceIsLimited: !showChecks,
          ignoreTrimWhitespace: ignoreWs,
          hideUnchangedRegions: { enabled: collapse },
          wordWrap: wrap ? 'on' : 'off',
          automaticLayout: true,
          minimap: { enabled: false },
          scrollBeyondLastLine: false,
          renderOverviewRuler: true,
          fontSize,
          lineNumbersMinChars: 4,
          glyphMargin: showChecks,
          folding: false,
          renderMarginRevertIcon: false,
          diffWordWrap: wrap ? 'on' : 'off',
        }}
      />
    )

  const hunkTools = d && !d.binary && !d.tooLarge && !d.conflict
  // Lines the line buttons act on: the ticked ones, else a (non-empty) editor selection.
  const lineTargets = showChecks && lineMode === 'stage' ? [...checked] : selKeys
  return (
    <div className="git-diff" data-wb-keys="F7" onKeyDownCapture={onKeyDownCapture}>
      <Toolbar>
        <IconButton size="small" icon={ChevronUp} label="Previous change (Shift+F7)" disabled={!hunks.length} onClick={() => goTo(cur - 1)} />
        <IconButton size="small" icon={ChevronDown} label="Next change (F7)" disabled={!hunks.length} onClick={() => goTo(cur + 1)} />
        <span className="git-hunk-pos">{hunks.length ? `${Math.min(cur + 1, hunks.length)} / ${hunks.length}` : '0 / 0'}</span>
        {hunkTools && !readOnlyMode && (
          <>
            <span className="git-tb-sep" />
            {p.mode === 'working' && d.canStageHunks && !lineTargets.length && (
              <>
                <Button size="small" icon={Plus} disabled={busy || !hunks.length} onClick={() => void hunkAction('stage-hunks')} title="Stage this change">
                  Stage
                </Button>
                <Button size="small" icon={Undo2} disabled={busy || !hunks.length} onClick={() => void hunkAction('discard-hunks')} title="Roll back this change">
                  Rollback
                </Button>
              </>
            )}
            {p.mode === 'staged' && d.canStageHunks && !lineTargets.length && (
              <Button size="small" icon={Minus} disabled={busy || !hunks.length} onClick={() => void hunkAction('unstage-hunks')} title="Unstage this change">
                Unstage
              </Button>
            )}
            {lineMode === 'stage' && lineTargets.length > 0 && (
              <>
                {p.mode === 'working' ? (
                  <>
                    <Button size="small" variant="primary" icon={Plus} disabled={busy} onClick={() => void runLines('stage', lineTargets)} title="Stage the selected lines">
                      Stage {countLabel(lineTargets.length)}
                    </Button>
                    <Button size="small" icon={Undo2} disabled={busy} onClick={() => void runLines('discard', lineTargets)} title="Roll back the selected lines">
                      Rollback
                    </Button>
                  </>
                ) : (
                  <Button size="small" variant="primary" icon={Minus} disabled={busy} onClick={() => void runLines('unstage', lineTargets)} title="Unstage the selected lines">
                    Unstage {countLabel(lineTargets.length)}
                  </Button>
                )}
              </>
            )}
            {p.mode === 'working' && !d.canStageHunks && !d.canSelectLines && hasUnstaged && !d.submodule && (
              <>
                <Button size="small" icon={Plus} onClick={() => void stagePaths(p.projectId, [p.path])}>
                  Stage File
                </Button>
                <Button size="small" icon={Undo2} onClick={() => void rollback(p.projectId, [p.path], 'worktree')}>
                  Rollback File
                </Button>
              </>
            )}
            {p.mode === 'working' && d.submoduleNewCommit && (
              <Button size="small" icon={Plus} onClick={() => void stagePaths(p.projectId, [p.path])}>
                Stage Submodule Commit
              </Button>
            )}
            {p.mode === 'working' && !d.canStageHunks && d.untracked && !hasUnstaged && !lineTargets.length && (
              <Button size="small" icon={Plus} onClick={() => void stagePaths(p.projectId, [p.path])}>
                Add File
              </Button>
            )}
            {p.mode === 'staged' && !d.canStageHunks && hasStaged && !lineTargets.length && (
              <Button size="small" icon={Minus} onClick={() => void unstagePaths(p.projectId, [p.path])}>
                Unstage File
              </Button>
            )}
            {lineMode === 'stage' && (
              <IconButton
                size="small"
                icon={ListChecks}
                active={checkMode}
                label={checkMode ? 'Hide line checkboxes' : 'Select lines with checkboxes'}
                onClick={() => setCheckMode(!checkMode)}
              />
            )}
          </>
        )}
        {lineMode === 'include' && (
          <>
            <span className="git-tb-sep" />
            <span
              className="git-incl wb-small"
              title={`${checkedKeys.size} of ${incl.total} changed line${incl.total === 1 ? '' : 's'} of this file in the commit (the Commit window's changelist view commits them)`}
            >
              <ListChecks size={14} className="wb-muted" />
              {checkedKeys.size === incl.total ? `All ${incl.total}` : `${checkedKeys.size}/${incl.total}`} line{incl.total === 1 ? '' : 's'}
            </span>
            <Button
              size="small"
              variant="ghost"
              disabled={checkedKeys.size === incl.total}
              title="Include every changed line in the commit"
              onClick={() => incl.set(new Set(lines.map(lineKey)))}
            >
              Include All
            </Button>
            <Button size="small" variant="ghost" disabled={!checkedKeys.size} title="Leave every changed line out of the commit" onClick={() => incl.set(new Set())}>
              Exclude All
            </Button>
          </>
        )}
        <span className="git-tb-sep" />
        <IconButton
          size="small"
          icon={sideBySide ? Rows2 : Columns2}
          label={showChecks ? 'Line checkboxes need the side-by-side view' : sideBySide ? 'Unified view' : 'Side-by-side view'}
          disabled={showChecks}
          onClick={() => setSideBySide(!sideBySide)}
        />
        <IconButton size="small" icon={FoldVertical} active={collapse} label="Collapse unchanged fragments" onClick={() => setCollapse(!collapse)} />
        <IconButton size="small" icon={WrapText} active={wrap} label="Soft wrap" onClick={() => setWrap(!wrap)} />
        <span className="wb-small" style={{ marginLeft: 4 }} title="Hide whitespace-only differences">
          <Checkbox checked={ignoreWs} onChange={setIgnoreWs}>
            Ignore whitespace
          </Checkbox>
        </span>
        <span style={{ flex: 1 }} />
        {p.mode === 'working' && hasStaged && (
          <Button size="small" variant="ghost" onClick={() => openDiff(p.projectId, p.path, 'staged')}>
            Staged part
          </Button>
        )}
        {p.mode === 'staged' && hasUnstaged && (
          <Button size="small" variant="ghost" onClick={() => openDiff(p.projectId, p.path, 'working')}>
            Unstaged part
          </Button>
        )}
        {p.mode === 'commit' && p.sha && <IconButton size="small" icon={GitCommitHorizontal} label="Show commit" onClick={() => openCommit(p.projectId, p.sha!)} />}
        {!d?.modifiedMissing && (p.mode === 'working' || p.mode === 'staged' || headCompare) && (
          <IconButton size="small" icon={GitCompareArrows} label="Compare with branch…" onClick={() => compareWithBranch(p.projectId, p.path)} />
        )}
        <IconButton size="small" icon={History} label="Show history" onClick={() => openGitLog(p.projectId, { path: p.path })} />
        <IconButton
          size="small"
          icon={FileCode}
          label="Open file (F4)"
          disabled={d?.modifiedMissing || d?.submodule}
          onClick={() => openFile(p.projectId, p.path, hunks[cur] ? Math.max(1, hunks[cur].newStart) : undefined)}
        />
        <IconButton size="small" icon={RefreshCw} label="Refresh" onClick={() => void q.refetch()} />
      </Toolbar>
      <div className="git-diff-head">
        <span className="side mono" title={p.oldPath ? `${p.oldPath} → ${p.path}` : p.path}>
          {p.oldPath && p.oldPath !== p.path ? `${p.oldPath} → ${p.path}` : p.path}
        </span>
        {d?.modeChange && <span className="wb-badge">mode {d.modeChange}</span>}
        {d?.lfs && <span className="wb-badge warning">LFS</span>}
        {d?.untracked && <span className="wb-badge git-c-untracked">unversioned</span>}
        {showChecks && lineMode === 'stage' && <span className="wb-xs wb-muted">Tick lines, then Stage{p.mode === 'working' ? ' or Rollback' : ''} · Shift+click: whole change</span>}
        <span>{title}</span>
      </div>
      <div className="git-diff-editor">{body}</div>
    </div>
  )
}
