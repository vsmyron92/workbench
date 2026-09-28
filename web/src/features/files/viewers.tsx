// Non-text viewers: images (zoom / fit), video and audio, PDF (the browser's
// viewer on the raw URL), and notices for binary, too-large and sensitive files.

import { useEffect, useRef, useState } from 'react'
import { Download, ExternalLink, FileQuestion, FileWarning, Maximize, ShieldAlert, ZoomIn, ZoomOut } from 'lucide-react'
import { Button, EmptyState, formatBytes, IconButton, Toolbar } from '@/ui'
import { filesApi } from './api'
import { basename } from './paths'

interface Src {
  projectId: string | null
  path: string
  /** Changes when the file changes on disk (cache-busting). */
  version?: number | string
}

export function ImageViewer({ projectId, path, version }: Src) {
  const [zoom, setZoom] = useState<number | 'fit'>('fit')
  const [dims, setDims] = useState<{ w: number; h: number } | null>(null)
  const [failed, setFailed] = useState(false)
  const url = filesApi.rawUrl(projectId, path, { v: version })
  useEffect(() => setFailed(false), [url])
  const step = (dir: 1 | -1) =>
    setZoom((z) => {
      const cur = z === 'fit' ? 1 : z
      const next = dir > 0 ? cur * 1.25 : cur / 1.25
      return Math.max(0.05, Math.min(16, Math.round(next * 100) / 100))
    })
  return (
    <div className="wb-fill">
      <Toolbar>
        <IconButton icon={ZoomOut} size="small" label="Zoom out" onClick={() => step(-1)} />
        <IconButton icon={ZoomIn} size="small" label="Zoom in" onClick={() => step(1)} />
        <IconButton icon={Maximize} size="small" label="Fit" active={zoom === 'fit'} onClick={() => setZoom('fit')} />
        <Button size="small" variant="ghost" onClick={() => setZoom(1)}>
          1:1
        </Button>
        <span className="wb-small wb-muted" style={{ marginLeft: 6 }}>
          {dims ? `${dims.w} × ${dims.h}` : ''} {zoom === 'fit' ? '' : `· ${Math.round(zoom * 100)}%`}
        </span>
        <span style={{ flex: 1 }} />
        <OpenRaw projectId={projectId} path={path} />
      </Toolbar>
      <div
        className={`wb-image-stage${zoom === 'fit' ? ' fit' : ''}`}
        onWheel={(e) => {
          if (!e.ctrlKey) return
          e.preventDefault()
          step(e.deltaY < 0 ? 1 : -1)
        }}
      >
        {failed ? (
          <EmptyState icon={FileWarning} title="Cannot display this image" />
        ) : (
          <img
            src={url}
            alt={basename(path)}
            draggable={false}
            onLoad={(e) => setDims({ w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight })}
            onError={() => setFailed(true)}
            style={zoom === 'fit' ? undefined : { width: dims ? dims.w * zoom : undefined, maxWidth: 'none', maxHeight: 'none' }}
          />
        )}
      </div>
    </div>
  )
}

export function MediaViewer({ projectId, path, version, kind }: Src & { kind: 'video' | 'audio' }) {
  const url = filesApi.rawUrl(projectId, path, { v: version })
  const ref = useRef<HTMLVideoElement & HTMLAudioElement>(null)
  return (
    <div className="wb-fill">
      <Toolbar>
        <span className="wb-small wb-muted">{basename(path)}</span>
        <span style={{ flex: 1 }} />
        <OpenRaw projectId={projectId} path={path} />
      </Toolbar>
      <div className="wb-media-stage">
        {kind === 'video' ? <video ref={ref} src={url} controls preload="metadata" /> : <audio ref={ref} src={url} controls preload="metadata" />}
      </div>
    </div>
  )
}

export function PdfViewer({ projectId, path, version }: Src) {
  const url = filesApi.rawUrl(projectId, path, { v: version })
  return (
    <div className="wb-fill">
      <iframe className="wb-pdf-frame" src={url} title={basename(path)} />
    </div>
  )
}

function OpenRaw({ projectId, path }: { projectId: string | null; path: string }) {
  return (
    <>
      <IconButton icon={ExternalLink} size="small" label="Open in a browser tab" onClick={() => window.open(filesApi.rawUrl(projectId, path), '_blank', 'noopener')} />
      <IconButton
        icon={Download}
        size="small"
        label="Download"
        onClick={() => {
          const a = document.createElement('a')
          a.href = filesApi.rawUrl(projectId, path, { download: true })
          a.download = basename(path)
          a.click()
        }}
      />
    </>
  )
}

export function BinaryNotice({ projectId, path, size, tooLarge }: { projectId: string | null; path: string; size: number; tooLarge?: boolean }) {
  return (
    <EmptyState
      icon={tooLarge ? FileWarning : FileQuestion}
      title={tooLarge ? 'This file is too large to edit' : 'Binary file'}
      action={
        <div className="wb-row">
          <Button size="small" icon={ExternalLink} onClick={() => window.open(filesApi.rawUrl(projectId, path), '_blank', 'noopener')}>
            Open raw
          </Button>
          <Button
            size="small"
            icon={Download}
            onClick={() => {
              const a = document.createElement('a')
              a.href = filesApi.rawUrl(projectId, path, { download: true })
              a.download = basename(path)
              a.click()
            }}
          >
            Download
          </Button>
        </div>
      }
    >
      {basename(path)} · {formatBytes(size)}
      {tooLarge ? ' (the editor opens files up to 5 MB)' : ''}
    </EmptyState>
  )
}

export function SensitiveNotice({ path, onReveal }: { path: string; onReveal: () => void }) {
  return (
    <EmptyState
      icon={ShieldAlert}
      title="This file is marked sensitive"
      action={
        <Button size="small" onClick={onReveal}>
          Show anyway
        </Button>
      }
    >
      {basename(path)} matches a sensitive pattern (credentials, personal data). Its content is hidden so it does not end up on a
      shared screen or in a screenshot.
    </EmptyState>
  )
}
