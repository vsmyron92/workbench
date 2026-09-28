// Rendered Confluence HTML. The server already sanitized it (ammonia) and rewrote
// images to the attachment proxy and internal links to `data-wb-page`; DOMPurify runs
// again here as defence in depth. Clicks on internal links open Workbench panels.
//
// Inline comments: highlights are coloured by their comment's state (open, resolved
// ones not highlighted, the active one stronger), and selecting text offers a
// "Comment" button that anchors a new inline comment to that passage.

import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState, type MouseEvent } from 'react'
import DOMPurify from 'dompurify'
import { MessageSquarePlus } from 'lucide-react'
import { toast } from '@/shell/actions'
import { openConfluencePage, openJiraIssue } from './actions'
import { anchorOf, type Anchor } from './selection'

export interface PageViewHandle {
  scrollToAnchor: (name: string) => boolean
  flashMarker: (ref: string) => boolean
  /** The current text selection as an inline-comment anchor (or why it cannot be one). */
  selectionAnchor: () => Anchor | { error: string }
  /** Keep a selection visibly highlighted while its comment is written. */
  holdSelection: (on: boolean) => void
}

const PURIFY = {
  FORBID_TAGS: ['style', 'script', 'form', 'input', 'button', 'iframe', 'object', 'embed'],
  FORBID_ATTR: ['onerror', 'onload', 'onclick'],
  ALLOW_DATA_ATTR: true,
}

export function sanitize(html: string): string {
  return DOMPurify.sanitize(html, PURIFY) as unknown as string
}

function flash(el: Element) {
  el.classList.remove('flash-target')
  // Restart the animation.
  void (el as HTMLElement).offsetWidth
  el.classList.add('flash-target')
  window.setTimeout(() => el.classList.remove('flash-target'), 1700)
}

/** Block elements an inline comment's text must stay within. */
const BLOCKS = 'p, li, td, th, h1, h2, h3, h4, h5, h6, blockquote, dd, dt, figcaption, caption'
const PENDING_HIGHLIGHT = 'wb-cf-pending'

type MarkerState = 'open' | 'resolved'

interface Props {
  html: string
  pageId: string
  onMarkerClick?: (ref: string) => void
  className?: string
  /** State of each marker's comment (by marker ref); unknown markers stay highlighted. */
  markers?: Record<string, MarkerState>
  activeRef?: string | null
  /** Offer "Comment" on a text selection. */
  onComment?: (anchor: Anchor) => void
}

export const PageView = forwardRef<PageViewHandle, Props>(function PageView({ html, pageId, onMarkerClick, className, markers, activeRef, onComment }, ref) {
  const root = useRef<HTMLDivElement>(null)
  const clean = useMemo(() => sanitize(html), [html])
  const [offer, setOffer] = useState<{ left: number; top: number } | null>(null)
  const held = useRef<Range | null>(null)

  const scrollToAnchor = (name: string) => {
    const el = root.current?.querySelector(`#${CSS.escape('cf-' + name)}`) ?? root.current?.querySelector(`#${CSS.escape(name)}`)
    if (!el) return false
    el.scrollIntoView({ behavior: 'smooth', block: 'start' })
    flash(el)
    return true
  }

  const currentRange = (): Range | null => {
    const sel = window.getSelection()
    if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return null
    const r = sel.getRangeAt(0)
    return root.current && root.current.contains(r.commonAncestorContainer) ? r : null
  }

  const anchorFor = useCallback((r: Range | null): Anchor | { error: string } => {
    const el = root.current
    if (!el || !r) return { error: 'Select text in the page first' }
    const blockOf = (n: Node) => (n.nodeType === Node.ELEMENT_NODE ? (n as Element) : n.parentElement)?.closest(BLOCKS)
    const a = blockOf(r.startContainer)
    if (!a || a !== blockOf(r.endContainer) || !el.contains(a)) return { error: 'Select text within one paragraph, list item or table cell' }
    if ((r.commonAncestorContainer.nodeType === Node.ELEMENT_NODE ? (r.commonAncestorContainer as Element) : r.commonAncestorContainer.parentElement)?.closest('pre, code')) {
      return { error: 'Code cannot carry inline comments' }
    }
    const before = document.createRange()
    before.selectNodeContents(el)
    before.setEnd(r.startContainer, r.startOffset)
    return anchorOf(el.textContent ?? '', r.toString(), before.toString().length)
  }, [])

  const setHighlight = (r: Range | null) => {
    const reg = (globalThis.CSS as unknown as { highlights?: Map<string, unknown> }).highlights
    const H = (globalThis as unknown as { Highlight?: new (...r: Range[]) => unknown }).Highlight
    if (!reg || !H) return
    if (r) reg.set(PENDING_HIGHLIGHT, new H(r))
    else reg.delete(PENDING_HIGHLIGHT)
  }

  useImperativeHandle(ref, () => ({
    scrollToAnchor,
    flashMarker: (markerRef: string) => {
      const el = root.current?.querySelector(`.inline-comment-marker[data-ref="${CSS.escape(markerRef)}"]`)
      if (!el) return false
      el.scrollIntoView({ behavior: 'smooth', block: 'center' })
      el.classList.add('flash')
      window.setTimeout(() => el.classList.remove('flash'), 1600)
      return true
    },
    selectionAnchor: () => anchorFor(currentRange()),
    holdSelection: (on: boolean) => {
      held.current = on ? (currentRange()?.cloneRange() ?? held.current) : null
      setHighlight(held.current)
    },
  }))

  // Drop the held highlight with the page (a new version renders new nodes).
  useEffect(() => () => setHighlight(null), [clean])

  // Colour highlights by their comment's state; the active one stands out.
  useEffect(() => {
    const els = root.current?.querySelectorAll<HTMLElement>('.inline-comment-marker[data-ref]') ?? []
    for (const el of els) {
      const r = el.dataset.ref ?? ''
      el.classList.toggle('resolved', markers?.[r] === 'resolved')
      el.classList.toggle('active', !!activeRef && r === activeRef)
    }
  }, [clean, markers, activeRef])

  // Offer "Comment" next to a finished selection.
  const onMouseUp = () => {
    if (!onComment) return
    window.setTimeout(() => {
      const r = currentRange()
      // The button is positioned in the scrolling page area (`.cf-main`, position: relative).
      const box = root.current?.closest<HTMLElement>('.cf-main')
      if (!r || !box || !r.toString().trim()) return setOffer(null)
      const rects = r.getClientRects()
      const last = rects[rects.length - 1] ?? r.getBoundingClientRect()
      const b = box.getBoundingClientRect()
      setOffer({ left: Math.min(last.right - b.left + box.scrollLeft + 4, box.clientWidth - 110 + box.scrollLeft), top: last.bottom - b.top + box.scrollTop + 4 })
    }, 0)
  }

  useEffect(() => {
    if (!offer) return
    const onSel = () => {
      if (!currentRange()) setOffer(null)
    }
    document.addEventListener('selectionchange', onSel)
    return () => document.removeEventListener('selectionchange', onSel)
  }, [offer])

  const comment = () => {
    const a = anchorFor(currentRange())
    if ('error' in a) return toast('warning', a.error)
    held.current = currentRange()?.cloneRange() ?? null
    setHighlight(held.current)
    setOffer(null)
    onComment?.(a)
  }

  const onClick = (e: MouseEvent) => {
    const target = e.target as HTMLElement
    const marker = target.closest<HTMLElement>('.inline-comment-marker')
    const a = target.closest<HTMLAnchorElement>('a')
    if (!a) {
      // A click that ends a selection is not a click on the highlight.
      if (marker?.dataset.ref && onMarkerClick && !window.getSelection()?.toString()) onMarkerClick(marker.dataset.ref)
      const img = target.closest('img')
      if (img?.src) window.open(img.src, '_blank', 'noopener,noreferrer')
      return
    }
    const href = a.getAttribute('href') ?? ''
    const page = a.dataset.wbPage
    const issue = a.dataset.wbIssue
    // Ctrl/Cmd-click keeps the browser behaviour (open the site's URL in a new tab).
    if ((e.ctrlKey || e.metaKey) && href && !href.startsWith('#')) {
      e.preventDefault()
      window.open(href, '_blank', 'noopener,noreferrer')
      return
    }
    e.preventDefault()
    if (page) {
      const anchor = a.dataset.wbAnchor
      if (page === pageId && anchor) scrollToAnchor(decodeURIComponent(anchor))
      else openConfluencePage(page, a.textContent?.trim() || undefined)
      return
    }
    if (issue) {
      openJiraIssue(issue)
      return
    }
    if (href.startsWith('#')) {
      scrollToAnchor(decodeURIComponent(href.slice(1)))
      return
    }
    if (href) window.open(href, '_blank', 'noopener,noreferrer')
  }

  return (
    <>
      <div
        ref={root}
        className={['wb-prose', 'wb-cf', className].filter(Boolean).join(' ')}
        onClick={onClick}
        onMouseUp={onMouseUp}
        onKeyUp={(e) => e.shiftKey && onMouseUp()}
        dangerouslySetInnerHTML={{ __html: clean }}
      />
      {offer && (
        <button
          className="cf-comment-offer"
          style={{ left: offer.left, top: offer.top }}
          onMouseDown={(e) => e.preventDefault()}
          onClick={comment}
          title="Comment on the selected text (Ctrl+Alt+C)"
        >
          <MessageSquarePlus size={13} /> Comment
        </button>
      )}
    </>
  )
})
