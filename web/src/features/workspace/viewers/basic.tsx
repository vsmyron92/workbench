// Step viewers for reports (sandboxed iframe), images, media, PDFs, text and
// anything else (download).

import { useEffect, useMemo, useRef, useState } from 'react'
import { Download, ExternalLink, FileQuestion, FileWarning, Maximize, RefreshCw, Scan } from 'lucide-react'
import { EmptyState, ErrorBox, formatBytes, IconButton, Loading, Spinner, Toolbar } from '@/ui'
import { useCardText, type WorkspaceCard } from '../api'
import { basename, externalLinkTarget } from '../logic'
import { Lightbox } from './Lightbox'

/** The sandbox for untrusted report HTML: scripts run, but in an opaque origin
 *  (no allow-same-origin), so they never reach Workbench's cookies, API or DOM.
 *  Popups stay sandboxed too (no allow-popups-to-escape-sandbox): the report's
 *  external links reach us as messages instead (see `HtmlView`). */
export const REPORT_SANDBOX = 'allow-scripts allow-popups allow-downloads'

/** What the report prelude (server/src/workspace/view.rs) posts for an external link. */
const OPEN_LINK = 'workbench:open-link'

export function OpenButtons({ url, name }: { url: string; name: string }) {
  return (
    <>
      <a className="wb-icon-btn small" href={url} target="_blank" rel="noopener noreferrer" title="Open in a new tab" aria-label="Open in a new tab">
        <ExternalLink size={14} />
      </a>
      <a className="wb-icon-btn small" href={url} download={name} title="Download" aria-label="Download">
        <Download size={14} />
      </a>
    </>
  )
}

/**
 * A report in a sandboxed iframe, kept hidden until it has loaded and painted
 * (two animation frames), so there is no white flash (a Mr. Mak lesson).
 */
export function HtmlView({ url, title, label, reloadKey }: { url: string; title: string; label: string; reloadKey?: number }) {
  const [loaded, setLoaded] = useState(false)
  const [nonce, setNonce] = useState(0)
  const raf = useRef(0)
  const frameRef = useRef<HTMLIFrameElement>(null)
  const src = nonce ? `${url}${url.includes('?') ? '&' : '?'}r=${nonce}` : url
  useEffect(() => {
    setLoaded(false)
    return () => cancelAnimationFrame(raf.current)
  }, [src, reloadKey])

  // A click on an external link in the report: open it here, as a normal tab with
  // no way back to us. The click's user activation reaches this (parent) frame.
  useEffect(() => {
    const onMessage = (e: MessageEvent) => {
      if (!frameRef.current || e.source !== frameRef.current.contentWindow) return
      const d = e.data as { type?: unknown; href?: unknown } | null
      if (!d || typeof d !== 'object' || d.type !== OPEN_LINK) return
      const target = externalLinkTarget(d.href, location)
      if (target) window.open(target, '_blank', 'noopener,noreferrer')
    }
    window.addEventListener('message', onMessage)
    return () => window.removeEventListener('message', onMessage)
  }, [])
  return (
    <div className="wb-fill">
      <Toolbar>
        <span className="wb-small wb-muted wb-ellipsis" style={{ padding: '0 4px' }}>
          {label}
        </span>
        <span className="spacer" />
        <IconButton icon={RefreshCw} size="small" label="Reload" onClick={() => setNonce((n) => n + 1)} />
        <OpenButtons url={url} name={basename(url.split('?')[0])} />
      </Toolbar>
      <div className="ws-html">
        <iframe
          key={`${src}#${reloadKey ?? 0}`}
          ref={frameRef}
          src={src}
          title={title}
          sandbox={REPORT_SANDBOX}
          allow="fullscreen"
          referrerPolicy="no-referrer"
          className={loaded ? 'loaded' : ''}
          onLoad={() => {
            cancelAnimationFrame(raf.current)
            raf.current = requestAnimationFrame(() => {
              raf.current = requestAnimationFrame(() => setLoaded(true))
            })
          }}
        />
        {!loaded && (
          <div className="ws-overlay" role="status">
            <Spinner size={18} />
            <span>Opening report…</span>
          </div>
        )}
      </div>
    </div>
  )
}

export function ImageView({ url, name }: { url: string; name: string }) {
  const [actual, setActual] = useState(false)
  const [dims, setDims] = useState<{ w: number; h: number } | null>(null)
  const [failed, setFailed] = useState(false)
  const [open, setOpen] = useState(false)
  useEffect(() => {
    setFailed(false)
    setDims(null)
  }, [url])
  return (
    <div className="wb-fill">
      <Toolbar>
        <IconButton icon={Maximize} size="small" label="Fit" active={!actual} onClick={() => setActual(false)} />
        <IconButton icon={Scan} size="small" label="Actual size (100%)" active={actual} onClick={() => setActual(true)} />
        <span className="wb-small wb-muted" style={{ marginLeft: 6 }}>
          {dims ? `${dims.w} × ${dims.h}` : ''}
        </span>
        <span className="spacer" />
        <OpenButtons url={url} name={name} />
      </Toolbar>
      <div className={`ws-image-stage${actual ? ' actual' : ''}`}>
        {failed ? (
          <EmptyState icon={FileWarning} title="This image could not be loaded" />
        ) : (
          <img
            src={url}
            alt={name}
            draggable={false}
            onClick={() => setOpen(true)}
            onLoad={(e) => setDims({ w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight })}
            onError={() => setFailed(true)}
          />
        )}
      </div>
      {open && <Lightbox items={[{ url, name }]} index={0} onIndex={() => {}} onClose={() => setOpen(false)} />}
    </div>
  )
}

export function MediaView({ url, name, kind }: { url: string; name: string; kind: 'video' | 'audio' }) {
  return (
    <div className="wb-fill">
      <Toolbar>
        <span className="wb-small wb-muted wb-ellipsis" style={{ padding: '0 4px' }}>
          {name}
        </span>
        <span className="spacer" />
        <OpenButtons url={url} name={name} />
      </Toolbar>
      <div className="ws-media-stage">
        {kind === 'video' ? <video src={url} controls preload="metadata" /> : <audio src={url} controls preload="metadata" />}
      </div>
    </div>
  )
}

/**
 * PDFs render in the browser's own viewer. The browser refuses plugins in a
 * sandboxed document, so the file is fetched through its grant and shown from a
 * blob URL (the bytes are a PDF, not script, so nothing runs in our origin).
 */
export function PdfView({ url, name }: { url: string; name: string }) {
  const [blobUrl, setBlobUrl] = useState<string | null>(null)
  const [error, setError] = useState<unknown>(null)
  useEffect(() => {
    const ctl = new AbortController()
    let made: string | null = null
    setBlobUrl(null)
    setError(null)
    fetch(url, { signal: ctl.signal, credentials: 'omit' })
      .then((r) => (r.ok ? r.arrayBuffer() : Promise.reject(new Error(`The PDF could not be loaded (HTTP ${r.status})`))))
      .then((buf) => {
        made = URL.createObjectURL(new Blob([buf], { type: 'application/pdf' }))
        setBlobUrl(made)
      })
      .catch((e) => !ctl.signal.aborted && setError(e))
    return () => {
      ctl.abort()
      if (made) URL.revokeObjectURL(made)
    }
  }, [url])
  return (
    <div className="wb-fill">
      <Toolbar>
        <span className="wb-small wb-muted wb-ellipsis" style={{ padding: '0 4px' }}>
          {name}
        </span>
        <span className="spacer" />
        <OpenButtons url={url} name={name} />
      </Toolbar>
      {error ? <ErrorBox error={error} /> : blobUrl ? <iframe className="ws-pdf" src={blobUrl} title={name} /> : <Loading label="Loading PDF…" />}
    </div>
  )
}

export function TextView({ card, path, name, url }: { card: WorkspaceCard; path: string; name: string; url: string }) {
  const { data, error, isLoading, refetch } = useCardText(card.scope, card.id, path)
  const text = useMemo(() => {
    if (!data) return ''
    if (/\.json$/i.test(path) && data.text.length < 1_000_000) {
      try {
        return JSON.stringify(JSON.parse(data.text), null, 2)
      } catch {
        return data.text
      }
    }
    return data.text
  }, [data, path])
  return (
    <div className="wb-fill">
      <Toolbar>
        <span className="wb-small wb-muted wb-ellipsis" style={{ padding: '0 4px' }}>
          {name}
          {data ? ` · ${formatBytes(data.size)}` : ''}
        </span>
        <span className="spacer" />
        <OpenButtons url={url} name={basename(path)} />
      </Toolbar>
      {error ? <ErrorBox error={error} onRetry={() => void refetch()} /> : isLoading ? <Loading /> : <pre className="ws-text wb-scroll">{text}</pre>}
    </div>
  )
}

export function FileView({ url, name, size }: { url: string; name: string; size?: number }) {
  return (
    <EmptyState
      icon={FileQuestion}
      title="No preview for this file type"
      action={
        <a className="wb-btn small" href={url} download={name}>
          <Download size={13} />
          Download
        </a>
      }
    >
      {name}
      {size !== undefined ? ` · ${formatBytes(size)}` : ''}
    </EmptyState>
  )
}

export function MissingView({ path }: { path: string }) {
  return (
    <EmptyState icon={FileWarning} title="File missing">
      The card lists <code>{path}</code>, but it is not in the card folder (yet).
    </EmptyState>
  )
}
