// xterm 6 does not scroll on a touch drag (verified in emulated mobile Chrome), so do it
// ourselves: the normal buffer scrolls its history; an alternate-buffer program with
// mouse tracking (a full-screen TUI) gets SGR wheel reports instead.

import type { Terminal } from '@xterm/xterm'

export function attachTouchScroll(term: Terminal, el: HTMLElement): () => void {
  let lastY: number | null = null
  let acc = 0
  const lineHeight = () => (el.querySelector('.xterm-screen')?.clientHeight ?? 0) / term.rows || 16
  const onStart = (e: TouchEvent) => {
    if (e.touches.length === 1) {
      lastY = e.touches[0].clientY
      acc = 0
    }
  }
  const onMove = (e: TouchEvent) => {
    if (lastY === null || e.touches.length !== 1) return
    const y = e.touches[0].clientY
    acc += lastY - y
    lastY = y
    const h = lineHeight()
    const lines = Math.trunc(acc / h)
    if (lines !== 0) {
      acc -= lines * h
      if (term.buffer.active.type === 'alternate' && term.modes.mouseTrackingMode !== 'none') {
        const btn = lines < 0 ? 64 : 65 // wheel up / down
        term.input(`\x1b[<${btn};1;1M`.repeat(Math.abs(lines)), false)
      } else {
        term.scrollLines(lines)
      }
    }
    e.preventDefault() // no page rubber-banding
  }
  const onEnd = () => {
    lastY = null
  }
  el.addEventListener('touchstart', onStart, { passive: true })
  el.addEventListener('touchmove', onMove, { passive: false })
  el.addEventListener('touchend', onEnd)
  el.addEventListener('touchcancel', onEnd)
  return () => {
    el.removeEventListener('touchstart', onStart)
    el.removeEventListener('touchmove', onMove)
    el.removeEventListener('touchend', onEnd)
    el.removeEventListener('touchcancel', onEnd)
  }
}
