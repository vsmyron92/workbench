// Editor VCS integration: change markers in the gutter (live against the git
// base text, with Rollback) and the Blame (annotate) column.

import { useEffect, useMemo, useRef, useState } from 'react'
import type { editor } from 'monaco-editor'
import { useEvent } from '@/api/events'
import { cssVar } from '@/theme/palette'
import { filesApi, type GitBlame } from './api'
import { diffLines, splitLines, type ChangeBlock } from './lineDiff'
import { isScratch } from './scratchStore'

/** The git base text (index version) of a file; null when unavailable. */
export function useGitBase(projectId: string | null, path: string, enabled: boolean, revision: number): string[] | null {
  const [base, setBase] = useState<string[] | null>(null)
  const [nonce, setNonce] = useState(0)
  useEvent('git.changed', (ev) => {
    if (enabled && ev.projectId === projectId) setNonce((n) => n + 1)
  })
  useEffect(() => {
    if (!enabled || !projectId || isScratch(projectId)) {
      setBase(null)
      return
    }
    let cancelled = false
    filesApi
      .gitDiff(projectId, path)
      .then((d) => !cancelled && setBase(d.binary || d.tooLarge ? null : splitLines(d.original ?? '')))
      .catch(() => !cancelled && setBase(null))
    return () => {
      cancelled = true
    }
  }, [projectId, path, enabled, nonce, revision])
  return base
}

const CLASS: Record<ChangeBlock['kind'], string> = {
  added: 'wb-gutter-add',
  modified: 'wb-gutter-mod',
  deleted: 'wb-gutter-del',
}

/**
 * Keeps change markers on `ed` up to date while the model is edited. Returns a
 * ref to the current blocks (for the gutter click menu).
 */
export function useChangeMarkers(ed: editor.IStandaloneCodeEditor | null, model: editor.ITextModel | null, base: string[] | null) {
  const blocks = useRef<ChangeBlock[]>([])
  useEffect(() => {
    blocks.current = []
    if (!ed || !model || !base || model.isDisposed()) return
    const coll = ed.createDecorationsCollection()
    const colors = { added: cssVar('--vcs-added'), modified: cssVar('--vcs-modified'), deleted: cssVar('--vcs-deleted') }
    const compute = () => {
      if (model.isDisposed()) return
      const bs = model.getLineCount() > 200_000 ? [] : diffLines(base, model.getLinesContent())
      blocks.current = bs
      coll.set(
        bs.map((b) => {
          const top = b.kind === 'deleted' && b.start === 0
          const line = b.kind === 'deleted' ? Math.max(1, b.start) : b.start
          const end = b.kind === 'deleted' ? line : b.end
          return {
            range: { startLineNumber: line, startColumn: 1, endLineNumber: end, endColumn: 1 },
            options: {
              isWholeLine: true,
              linesDecorationsClassName: top ? 'wb-gutter-del top' : CLASS[b.kind],
              overviewRuler: { color: colors[b.kind], position: 1 },
            },
          }
        }),
      )
    }
    compute()
    let t: number | undefined
    const sub = model.onDidChangeContent(() => {
      window.clearTimeout(t)
      t = window.setTimeout(compute, 250)
    })
    return () => {
      window.clearTimeout(t)
      sub.dispose()
      coll.clear()
    }
  }, [ed, model, base])
  return blocks
}

/** Change block covering `line` (1-based). */
export function blockAt(blocks: ChangeBlock[], line: number): ChangeBlock | null {
  return (
    blocks.find((b) => (b.kind === 'deleted' ? Math.max(1, b.start) === line : line >= b.start && line <= b.end)) ?? null
  )
}

// ---------------------------------------------------------------- blame

export type BlameLine = GitBlame['lines'][number]

function toMs(t: number) {
  return t < 1e12 ? t * 1000 : t
}

export function blameDate(t: number): string {
  const d = new Date(toMs(t))
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`
}

export function blameTime(t: number) {
  return toMs(t)
}

/** Line → blame entry, plus the `lineNumbers` renderer for the annotate column. */
export function useBlame(blame: GitBlame | null) {
  return useMemo(() => {
    if (!blame) return null
    const byLine = new Map<number, BlameLine>()
    for (const l of blame.lines) byLine.set(l.line, l)
    const width = 11 + 1 + 12
    const render = (n: number) => {
      const b = byLine.get(n)
      const label = b ? `${blameDate(b.time)} ${b.author.slice(0, 12).padEnd(12)}` : ' '.repeat(width)
      return `${label}  ${n}`
    }
    return { byLine, render, minChars: width + 6 }
  }, [blame])
}
