// Fixed-row-height virtual list: renders only the rows in view (plus overscan),
// so trees and result lists with tens of thousands of rows stay fast.

import { forwardRef, useCallback, useEffect, useImperativeHandle, useLayoutEffect, useRef, useState, type HTMLAttributes, type ReactNode } from 'react'

export interface VirtualListHandle {
  scrollToIndex: (index: number, align?: 'auto' | 'center' | 'start') => void
  element: () => HTMLDivElement | null
}

interface Props extends Omit<HTMLAttributes<HTMLDivElement>, 'children'> {
  count: number
  rowHeight: number
  renderRow: (index: number) => ReactNode
  overscan?: number
  /** Rendered after the rows (e.g. a drop zone or a "truncated" note). */
  footer?: ReactNode
}

export const VirtualList = forwardRef<VirtualListHandle, Props>(function VirtualList(
  { count, rowHeight, renderRow, overscan = 8, footer, className, ...rest },
  ref,
) {
  const el = useRef<HTMLDivElement>(null)
  const [scrollTop, setScrollTop] = useState(0)
  const [height, setHeight] = useState(0)

  useLayoutEffect(() => {
    const node = el.current
    if (!node) return
    setHeight(node.clientHeight)
    const ro = new ResizeObserver(() => setHeight(node.clientHeight))
    ro.observe(node)
    return () => ro.disconnect()
  }, [])

  // Keep the scroll position valid when the list shrinks.
  useEffect(() => {
    const node = el.current
    if (node && scrollTop > Math.max(0, count * rowHeight - node.clientHeight)) {
      setScrollTop(node.scrollTop)
    }
  }, [count, rowHeight, scrollTop])

  const scrollToIndex = useCallback(
    (index: number, align: 'auto' | 'center' | 'start' = 'auto') => {
      const node = el.current
      if (!node || index < 0) return
      const top = index * rowHeight
      const bottom = top + rowHeight
      if (align === 'start') node.scrollTop = top
      else if (align === 'center') node.scrollTop = Math.max(0, top - node.clientHeight / 2 + rowHeight / 2)
      else if (top < node.scrollTop) node.scrollTop = top
      else if (bottom > node.scrollTop + node.clientHeight) node.scrollTop = bottom - node.clientHeight
    },
    [rowHeight],
  )
  useImperativeHandle(ref, () => ({ scrollToIndex, element: () => el.current }), [scrollToIndex])

  const first = Math.max(0, Math.floor(scrollTop / rowHeight) - overscan)
  const last = Math.min(count, Math.ceil((scrollTop + (height || 800)) / rowHeight) + overscan)
  const rows: ReactNode[] = []
  for (let i = first; i < last; i++) {
    rows.push(
      <div key={i} className="wb-vrow" style={{ top: i * rowHeight, height: rowHeight }}>
        {renderRow(i)}
      </div>,
    )
  }
  return (
    <div ref={el} className={['wb-vlist', className].filter(Boolean).join(' ')} onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)} {...rest}>
      <div className="wb-vlist-inner" style={{ height: count * rowHeight }}>
        {rows}
      </div>
      {footer}
    </div>
  )
})
