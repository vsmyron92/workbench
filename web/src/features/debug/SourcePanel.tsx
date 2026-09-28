// The `debug.source` panel: a read-only view of a frame's source outside the project
// (a library header, a crate's source, the standard library — `GET
// sessions/{sid}/file`, which serves only files the session's frames or output named)
// or of source only the debugger holds (`sourceReference`: `GET sessions/{sid}/source`),
// with the session's execution point and selected frame as the editor shows them.
// Its models are `inmemory://debug-source/<sessionId>/…`, never `file:` models (those
// belong to the files slice).

import { useEffect, useMemo, useRef, useState } from 'react'
import type { OnMount } from '@monaco-editor/react'
import { useQuery } from '@tanstack/react-query'
import { FileCode, Lock } from 'lucide-react'
import type { PanelProps } from '@/shell/types'
import { useUi } from '@/state/store'
import { cssVar } from '@/theme/palette'
import { EmptyState, ErrorBox, Loading, MonacoEditor } from '@/ui'
import { debugApi } from './api'
import { sourceLines } from './logic'
import { useDebug } from './store'

type EditorT = Parameters<OnMount>[0]

export interface SourceParams {
  projectId: string
  sessionId: string
  /** Absolute (a file outside the project), or none with `sourceReference`. */
  path?: string
  sourceReference?: number
  name?: string
  line?: number
  column?: number
  /** Forces a re-navigation to `line`. */
  t?: number
}

/** The URI of a source view's model (see `sourceOfModel` in editor.ts). */
export function sourceModelPath(p: Pick<SourceParams, 'sessionId' | 'path' | 'sourceReference'>): string {
  const rest = p.path ? p.path.replace(/^\/+/, '') : `ref/${p.sourceReference}`
  return `inmemory://debug-source/${encodeURIComponent(p.sessionId)}/${rest}`
}

export function DebugSourcePanel({ params }: PanelProps<SourceParams>) {
  if (!params?.projectId || !params.sessionId || (!params.path && !params.sourceReference)) {
    return <EmptyState icon={FileCode} title="No source" />
  }
  return <SourceView p={params} />
}

function SourceView({ p }: { p: SourceParams }) {
  const theme = useUi((s) => s.prefs.theme)
  const fontSize = useUi((s) => s.prefs.editorFontSize)
  const [lang, setLang] = useState('plaintext')
  const [ed, setEd] = useState<EditorT | null>(null)
  const deco = useRef<ReturnType<EditorT['createDecorationsCollection']> | null>(null)
  const q = useQuery({
    queryKey: ['debug', 'source', p.sessionId, p.path ?? `ref:${p.sourceReference}`],
    queryFn: async () => (p.path ? (await debugApi.file(p.projectId, p.sessionId, p.path)).content : (await debugApi.source(p.projectId, p.sessionId, p.sourceReference!)).content),
    staleTime: Infinity,
    retry: false,
  })
  const s = useDebug((st) => st.sessions[p.sessionId])
  const stack = useDebug((st) => st.stacks[p.sessionId])
  const frameIndex = useDebug((st) => st.selection[p.sessionId]?.frameIndex ?? 0)

  useEffect(() => {
    let alive = true
    void import('@/lib/monacoSetup').then((m) => alive && setLang(m.languageFor(p.path ?? p.name ?? '')))
    return () => {
      alive = false
    }
  }, [p.path, p.name])

  // The execution point and the selected frame, while the session is suspended here.
  const lines = useMemo(() => sourceLines(p, s, stack, frameIndex), [p, s, stack, frameIndex])
  useEffect(() => {
    if (!ed || q.data === undefined) return
    deco.current ??= ed.createDecorationsCollection()
    const max = ed.getModel()?.getLineCount() ?? 0
    deco.current.set(
      lines
        .filter((l) => l.line >= 1 && l.line <= max)
        .map((l) => ({
          range: { startLineNumber: l.line, startColumn: 1, endLineNumber: l.line, endColumn: 1 },
          options: {
            isWholeLine: true,
            className: l.kind === 'exec' ? 'wb-dbg-exec-line' : 'wb-dbg-frame-line',
            glyphMarginClassName: `wb-dbg-glyph ${l.kind}`,
            glyphMarginHoverMessage: { value: l.kind === 'exec' ? 'Execution point' : 'Selected frame' },
          },
        })),
    )
  }, [ed, lines, q.data])

  // Navigate to the frame's line (again whenever `t` changes).
  useEffect(() => {
    if (!ed || q.data === undefined || !p.line) return
    ed.revealLineInCenter(p.line)
    ed.setPosition({ lineNumber: p.line, column: p.column ?? 1 })
  }, [ed, q.data, p.line, p.column, p.t])

  const options = useMemo(
    () => ({
      automaticLayout: true,
      readOnly: true,
      readOnlyMessage: { value: 'Files outside the project are read-only' },
      minimap: { enabled: false },
      fontSize,
      fontFamily: cssVar('--font-mono', 'monospace'),
      lineNumbersMinChars: 4,
      lineDecorationsWidth: 12,
      glyphMargin: true,
      scrollBeyondLastLine: false,
      fixedOverflowWidgets: true,
      stickyScroll: { enabled: true },
      padding: { top: 4 },
      scrollbar: { verticalScrollbarSize: 10, horizontalScrollbarSize: 10 },
      unicodeHighlight: { ambiguousCharacters: false },
    }),
    [fontSize],
  )

  const where = p.path ?? p.name ?? `source ${p.sourceReference}`
  return (
    <div className="wb-fill wb-dbg-src">
      <div className="wb-dbg-src-bar">
        <FileCode size={14} className="wb-muted" />
        <span className="wb-ellipsis wb-dbg-src-path" title={where}>
          {where}
        </span>
        <span className="wb-dbg-src-note" title={p.path ? 'Outside the project: shown read-only for the debug session' : 'Source the debugger provides'}>
          <Lock size={12} />
          {p.path ? 'read-only, outside the project' : 'read-only, from the debugger'}
        </span>
      </div>
      <div className="wb-dbg-src-body">
        {q.isLoading ? (
          <Loading label="Loading source…" />
        ) : q.isError ? (
          <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
        ) : (
          <MonacoEditor
            theme={theme === 'dark' ? 'workbench-dark' : 'workbench-light'}
            path={sourceModelPath(p)}
            value={q.data ?? ''}
            language={lang}
            options={options}
            onMount={(e) => {
              deco.current = null
              setEd(e)
            }}
            loading={<Loading label="Loading editor…" />}
          />
        )}
      </div>
    </div>
  )
}
