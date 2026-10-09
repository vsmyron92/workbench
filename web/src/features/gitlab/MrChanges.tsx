// MR "Changes" tab: CLion-style changed-files tree on the left, Monaco diff of
// the selected file (raw files at base_sha / head_sha) on the right. Review
// threads show as gutter marks with hover text; the editor context menu adds
// a diff-line comment.

import { useEffect, useMemo, useRef, useState, type MutableRefObject } from 'react'
import type { DiffOnMount } from '@monaco-editor/react'
import { useQueryClient } from '@tanstack/react-query'
import { ChevronDown, ChevronRight, Columns2, FileCode, FoldVertical, Folder, MessageSquare, Rows2, SkipBack, SkipForward } from 'lucide-react'
import { openPanel, promptDialog, toast, toastError } from '@/shell/actions'
import { fileInProject, scopeProject } from '@/api/repos'
import { useUi } from '@/state/store'
import { monacoThemeName } from '@/theme/palette'
import { EmptyState, ErrorBox, IconButton, Loading, MonacoDiffEditor, Spacer, Toolbar } from '@/ui'
import { glApi, glk, useMrDiffs, useMrFile } from './api'
import { Avatar, ExtLink } from './components'
import { changeKind, fileRows, positionForLine, type LineChange } from './logic'
import type { Discussion, Mr, MrDiffFile } from './types'

type DiffEditorT = Parameters<DiffOnMount>[0]
type MonacoT = Parameters<DiffOnMount>[1]
type CodeEditorT = ReturnType<DiffEditorT['getModifiedEditor']>
type Collection = ReturnType<CodeEditorT['createDecorationsCollection']>
type Decorations = NonNullable<Parameters<CodeEditorT['createDecorationsCollection']>[0]>

const fileKey = (f: MrDiffFile) => (f.deletedFile ? f.oldPath : f.newPath)

function threadsFor(discussions: Discussion[], f: MrDiffFile): Discussion[] {
  return discussions.filter((d) => {
    const p = d.notes[0]?.position
    return !!p && !d.notes[0].system && (p.newPath === f.newPath || p.oldPath === f.oldPath)
  })
}

function FileDiff({
  projectId,
  mr,
  file,
  threads,
  sideBySide,
  collapseUnchanged,
  revealRef,
}: {
  projectId: string
  mr: Mr
  file: MrDiffFile
  threads: Discussion[]
  sideBySide: boolean
  collapseUnchanged: boolean
  /** Set to a function that scrolls the diff to a line. */
  revealRef: MutableRefObject<((line: number, old: boolean) => void) | null>
}) {
  const qc = useQueryClient()
  const refs = mr.diffRefs
  const q = useMrFile(projectId, mr.iid, file, refs?.baseSha ?? '', refs?.headSha ?? '')
  const fontSize = useUi((s) => s.prefs.editorFontSize)
  const [language, setLanguage] = useState('plaintext')
  const [mounted, setMounted] = useState(0)
  const editor = useRef<DiffEditorT | null>(null)
  const monaco = useRef<MonacoT | null>(null)
  const decos = useRef<Collection[]>([])
  const current = useRef({ file, mr })
  current.current = { file, mr }
  // @monaco-editor/react disposes the models before the diff widget on unmount,
  // which Monaco reports as an error. We keep them (keepCurrent*Model) and
  // dispose every model this diff used once the editor itself is gone.
  const models = useRef(new Set<{ dispose(): void; isDisposed(): boolean }>())
  useEffect(() => {
    const m = editor.current?.getModel()
    if (m) {
      models.current.add(m.original)
      models.current.add(m.modified)
    }
  })
  useEffect(() => {
    const tracked = models.current
    return () => {
      window.setTimeout(() => tracked.forEach((m) => m.isDisposed() || m.dispose()), 0)
    }
  }, [])

  useEffect(() => {
    let live = true
    // Loaded lazily: the Monaco chunk is already there once a diff renders.
    void import('@/lib/monacoSetup').then((m) => live && setLanguage(m.languageFor(file.newPath)))
    return () => {
      live = false
    }
  }, [file.newPath])

  // Gutter marks and hovers for this file's review threads.
  useEffect(() => {
    const ed = editor.current
    const m = monaco.current
    if (!ed || !m || !q.data) return
    decos.current.forEach((c) => c.clear())
    const mod: Decorations = []
    const orig: Decorations = []
    for (const d of threads) {
      const p = d.notes[0].position
      if (!p) continue
      const hover = d.notes
        .filter((n) => !n.system)
        .map((n) => `**${n.author?.username ?? '?'}**${n.resolved ? ' (resolved)' : ''}: ${n.body}`)
        .join('\n\n---\n\n')
      const deco = (line: number) => ({
        range: new m.Range(line, 1, line, 1),
        options: { isWholeLine: true, className: 'gl-comment-line', glyphMarginClassName: 'gl-comment-glyph', glyphMarginHoverMessage: { value: hover } },
      })
      if (p.newLine) mod.push(deco(p.newLine))
      else if (p.oldLine) orig.push(deco(p.oldLine))
    }
    decos.current = [ed.getModifiedEditor().createDecorationsCollection(mod), ed.getOriginalEditor().createDecorationsCollection(orig)]
  }, [threads, q.data, mounted])

  const onMount: DiffOnMount = (ed, m) => {
    editor.current = ed
    monaco.current = m
    const comment = (side: 'original' | 'modified') => async () => {
      const inner = side === 'modified' ? ed.getModifiedEditor() : ed.getOriginalEditor()
      const line = inner.getPosition()?.lineNumber
      if (!line) return
      const { file: f, mr: cur } = current.current
      const pos = positionForLine((ed.getLineChanges() ?? []) as LineChange[], side, line)
      const body = await promptDialog({
        title: `Comment on ${f.newPath}:${pos.newLine ?? pos.oldLine}`,
        label: 'Markdown',
        multiline: true,
        confirmLabel: 'Comment',
      })
      if (!body?.trim()) return
      try {
        await glApi.addDiscussion(projectId, cur.iid, body, { oldPath: f.oldPath, newPath: f.newPath, ...pos })
        qc.invalidateQueries({ queryKey: glk.mrPart(projectId, cur.iid, 'discussions') })
        toast('success', 'Comment added')
      } catch (e) {
        toastError(e, 'Could not add the comment')
      }
    }
    const action = { id: 'gitlab.review.comment', label: 'Add review comment on this line', contextMenuGroupId: 'navigation', contextMenuOrder: 0 }
    const actions = (['modified', 'original'] as const).map((side) =>
      (side === 'modified' ? ed.getModifiedEditor() : ed.getOriginalEditor()).addAction({ ...action, run: comment(side) }),
    )
    const reveal = (line: number, old: boolean) => {
      const inner = old ? ed.getOriginalEditor() : ed.getModifiedEditor()
      inner.revealLineInCenter(line)
      inner.setPosition({ lineNumber: line, column: 1 })
      inner.focus()
    }
    revealRef.current = reveal
    // A new diff editor is created for every file shown. Monaco keeps an action's
    // command and context-menu item (and so the whole editor, DOM and closures) in
    // global registries until the action is disposed, which disposing the editor
    // does not do. The diff editor's onDidDispose never fires, so hook dispose
    // itself (the wrapper calls it on unmount; MonacoDiffEditor already does, to
    // release the overview ruler) and drop our own references too.
    const dispose = ed.dispose.bind(ed)
    ed.dispose = () => {
      actions.forEach((a) => a.dispose())
      if (revealRef.current === reveal) revealRef.current = null
      if (editor.current === ed) {
        editor.current = null
        decos.current = []
      }
      dispose()
    }
    setMounted((n) => n + 1)
  }

  if (!refs) return <EmptyState title="GitLab has not computed this diff yet" />
  if (q.error) return <ErrorBox error={q.error} onRetry={() => q.refetch()} />
  if (!q.data) return <Loading label="Loading file…" />
  if (q.data.binary) return <EmptyState icon={FileCode} title="Binary file" >Open it on GitLab to see it.</EmptyState>
  if (q.data.tooLarge) return <EmptyState icon={FileCode} title="This file is too large to diff here" action={<ExtLink href={`${mr.webUrl}/diffs`}>Open on GitLab</ExtLink>} />
  const base = `gitlab-mr://${encodeURIComponent(projectId)}/${mr.iid}`
  return (
    <div className="gl-diff-editor">
      <MonacoDiffEditor
        original={q.data.original}
        modified={q.data.modified}
        language={language}
        theme={monacoThemeName()}
        originalModelPath={`${base}/${q.data.baseSha}/${encodeURI(file.oldPath)}`}
        modifiedModelPath={`${base}/${q.data.headSha}/${encodeURI(file.newPath)}`}
        onMount={onMount}
        keepCurrentOriginalModel
        keepCurrentModifiedModel
        options={{
          readOnly: true,
          originalEditable: false,
          renderSideBySide: sideBySide,
          glyphMargin: true,
          fontSize,
          minimap: { enabled: false },
          scrollBeyondLastLine: false,
          hideUnchangedRegions: { enabled: collapseUnchanged },
          renderOverviewRuler: true,
        }}
      />
    </div>
  )
}

function FileComments({ threads, onReveal }: { threads: Discussion[]; onReveal: (line: number, old: boolean) => void }) {
  const [open, setOpen] = useState(true)
  if (!threads.length) return null
  return (
    <div className="gl-file-comments">
      <div className="wb-section-header" onClick={() => setOpen(!open)}>
        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <span>Review threads</span>
        <span className="wb-subtle">{threads.length}</span>
      </div>
      {open &&
        threads.map((d) => {
          const n = d.notes[0]
          const p = n.position!
          const line = p.newLine ?? p.oldLine ?? 0
          const resolved = d.notes.some((x) => x.resolvable) && d.notes.filter((x) => x.resolvable).every((x) => x.resolved)
          return (
            <div key={d.id} className="gl-row" onClick={() => onReveal(line, !p.newLine)}>
              <MessageSquare size={13} className={resolved ? 'wb-subtle' : 'wb-warning'} />
              <span className="title">
                <span className="num">L{line}</span>
                {n.body.split('\n')[0]}
              </span>
              <span className="right">{d.notes.length > 1 ? `${d.notes.length - 1} repl.` : ''}</span>
              <span className="meta">
                <Avatar user={n.author} small />
                {n.author?.username}
                {resolved && <span>· resolved</span>}
              </span>
            </div>
          )
        })}
    </div>
  )
}

export function MrChanges({
  projectId,
  mr,
  discussions,
  focusPath,
}: {
  projectId: string
  mr: Mr
  discussions: Discussion[]
  focusPath: string | null
  visible: boolean
}) {
  const q = useMrDiffs(projectId, mr.iid)
  const [selected, setSelected] = useState<string | null>(focusPath)
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set())
  const [sideBySide, setSideBySide] = useState(true)
  const [collapseUnchanged, setCollapseUnchanged] = useState(true)
  const reveal = useRef<((line: number, old: boolean) => void) | null>(null)
  const files = useMemo(() => q.data?.files ?? [], [q.data])
  const rows = useMemo(() => fileRows(files, collapsed), [files, collapsed])
  const ordered = useMemo(() => rows.filter((r) => r.file).map((r) => r.file!), [rows])

  useEffect(() => {
    if (focusPath) setSelected(focusPath)
  }, [focusPath])
  useEffect(() => {
    if (!selected && ordered.length) setSelected(fileKey(ordered[0]))
  }, [ordered, selected])

  if (q.error) return <ErrorBox error={q.error} onRetry={() => q.refetch()} />
  if (!q.data) return <Loading label="Loading changes…" />
  if (!files.length) return <EmptyState title="No changes" />

  const file = files.find((f) => fileKey(f) === selected || f.oldPath === selected) ?? null
  const idx = file ? ordered.indexOf(file) : -1
  const threads = file ? threadsFor(discussions, file) : []
  const totals = files.reduce((a, f) => [a[0] + f.additions, a[1] + f.deletions], [0, 0])
  const step = (d: number) => {
    const next = ordered[idx + d]
    if (next) setSelected(fileKey(next))
  }

  return (
    <div className="gl-changes">
      <div className="gl-tree" role="tree">
        <div className="wb-section-header" style={{ cursor: 'default' }}>
          <span>{files.length} files</span>
          <span className="gl-add">+{totals[0]}</span>
          <span className="gl-del">−{totals[1]}</span>
        </div>
        {q.data.truncated && <div className="wb-small wb-warning" style={{ padding: '0 8px 4px' }}>Only the first {files.length} files are shown.</div>}
        {rows.map((r) =>
          r.kind === 'dir' ? (
            <div
              key={`d:${r.path}`}
              className="gl-tree-row"
              style={{ paddingLeft: 6 + r.depth * 14 }}
              onClick={() =>
                setCollapsed((c) => {
                  const n = new Set(c)
                  if (n.has(r.path)) n.delete(r.path)
                  else n.add(r.path)
                  return n
                })
              }
            >
              {collapsed.has(r.path) ? <ChevronRight size={13} /> : <ChevronDown size={13} />}
              <Folder size={13} className="wb-muted" />
              <span className="n dirname">{r.name}</span>
            </div>
          ) : (
            <div
              key={`f:${r.path}:${r.file!.oldPath}`}
              className={r.file === file ? 'gl-tree-row selected' : 'gl-tree-row'}
              style={{ paddingLeft: 19 + r.depth * 14 }}
              onClick={() => setSelected(fileKey(r.file!))}
              title={r.file!.renamedFile ? `${r.file!.oldPath} → ${r.file!.newPath}` : r.path}
            >
              <span className={`n gl-vcs-${changeKind(r.file!)}`}>{r.name}</span>
              {threadsFor(discussions, r.file!).length > 0 && <MessageSquare size={11} className="wb-muted" />}
              <span className="stats">
                {r.file!.additions > 0 && <span className="gl-add">+{r.file!.additions} </span>}
                {r.file!.deletions > 0 && <span className="gl-del">−{r.file!.deletions}</span>}
              </span>
            </div>
          ),
        )}
      </div>
      <div className="gl-diff">
        {file ? (
          <>
            <Toolbar>
              <span className={`title wb-ellipsis gl-vcs-${changeKind(file)}`} title={file.newPath}>
                {file.renamedFile ? `${file.oldPath} → ${file.newPath}` : file.newPath}
              </span>
              <span className="wb-small">
                <span className="gl-add">+{file.additions}</span> <span className="gl-del">−{file.deletions}</span>
              </span>
              <Spacer />
              <IconButton icon={SkipBack} size="small" label="Previous file" disabled={idx <= 0} onClick={() => step(-1)} />
              <IconButton icon={SkipForward} size="small" label="Next file" disabled={idx >= ordered.length - 1} onClick={() => step(1)} />
              <IconButton
                icon={FoldVertical}
                size="small"
                label={collapseUnchanged ? 'Show unchanged lines' : 'Collapse unchanged lines'}
                active={collapseUnchanged}
                onClick={() => setCollapseUnchanged(!collapseUnchanged)}
              />
              <IconButton
                icon={sideBySide ? Rows2 : Columns2}
                size="small"
                label={sideBySide ? 'Unified view' : 'Side-by-side view'}
                onClick={() => setSideBySide(!sideBySide)}
              />
              {!file.deletedFile && (
                <IconButton
                  icon={FileCode}
                  size="small"
                  label="Open the local file"
                  onClick={() =>
                    // The editor belongs to the real project, and GitLab's path is relative to the repository.
                    openPanel({
                      kind: 'editor',
                      id: `editor:${scopeProject(projectId)}:${fileInProject(projectId, file.newPath)}`,
                      title: file.newPath.split('/').pop(),
                      params: { projectId: scopeProject(projectId), path: fileInProject(projectId, file.newPath) },
                    })
                  }
                />
              )}
            </Toolbar>
            <FileDiff
              key={`${file.oldPath}\u0000${file.newPath}`}
              projectId={projectId}
              mr={mr}
              file={file}
              threads={threads}
              sideBySide={sideBySide}
              collapseUnchanged={collapseUnchanged}
              revealRef={reveal}
            />
            <FileComments threads={threads} onReveal={(line, old) => reveal.current?.(line, old)} />
          </>
        ) : (
          <EmptyState title="Select a file" />
        )}
      </div>
    </div>
  )
}
