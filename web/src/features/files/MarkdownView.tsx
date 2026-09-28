// Rendered Markdown of a file: relative links open files (Markdown as a page,
// the rest in the editor, `#L42` at that line), `#anchors` scroll to headings,
// and relative images load through the raw endpoint. As a `page` it reads like
// a web page: a centred column with an "On this page" outline beside it.

import { useCallback, useEffect, useRef, useState, type RefObject } from 'react'
import { Markdown } from '@/ui'
import { findAnchor } from '@/ui/markdownPlugins'
import { filesApi } from './api'
import { openFile } from './openers'
import { isExternalHref, isMarkdown, resolveLink, splitAnchor } from './paths'
import { slugify } from './text'

export function MarkdownView({
  projectId,
  path,
  text,
  className,
  page,
}: {
  projectId: string | null
  path: string
  text: string
  className?: string
  /** Full-width reading layout with an outline (Read mode, the preview panel). */
  page?: boolean
}) {
  const ref = useRef<HTMLDivElement>(null)

  const scrollToAnchor = useCallback((anchor: string) => {
    const root = ref.current
    if (!root) return
    let el = findAnchor(root, anchor)
    if (!el) {
      // Hand-written anchors that don't follow GitHub's slugs.
      const want = slugify(safeDecode(anchor))
      el = Array.from(root.querySelectorAll('h1, h2, h3, h4, h5, h6')).find((h) => slugify(h.textContent ?? '') === want) ?? null
    }
    el?.scrollIntoView({ block: 'start', behavior: 'smooth' })
  }, [])

  const onLinkClick = useCallback(
    (href: string) => {
      if (href.startsWith('#')) {
        scrollToAnchor(href.slice(1))
        return true
      }
      if (isExternalHref(href)) return false
      const { path: target, line, anchor } = splitAnchor(href)
      const resolved = target ? resolveLink(path, target) : path
      if (resolved === null) return true
      if (resolved === path && anchor) {
        scrollToAnchor(anchor)
        return true
      }
      if (isMarkdown(resolved) && !line) openFile({ projectId, path: resolved, mode: 'read' })
      else openFile({ projectId, path: resolved, line })
      return true
    },
    [path, projectId, scrollToAnchor],
  )

  const resolveImage = useCallback(
    (src: string) => {
      if (!src || isExternalHref(src) || src.startsWith('data:')) return src
      const resolved = resolveLink(path, splitAnchor(src).path)
      return resolved === null ? '' : filesApi.rawUrl(projectId, resolved)
    },
    [path, projectId],
  )

  const outline = useOutline(ref, !!page)

  if (!page) {
    return (
      <div ref={ref} className={['wb-md-view', className].filter(Boolean).join(' ')}>
        <Markdown text={text} onLinkClick={onLinkClick} resolveImage={resolveImage} />
      </div>
    )
  }
  return (
    <div className={['wb-md-view', 'wb-md-reader', className].filter(Boolean).join(' ')}>
      <div className="wb-md-reader-grid">
        <div ref={ref} className="wb-md-page">
          <Markdown text={text} onLinkClick={onLinkClick} resolveImage={resolveImage} />
        </div>
        {outline.items.length > 2 && (
          <nav className="wb-md-toc" aria-label="On this page">
            <div className="wb-md-toc-title">On this page</div>
            {outline.items.map((h) => (
              <a
                key={h.id}
                href={`#${h.id}`}
                className={`wb-md-toc-item l${h.level}${h.id === outline.current ? ' current' : ''}`}
                onClick={(e) => {
                  e.preventDefault()
                  byId(ref.current, h.id)?.scrollIntoView({ block: 'start', behavior: 'smooth' })
                }}
              >
                {h.text}
              </a>
            ))}
          </nav>
        )}
      </div>
    </div>
  )
}

/** By id within this view (the same file may be open twice). */
function byId(root: HTMLElement | null, id: string) {
  return root?.querySelector(`#${CSS.escape(id)}`) ?? null
}

function safeDecode(s: string): string {
  try {
    return decodeURIComponent(s)
  } catch {
    return s
  }
}

interface OutlineItem {
  id: string
  text: string
  level: number
}

/**
 * The document's h1–h3 (read from the rendered DOM, which the lazily loaded
 * renderer fills in later) and the one last scrolled past.
 */
function useOutline(ref: RefObject<HTMLDivElement | null>, enabled: boolean) {
  const [items, setItems] = useState<OutlineItem[]>([])
  const [current, setCurrent] = useState<string | null>(null)

  useEffect(() => {
    const root = ref.current
    if (!enabled || !root) {
      setItems([])
      return
    }
    let frame = 0
    const read = () => {
      frame = 0
      const next = Array.from(root.querySelectorAll<HTMLElement>('h1[id], h2[id], h3[id]')).map((h) => ({
        id: h.id,
        text: (h.textContent ?? '').trim(),
        level: Number(h.tagName.slice(1)),
      }))
      setItems((prev) => (sameOutline(prev, next) ? prev : next))
    }
    read()
    const mo = new MutationObserver(() => {
      if (!frame) frame = requestAnimationFrame(read)
    })
    mo.observe(root, { childList: true, subtree: true, characterData: true })
    return () => {
      mo.disconnect()
      if (frame) cancelAnimationFrame(frame)
    }
  }, [ref, enabled])

  useEffect(() => {
    const root = ref.current
    if (!enabled || !root || items.length === 0) return
    let frame = 0
    const onScroll = (e: Event) => {
      const scroller = e.target
      if (!(scroller instanceof Element) || !scroller.contains(root) || frame) return
      frame = requestAnimationFrame(() => {
        frame = 0
        // A heading counts once it is in the top part of the view (near the end
        // of a document the last ones never reach the very top).
        const box = scroller.getBoundingClientRect()
        const top = box.top + Math.min(120, box.height / 4)
        let id: string | null = null
        for (const h of items) {
          const el = byId(root, h.id)
          if (el && el.getBoundingClientRect().top <= top) id = h.id
          else break
        }
        setCurrent(id)
      })
    }
    // Capture: scroll events don't bubble, and the scroller is an ancestor.
    document.addEventListener('scroll', onScroll, true)
    return () => {
      document.removeEventListener('scroll', onScroll, true)
      if (frame) cancelAnimationFrame(frame)
    }
  }, [ref, enabled, items])

  return { items, current }
}

function sameOutline(a: OutlineItem[], b: OutlineItem[]) {
  return a.length === b.length && a.every((x, i) => x.id === b[i].id && x.text === b[i].text && x.level === b[i].level)
}
