// Pull request "Files changed": CLion-style changed-files tree on the left,
// Monaco diff of the selected file (merge base ↔ head) on the right. Review
// threads show as gutter marks with hover text; the editor context menu adds
// a line comment (GitHub takes them only on lines inside the diff's hunks).

import { useEffect, useMemo, useRef, useState, type MutableRefObject } from 'react'
import type { DiffOnMount } from '@monaco-editor/react'
import { useQueryClient } from '@tanstack/react-query'
import { ChevronDown, ChevronRight, Columns2, FileCode, FoldVertical, Folder, MessageSquare, Rows2, SkipBack, SkipForward } from 'lucide-react'
import { openPanel, promptDialog, toast, toastError } from '@/shell/actions'
import { useUi } from '@/state/store'
import { monacoThemeName } from '@/theme/palette'
import { EmptyState, ErrorBox, IconButton, Loading, MonacoDiffEditor, Spacer, Toolbar } from '@/ui'
import { ghApi, ghk, usePrFile, usePrFiles } from './api'
import { Avatar, ExtLink } from './components'
import { changeKind, commentTarget, diffLines, fileRows, placedThreads } from './logic'
import type { PrFile, PullDetail, Thread } from './types'

type DiffEditorT = Parameters<DiffOnMount>[0]
type MonacoT = Parameters<DiffOnMount>[1]
type CodeEditorT = ReturnType<DiffEditorT['getModifiedEditor']>
type Collection = ReturnType<CodeEditorT['createDecorationsCollection']>
type Decorations = NonNullable<Parameters<CodeEditorT['createDecorationsCollection']>[0]>

function threadsFor(threads: readonly Thread[], f: PrFile): Thread[] {
  return threads.filter((t) => t.path === f.filename || (!!f.previousFilename && t.path === f.previousFilename))
}

function FileDiff({
  projectId,
  pr,
  file,
  threads,
  sideBySide,
  collapseUnchanged,
  anonymous,
  revealRef,
}: {
  projectId: string
  pr: PullDetail
  file: PrFile
  threads: Thread[]
  sideBySide: boolean
  collapseUnchanged: boolean
  anonymous: boolean
  /** Set to a function that scrolls the diff to a line. */
  revealRef: MutableRefObject<((line: number, left: boolean) => void) | null>
}) {
  const qc = useQueryClient()
  const head = pr.head?.sha ?? ''
  const base = pr.mergeBaseSha ?? ''
  const q = usePrFile(projectId, pr.number, file, base, head)
  const fontSize = useUi((s) => s.prefs.editorFontSize)
  const [language, setLanguage] = useState('plaintext')
  const [mounted, setMounted] = useState(0)
  const editor = useRef<DiffEditorT | null>(null)
  const monaco = useRef<MonacoT | null>(null)
  const decos = useRef<Collection[]>([])
  const current = useRef({ file, pr, anonymous })
  current.current = { file, pr, anonymous }
  // @monaco-editor/react disposes models before the diff widget on unmount; keep
  // them and dispose every model this diff used once the editor itself is gone.
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
    void import('@/lib/monacoSetup').then((m) => live && setLanguage(m.languageFor(file.filename)))
    return () => {
      live = false
    }
  }, [file.filename])

  // Gutter marks and hovers for this file's review threads.
  useEffect(() => {
    const ed = editor.current
    const m = monaco.current
    if (!ed || !m || !q.data) return
    decos.current.forEach((c) => c.clear())
    const mod: Decorations = []
    const orig: Decorations = []
    for (const { thread, side, line } of placedThreads(threads, file.filename)) {
      const hover = thread.comments.map((c) => `**${c.user?.login ?? '?'}**: ${c.body}`).join('\n\n---\n\n') + (thread.resolved ? '\n\n*(resolved)*' : '')
      const deco = {
        range: new m.Range(line, 1, line, 1),
        options: { isWholeLine: true, className: 'gh-comment-line', glyphMarginClassName: 'gh-comment-glyph', glyphMarginHoverMessage: { value: hover } },
      }
      if (side === 'LEFT') orig.push(deco)
      else mod.push(deco)
    }
    decos.current = [ed.getModifiedEditor().createDecorationsCollection(mod), ed.getOriginalEditor().createDecorationsCollection(orig)]
  }, [threads, q.data, mounted, file.filename])

  const onMount: DiffOnMount = (ed, m) => {
    editor.current = ed
    monaco.current = m
    const comment = (side: 'original' | 'modified') => async () => {
      const inner = side === 'modified' ? ed.getModifiedEditor() : ed.getOriginalEditor()
      const line = inner.getPosition()?.lineNumber
      if (!line) return
      const { file: f, pr: cur, anonymous: anon } = current.current
      if (anon) {
        toast('info', 'Commenting needs a GitHub token')
        return
      }
      const target = commentTarget(diffLines(f.patch), side, line)
      if (!target) {
        toast('info', 'GitHub takes comments only on lines in the diff (changed lines and the lines around them)')
        return
      }
      const body = await promptDialog({ title: `Comment on ${f.filename}:${target.line}`, label: 'Markdown', multiline: true, confirmLabel: 'Comment' })
      if (!body?.trim()) return
      try {
        await ghApi.reviewComment(projectId, cur.number, { body, path: f.filename, line: target.line, side: target.side, commitId: cur.head?.sha })
        void qc.invalidateQueries({ queryKey: ghk.pullPart(projectId, cur.number, 'threads') })
        toast('success', 'Comment added')
      } catch (e) {
        toastError(e, 'Could not add the comment')
      }
    }
    const action = { id: 'github.review.comment', label: 'Add review comment on this line', contextMenuGroupId: 'navigation', contextMenuOrder: 0 }
    const actions = (['modified', 'original'] as const).map((side) =>
      (side === 'modified' ? ed.getModifiedEditor() : ed.getOriginalEditor()).addAction({ ...action, run: comment(side) }),
    )
    const reveal = (line: number, left: boolean) => {
      const inner = left ? ed.getOriginalEditor() : ed.getModifiedEditor()
      inner.revealLineInCenter(line)
      inner.setPosition({ lineNumber: line, column: 1 })
      inner.focus()
    }
    revealRef.current = reveal
    // Actions live in Monaco's global registries until disposed, which disposing
    // the diff editor does not do: hook dispose (see the GitLab MR diff too).
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

  if (!head || !base) return <EmptyState title="GitHub has not computed this pull request's commits yet" />
  if (q.error) return <ErrorBox error={q.error} onRetry={() => q.refetch()} />
  if (!q.data) return <Loading label="Loading file…" />
  if (q.data.binary) return <EmptyState icon={FileCode} title="Binary file">Open it on GitHub to see it.</EmptyState>
  if (q.data.tooLarge) return <EmptyState icon={FileCode} title="This file is too large to diff here" action={<ExtLink href={`${pr.htmlUrl}/files`}>Open on GitHub</ExtLink>} />
  const uri = `github-pr://${encodeURIComponent(projectId)}/${pr.number}`
  return (
    <div className="gh-diff-editor">
      <MonacoDiffEditor
        original={q.data.original}
        modified={q.data.modified}
        language={language}
        theme={monacoThemeName()}
        originalModelPath={`${uri}/${q.data.baseSha}/${encodeURI(file.previousFilename ?? file.filename)}`}
        modifiedModelPath={`${uri}/${q.data.headSha}/${encodeURI(file.filename)}`}
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

function FileThreads({ threads, onReveal }: { threads: Thread[]; onReveal: (line: number, left: boolean) => void }) {
  const [open, setOpen] = useState(true)
  if (!threads.length) return null
  return (
    <div className="gh-file-comments">
      <div className="wb-section-header" onClick={() => setOpen(!open)}>
        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <span>Review threads</span>
        <span className="wb-subtle">{threads.length}</span>
      </div>
      {open &&
        threads.map((t) => {
          const c = t.comments[0]
          const line = t.line ?? t.originalLine ?? 0
          return (
            <div key={t.id} className="gh-row" onClick={() => !t.outdated && t.line && onReveal(t.line, t.side === 'LEFT')}>
              <MessageSquare size={13} className={t.resolved ? 'wb-subtle' : t.resolved === false ? 'wb-warning' : 'wb-muted'} />
              <span className="title">
                <span className="num">L{line}</span>
                {c?.body.split('\n')[0]}
              </span>
              <span className="right">{t.comments.length > 1 ? `${t.comments.length - 1} repl.` : ''}</span>
              <span className="meta">
                <Avatar user={c?.user} small />
                {c?.user?.login}
                {t.outdated && <span>· outdated</span>}
                {t.resolved && <span>· resolved</span>}
              </span>
            </div>
          )
        })}
    </div>
  )
}

export function PrFilesView({
  projectId,
  pr,
  threads,
  focusPath,
  anonymous,
}: {
  projectId: string
  pr: PullDetail
  threads: Thread[]
  focusPath: string | null
  anonymous: boolean
}) {
  const q = usePrFiles(projectId, pr.number)
  const [selected, setSelected] = useState<string | null>(focusPath)
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set())
  const [sideBySide, setSideBySide] = useState(true)
  const [collapseUnchanged, setCollapseUnchanged] = useState(true)
  const reveal = useRef<((line: number, left: boolean) => void) | null>(null)
  const files = useMemo(() => q.data?.files ?? [], [q.data])
  const rows = useMemo(() => fileRows(files, collapsed), [files, collapsed])
  const ordered = useMemo(() => rows.filter((r) => r.file).map((r) => r.file!), [rows])

  useEffect(() => {
    if (focusPath) setSelected(focusPath)
  }, [focusPath])
  useEffect(() => {
    if (!selected && ordered.length) setSelected(ordered[0].filename)
  }, [ordered, selected])

  if (q.error) return <ErrorBox error={q.error} onRetry={() => q.refetch()} />
  if (!q.data) return <Loading label="Loading changes…" />
  if (!files.length) return <EmptyState title="No changes" />

  const file = files.find((f) => f.filename === selected || f.previousFilename === selected) ?? null
  const idx = file ? ordered.indexOf(file) : -1
  const fileThreads = file ? threadsFor(threads, file) : []
  const totals = files.reduce((a, f) => [a[0] + f.additions, a[1] + f.deletions], [0, 0])
  const step = (d: number) => {
    const next = ordered[idx + d]
    if (next) setSelected(next.filename)
  }

  return (
    <div className="gh-changes">
      <div className="gh-tree" role="tree">
        <div className="wb-section-header" style={{ cursor: 'default' }}>
          <span>{files.length} files</span>
          <span className="gh-add">+{totals[0]}</span>
          <span className="gh-del">−{totals[1]}</span>
        </div>
        {q.data.truncated && <div className="wb-small wb-warning" style={{ padding: '0 8px 4px' }}>Only the first {files.length} files are shown.</div>}
        {rows.map((r) =>
          r.kind === 'dir' ? (
            <div
              key={`d:${r.path}`}
              className="gh-tree-row"
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
              key={`f:${r.path}`}
              className={r.file === file ? 'gh-tree-row selected' : 'gh-tree-row'}
              style={{ paddingLeft: 19 + r.depth * 14 }}
              onClick={() => setSelected(r.file!.filename)}
              title={r.file!.previousFilename ? `${r.file!.previousFilename} → ${r.file!.filename}` : r.path}
            >
              <span className={`n gh-vcs-${changeKind(r.file!)}`}>{r.name}</span>
              {threadsFor(threads, r.file!).length > 0 && <MessageSquare size={11} className="wb-muted" />}
              <span className="stats">
                {r.file!.additions > 0 && <span className="gh-add">+{r.file!.additions} </span>}
                {r.file!.deletions > 0 && <span className="gh-del">−{r.file!.deletions}</span>}
              </span>
            </div>
          ),
        )}
      </div>
      <div className="gh-diff">
        {file ? (
          <>
            <Toolbar>
              <span className={`title wb-ellipsis gh-vcs-${changeKind(file)}`} title={file.filename}>
                {file.previousFilename && file.status === 'renamed' ? `${file.previousFilename} → ${file.filename}` : file.filename}
              </span>
              <span className="wb-small">
                <span className="gh-add">+{file.additions}</span> <span className="gh-del">−{file.deletions}</span>
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
              <IconButton icon={sideBySide ? Rows2 : Columns2} size="small" label={sideBySide ? 'Unified view' : 'Side-by-side view'} onClick={() => setSideBySide(!sideBySide)} />
              {file.status !== 'removed' && (
                <IconButton
                  icon={FileCode}
                  size="small"
                  label="Open the local file"
                  onClick={() =>
                    openPanel({ kind: 'editor', id: `editor:${projectId}:${file.filename}`, title: file.filename.split('/').pop(), params: { projectId, path: file.filename } })
                  }
                />
              )}
            </Toolbar>
            <FileDiff
              key={`${file.previousFilename ?? ''}\u0000${file.filename}`}
              projectId={projectId}
              pr={pr}
              file={file}
              threads={fileThreads}
              sideBySide={sideBySide}
              collapseUnchanged={collapseUnchanged}
              anonymous={anonymous}
              revealRef={reveal}
            />
            <FileThreads threads={fileThreads} onReveal={(line, left) => reveal.current?.(line, left)} />
          </>
        ) : (
          <EmptyState title="Select a file" />
        )}
      </div>
    </div>
  )
}
