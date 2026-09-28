// The 'conflict' panel: CLion-style three-way merge. Yours | Result (editable,
// starts from the working-tree file with conflict markers) | Theirs. Each
// conflict block can take yours, theirs or both; the whole file can take a side.

import { useEffect, useMemo, useRef, useState } from 'react'
import type { OnMount } from '@monaco-editor/react'
import { ArrowLeftToLine, ArrowRightToLine, Check, ChevronDown, ChevronUp, FileCode, GitMerge, RefreshCw, Rows2 } from 'lucide-react'
import { confirmDialog, toast, toastError } from '@/shell/actions'
import type { PanelProps } from '@/shell/types'
import { useUi } from '@/state/store'
import { Button, EmptyState, ErrorBox, IconButton, Loading, MonacoEditor, Toolbar } from '@/ui'
import { gitApi, useConflict } from './api'
import { openFile } from './actions'
import { parseConflictBlocks, resolveConflictBlock, splitPath, type BlockChoice, type ConflictBlock } from './logic'

type EditorT = Parameters<OnMount>[0]
type Decorations = ReturnType<EditorT['createDecorationsCollection']>

export function ConflictPanel({ params, close }: PanelProps<{ projectId: string; path: string }>) {
  if (!params?.projectId || !params.path) return <EmptyState title="No file" />
  return <ConflictView key={`${params.projectId}:${params.path}`} pid={params.projectId} path={params.path} close={close} />
}

/** Scroll a side editor to the first line of `lines`, nearest to `near`. */
function revealText(ed: EditorT | null, lines: string[], near: number) {
  const first = lines.find((l) => l.trim().length > 2)?.replace(/\r?\n$/, '')
  const model = ed?.getModel()
  if (!ed || !model || !first) return
  const matches = model.findMatches(first, false, false, true, null, false, 50)
  if (!matches.length) return
  const best = matches.reduce((a, b) => (Math.abs(b.range.startLineNumber - near) < Math.abs(a.range.startLineNumber - near) ? b : a))
  ed.revealLineInCenter(best.range.startLineNumber)
  ed.setSelection(best.range)
}

function ConflictView({ pid, path, close }: { pid: string; path: string; close: () => void }) {
  const q = useConflict(pid, path)
  const d = q.data
  const theme = useUi((s) => s.prefs.theme)
  const fontSize = useUi((s) => s.prefs.editorFontSize)
  const [result, setResult] = useState<string | null>(null)
  const [dirty, setDirty] = useState(false)
  const [cur, setCur] = useState(0)
  const [busy, setBusy] = useState(false)
  const [lang, setLang] = useState('plaintext')
  const resultEd = useRef<EditorT | null>(null)
  const oursEd = useRef<EditorT | null>(null)
  const theirsEd = useRef<EditorT | null>(null)
  const deco = useRef<Decorations | null>(null)
  const [mounted, setMounted] = useState(false)

  useEffect(() => {
    let alive = true
    void import('@/lib/monacoSetup').then((m) => alive && setLang(m.languageFor(path)))
    return () => {
      alive = false
    }
  }, [path])

  // Take the server's merged text until the user starts editing.
  useEffect(() => {
    if (d && !dirty) setResult(d.merged)
  }, [d, dirty])

  const blocks: ConflictBlock[] = useMemo(() => parseConflictBlocks(result ?? ''), [result])
  const blocksRef = useRef(blocks)
  blocksRef.current = blocks
  const revealedFirst = useRef(false)

  // Jump to the first conflict once the result editor has its text.
  useEffect(() => {
    if (!mounted || result === null || revealedFirst.current || !blocks.length) return
    revealedFirst.current = true
    window.setTimeout(() => goTo(0), 50)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mounted, result])

  useEffect(() => {
    deco.current?.set(
      blocks.map((b) => ({
        range: { startLineNumber: b.start + 1, startColumn: 1, endLineNumber: b.end + 1, endColumn: 1 },
        options: { isWholeLine: true, className: 'git-conflict-block', linesDecorationsClassName: 'git-conflict-gutter' },
      })),
    )
    if (cur >= blocks.length && blocks.length) setCur(blocks.length - 1)
  }, [blocks, cur, mounted])

  const goTo = (i: number) => {
    const bs = blocksRef.current
    if (!bs.length) return
    const idx = ((i % bs.length) + bs.length) % bs.length
    setCur(idx)
    const b = bs[idx]
    resultEd.current?.revealLineInCenter(b.start + 1)
    resultEd.current?.setPosition({ lineNumber: b.start + 1, column: 1 })
    revealText(oursEd.current, b.ours, b.start + 1)
    revealText(theirsEd.current, b.theirs, b.start + 1)
  }

  const take = (choice: BlockChoice) => {
    if (result === null || !blocks[cur]) return
    setResult(resolveConflictBlock(result, cur, choice))
    setDirty(true)
    window.setTimeout(() => goTo(cur), 30)
  }

  const finish = async (body: { content?: string; side?: 'ours' | 'theirs' }, label: string) => {
    setBusy(true)
    try {
      await gitApi.post(pid, 'conflict/resolve', { path, ...body })
      toast('success', `${splitPath(path).name} resolved (${label})`)
      close()
    } catch (e) {
      toastError(e, 'Resolve failed')
    } finally {
      setBusy(false)
    }
  }

  const save = async () => {
    if (result === null) return
    if (blocks.length) {
      const ok = await confirmDialog({
        title: 'Conflict markers remain',
        message: `The result still has ${blocks.length} unresolved block${blocks.length === 1 ? '' : 's'}. Mark the file resolved anyway?`,
        confirmLabel: 'Mark Resolved',
        danger: true,
      })
      if (!ok) return
    }
    await finish({ content: result }, 'merged')
  }

  if (q.isLoading) return <Loading label="Loading conflict…" />
  if (q.error) return <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
  if (!d) return null
  if (!d.inConflict)
    return (
      <EmptyState icon={Check} title="No conflict" action={<Button onClick={close}>Close</Button>}>
        {path} is not in conflict (anymore).
      </EmptyState>
    )

  const themeName = theme === 'light' ? 'workbench-light' : 'workbench-dark'
  const common = {
    language: lang,
    theme: themeName,
    options: {
      fontSize,
      minimap: { enabled: false },
      automaticLayout: true,
      scrollBeyondLastLine: false,
      lineNumbersMinChars: 3,
      folding: false,
      glyphMargin: false,
    },
  }
  const sideOnly = d.binary || d.tooLarge

  return (
    <div className="git-merge">
      {/* Groups wrap as units in a narrow dock group, so every action stays reachable. */}
      <Toolbar>
        <span className="git-tb-group git-merge-path">
          <GitMerge size={15} className="wb-muted" />
          <span className="wb-small wb-ellipsis" title={path}>
            {path}
          </span>
        </span>
        {!sideOnly && (
          <span className="git-tb-group">
            <IconButton size="small" icon={ChevronUp} label="Previous conflict" disabled={!blocks.length} onClick={() => goTo(cur - 1)} />
            <IconButton size="small" icon={ChevronDown} label="Next conflict" disabled={!blocks.length} onClick={() => goTo(cur + 1)} />
            <span className="git-hunk-pos">{blocks.length ? `${Math.min(cur + 1, blocks.length)} / ${blocks.length}` : 'none left'}</span>
            <Button size="small" icon={ArrowRightToLine} disabled={!blocks.length} onClick={() => take('ours')} title="Use yours for this conflict">
              Take Yours
            </Button>
            <Button size="small" icon={ArrowLeftToLine} disabled={!blocks.length} onClick={() => take('theirs')} title="Use theirs for this conflict">
              Take Theirs
            </Button>
            <Button size="small" icon={Rows2} disabled={!blocks.length} onClick={() => take('both')} title="Yours, then theirs">
              Both
            </Button>
          </span>
        )}
        <span className="git-tb-group">
          <Button size="small" disabled={busy} onClick={() => void finish({ side: 'ours' }, 'yours')}>
            Accept Yours
          </Button>
          <Button size="small" disabled={busy} onClick={() => void finish({ side: 'theirs' }, 'theirs')}>
            Accept Theirs
          </Button>
        </span>
        <span style={{ flex: 1 }} />
        <span className="git-tb-group">
          <IconButton size="small" icon={FileCode} label="Open in editor" onClick={() => openFile(pid, path)} />
          <IconButton
            size="small"
            icon={RefreshCw}
            label="Reload from disk"
            onClick={() => {
              setDirty(false)
              void q.refetch()
            }}
          />
          {!sideOnly && (
            <Button size="small" variant="primary" icon={Check} loading={busy} onClick={() => void save()}>
              Save and Mark Resolved
            </Button>
          )}
        </span>
      </Toolbar>
      {sideOnly ? (
        <EmptyState icon={GitMerge} title={d.binary ? 'Binary file in conflict' : 'File too large to merge here'}>
          Choose a whole side with Accept Yours or Accept Theirs.
        </EmptyState>
      ) : (
        <div className="git-merge-cols">
          <div className="git-merge-col">
            <header>
              <span className="label" title={d.oursLabel}>
                {d.oursLabel}
              </span>
            </header>
            <div className="ed">
              {d.ours === null ? (
                <EmptyState title="Deleted in this version" />
              ) : (
                <MonacoEditor {...common} value={d.ours} path={`inmemory://git-merge/${pid}/ours/${path}`} options={{ ...common.options, readOnly: true }} onMount={(e) => (oursEd.current = e)} />
              )}
            </div>
          </div>
          <div className="git-merge-col result">
            <header>
              <span className="label">Result{dirty ? ' (edited)' : ''}</span>
              <span className="wb-xs wb-subtle">{blocks.length ? `${blocks.length} conflict${blocks.length === 1 ? '' : 's'} left` : 'no markers left'}</span>
            </header>
            <div className="ed">
              <MonacoEditor
                {...common}
                value={result ?? ''}
                path={`inmemory://git-merge/${pid}/result/${path}`}
                onChange={(v) => {
                  setResult(v ?? '')
                  setDirty(true)
                }}
                onMount={(e) => {
                  resultEd.current = e
                  deco.current = e.createDecorationsCollection()
                  setMounted(true)
                }}
              />
            </div>
          </div>
          <div className="git-merge-col">
            <header>
              <span className="label" title={d.theirsLabel}>
                {d.theirsLabel}
              </span>
            </header>
            <div className="ed">
              {d.theirs === null ? (
                <EmptyState title="Deleted in this version" />
              ) : (
                <MonacoEditor {...common} value={d.theirs} path={`inmemory://git-merge/${pid}/theirs/${path}`} options={{ ...common.options, readOnly: true }} onMount={(e) => (theirsEd.current = e)} />
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
