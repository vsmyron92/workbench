// Full-screen image preview: arrow keys browse, Fit / 100%, open original, download.
// (Mr. Mak's report lightbox, as a React overlay.)

import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { ChevronLeft, ChevronRight, Download, ExternalLink, Maximize, Scan, X } from 'lucide-react'
import { IconButton } from '@/ui'

export interface LightboxItem {
  url: string
  name: string
}

export function Lightbox({ items, index, onIndex, onClose }: { items: LightboxItem[]; index: number; onIndex: (i: number) => void; onClose: () => void }) {
  const [actual, setActual] = useState(false)
  const [failed, setFailed] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  const item = items[index]
  const count = items.length
  const go = (d: number) => {
    if (count > 1) onIndex((index + d + count) % count)
  }

  useEffect(() => {
    setActual(false)
    setFailed(false)
  }, [index])

  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null
    ref.current?.focus({ preventScroll: true })
    return () => previous?.focus?.({ preventScroll: true })
  }, [])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopPropagation()
        onClose()
      } else if (e.key === 'ArrowRight') {
        e.preventDefault()
        go(1)
      } else if (e.key === 'ArrowLeft') {
        e.preventDefault()
        go(-1)
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  })

  if (!item) return null
  return createPortal(
    <div ref={ref} className="ws-lightbox" role="dialog" aria-modal="true" aria-label={`Image preview: ${item.name}`} tabIndex={-1}>
      <div className="ws-lightbox-bar">
        <span className="ws-lightbox-caption wb-ellipsis" title={item.name}>
          {item.name}
        </span>
        {count > 1 && (
          <span className="wb-muted wb-small">
            {index + 1} / {count}
          </span>
        )}
        <span className="spacer" />
        {count > 1 && (
          <>
            <IconButton icon={ChevronLeft} label="Previous (←)" onClick={() => go(-1)} />
            <IconButton icon={ChevronRight} label="Next (→)" onClick={() => go(1)} />
          </>
        )}
        <IconButton icon={actual ? Maximize : Scan} label={actual ? 'Fit to window' : 'Actual size (100%)'} active={actual} onClick={() => setActual(!actual)} />
        <a className="wb-icon-btn" href={item.url} target="_blank" rel="noopener noreferrer" title="Open original" aria-label="Open original">
          <ExternalLink size={16} />
        </a>
        <a className="wb-icon-btn" href={item.url} download={item.name} title="Download" aria-label="Download">
          <Download size={16} />
        </a>
        <IconButton icon={X} label="Close (Esc)" onClick={onClose} />
      </div>
      <div className={`ws-lightbox-stage${actual ? ' actual' : ''}`} onClick={(e) => e.target === e.currentTarget && onClose()}>
        {failed ? (
          <span className="wb-muted">This image could not be loaded.</span>
        ) : (
          <img key={item.url} src={item.url} alt={item.name} onClick={() => setActual(!actual)} onError={() => setFailed(true)} />
        )}
      </div>
      <div className="ws-lightbox-note">{count > 1 ? 'Arrow keys to browse · Esc to close' : 'Esc to close'}</div>
    </div>,
    document.body,
  )
}
