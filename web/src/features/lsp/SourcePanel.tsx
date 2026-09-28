// The `lsp.source` panel: a read-only view of a file outside the project that a
// language server pointed to (the Rust standard library, ~/.cargo/registry,
// node_modules, site-packages, a file only in the dev container). Its model is
// `lsp-src://<projectId>/<path>`, synchronized with the server like any document, so
// hover, go to declaration and usages keep working inside it.

import { useEffect, useMemo, useRef, useState } from 'react'
import type { editor } from 'monaco-editor'
import { useQuery } from '@tanstack/react-query'
import { Copy, Library, Lock } from 'lucide-react'
import { toast } from '@/shell/actions'
import type { PanelProps } from '@/shell/types'
import { useUi } from '@/state/store'
import { cssVar } from '@/theme/palette'
import { EmptyState, ErrorBox, IconButton, Loading, MonacoEditor } from '@/ui'
import { lspApi } from './api'
import { lsp } from './client'
import { displayPath, shortenPath } from './convert'
import type { SourceParams } from './nav'
import { acquireSource, releaseSource } from './sourceModels'

export function SourcePanel({ params, setTitle, active }: PanelProps<SourceParams>) {
  const { projectId, uri } = params
  const path = displayPath(uri)
  const theme = useUi((s) => s.prefs.theme)
  const fontSize = useUi((s) => s.prefs.editorFontSize)
  const [ed, setEd] = useState<editor.IStandaloneCodeEditor | null>(null)
  const [model, setModel] = useState<editor.ITextModel | null>(null)
  const src = useQuery({ queryKey: ['lsp', 'source', projectId, uri], queryFn: () => lspApi.source(projectId, uri), staleTime: Infinity, retry: false })

  useEffect(() => {
    setTitle(path.slice(path.lastIndexOf('/') + 1))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path])

  // The model: created once the text is here, shared, released on unmount.
  useEffect(() => {
    if (!src.data) return
    let cancelled = false
    let got = false
    void (async () => {
      const { monaco, languageFor } = await import('@/lib/monacoSetup')
      await lsp.install()
      if (cancelled) return
      const m = acquireSource(monaco, uri, src.data.content, languageFor)
      got = true
      setModel(m)
    })()
    return () => {
      cancelled = true
      if (got) releaseSource(uri)
    }
  }, [src.data, uri, path])

  useEffect(() => {
    if (ed && model) ed.setModel(model)
  }, [ed, model])

  // Reveal the location on every navigation (`t` changes).
  const lineKey = `${params.line ?? ''}:${params.column ?? ''}:${params.t ?? ''}`
  const revealed = useRef('')
  useEffect(() => {
    if (!ed || !model || !params.line || revealed.current === lineKey) return
    revealed.current = lineKey
    const line = Math.min(Math.max(1, params.line), model.getLineCount())
    const col = Math.max(1, params.column ?? 1)
    const end = params.endColumn && params.endColumn > col ? params.endColumn : col
    ed.setSelection({ startLineNumber: line, startColumn: col, endLineNumber: line, endColumn: end })
    ed.revealLineInCenter(line)
    ed.focus()
  }, [ed, model, lineKey, params.line, params.column, params.endColumn])

  useEffect(() => {
    if (active && ed) ed.focus()
  }, [active, ed])

  const options = useMemo<editor.IStandaloneEditorConstructionOptions>(
    () => ({
      automaticLayout: true,
      readOnly: true,
      readOnlyMessage: { value: 'Library source outside the project: read-only' },
      minimap: { enabled: false },
      fontSize,
      fontFamily: cssVar('--font-mono', 'monospace'),
      scrollBeyondLastLine: false,
      fixedOverflowWidgets: true,
      stickyScroll: { enabled: true },
      padding: { top: 4 },
      scrollbar: { verticalScrollbarSize: 10, horizontalScrollbarSize: 10 },
      unicodeHighlight: { ambiguousCharacters: false },
    }),
    [fontSize],
  )

  if (src.error) {
    return (
      <div className="wb-pad">
        <ErrorBox error={src.error} onRetry={() => void src.refetch()} />
      </div>
    )
  }
  if (!src.data) return <Loading label={`Opening ${path.slice(path.lastIndexOf('/') + 1)}…`} />
  if (!projectId) return <EmptyState title="No project" />
  return (
    <div className="wb-fill lsp-source">
      <div className="lsp-source-bar">
        <Library size={13} className="wb-subtle" />
        <span className="wb-ellipsis lsp-source-path" title={path}>
          {shortenPath(path)}
        </span>
        <span className="wb-grow" />
        <span className="lsp-source-badge">
          <Lock size={11} /> Read-only library source
        </span>
        <IconButton
          icon={Copy}
          size="small"
          label="Copy path"
          onClick={() => {
            void navigator.clipboard?.writeText(path).then(
              () => toast('success', 'Path copied', { timeout: 1500 }),
              () => toast('error', 'Could not copy'),
            )
          }}
        />
      </div>
      <div className="lsp-source-editor">
        <MonacoEditor
          theme={theme === 'dark' ? 'workbench-dark' : 'workbench-light'}
          options={options}
          keepCurrentModel
          onMount={(e) => {
            const initial = e.getModel()
            setEd(e)
            if (initial) queueMicrotask(() => !initial.isDisposed() && initial.uri.scheme === 'inmemory' && initial.dispose())
          }}
        />
      </div>
    </div>
  )
}
