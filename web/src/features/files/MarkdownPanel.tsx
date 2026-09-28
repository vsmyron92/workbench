// The `markdown` panel: a live-updating rendered view of a Markdown file. It
// follows the disk (agents writing plans and reports) and, while the file is
// open in an editor here, its unsaved text.

import { useEffect, useState } from 'react'
import { FileDown, FileX, Pencil, RefreshCw } from 'lucide-react'
import { ApiError } from '@/api/client'
import type { PanelProps } from '@/shell/types'
import { Button, EmptyState, ErrorBox, IconButton, Loading } from '@/ui'
import { filesApi } from './api'
import { getModel, onBufferText, useBuffers } from './buffers'
import { exportAsHtml } from './export/ExportDialog'
import { useFileChanged } from './hooks'
import { MarkdownView } from './MarkdownView'
import { openFile } from './openers'
import { basename, bufferKey } from './paths'

export interface MarkdownParams {
  projectId: string | null
  path: string
}

export function MarkdownPanel({ params, setTitle, close }: PanelProps<MarkdownParams>) {
  const projectId = params.projectId ?? null
  const path = params.path
  const key = bufferKey(projectId, path)
  const [disk, setDisk] = useState<{ text?: string; error?: unknown }>({})
  const [nonce, setNonce] = useState(0)
  const [live, setLive] = useState<string | null>(null)
  const hasBuffer = useBuffers((s) => !!s.buffers[key])

  useEffect(() => setTitle(`${basename(path)} (preview)`), [path, setTitle])

  useEffect(() => {
    let cancelled = false
    filesApi.read(projectId, path).then(
      (r) => {
        if (cancelled) return
        if (r.sensitive && r.content === null) setDisk({ error: new Error('This file is marked sensitive; open it in the editor to reveal it.') })
        else if (r.content === null) setDisk({ error: new Error(r.tooLarge ? 'The file is too large to preview.' : 'Not a text file.') })
        else setDisk({ text: r.content })
      },
      (error) => !cancelled && setDisk({ error }),
    )
    return () => {
      cancelled = true
    }
  }, [projectId, path, nonce])

  useFileChanged(projectId, path, () => setNonce((n) => n + 1))

  // Follow the editor buffer while one is open.
  useEffect(() => {
    if (!hasBuffer) {
      setLive(null)
      return
    }
    const m = getModel(key)
    if (m) setLive(m.getValue())
    return onBufferText(key, setLive)
  }, [hasBuffer, key])

  const text = live ?? disk.text
  const missing = disk.error instanceof ApiError && disk.error.status === 404
  return (
    <div className="wb-fill">
      <div className="wb-editor-bar">
        <span className="wb-editor-crumbs wb-ellipsis" title={path}>
          {path}
        </span>
        {live !== null && <span className="wb-badge accent">live</span>}
        <span style={{ flex: 1 }} />
        <IconButton icon={RefreshCw} size="small" label="Reload" onClick={() => setNonce((n) => n + 1)} />
        <IconButton icon={FileDown} size="small" label="Export as HTML…" disabled={text === undefined} onClick={() => exportAsHtml(projectId, path)} />
        <IconButton icon={Pencil} size="small" label="Edit source" onClick={() => openFile({ projectId, path, mode: 'edit' })} />
      </div>
      <div className="wb-scroll wb-md-scroll">
        {text !== undefined ? (
          <MarkdownView projectId={projectId} path={path} text={text} page />
        ) : missing ? (
          <EmptyState icon={FileX} title="File not found" action={<Button size="small" onClick={close}>Close</Button>}>
            {path}
          </EmptyState>
        ) : disk.error ? (
          <ErrorBox error={disk.error} onRetry={() => setNonce((n) => n + 1)} />
        ) : (
          <Loading />
        )}
      </div>
    </div>
  )
}
