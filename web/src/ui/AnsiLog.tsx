// Read-only ANSI log viewer (CI job logs, run output, remote logs) on xterm.js.
// Pass the whole text; appends are detected (when `text` starts with the previous
// text only the tail is written), otherwise the view is reset.

import { useEffect, useRef } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { WebLinksAddon } from '@xterm/addon-web-links'
import { SearchAddon } from '@xterm/addon-search'
import '@xterm/xterm/css/xterm.css'
import { xtermTheme } from '@/theme/palette'
import { useUi } from '@/state/store'

export function AnsiLog({
  text,
  follow = true,
  onSearchReady,
}: {
  text: string
  /** Keep scrolled to the bottom as text grows. */
  follow?: boolean
  onSearchReady?: (search: SearchAddon) => void
}) {
  const host = useRef<HTMLDivElement>(null)
  const term = useRef<Terminal | null>(null)
  const written = useRef('')
  const fontSize = useUi((s) => s.prefs.terminalFontSize)
  const theme = useUi((s) => s.prefs.theme)

  useEffect(() => {
    const t = new Terminal({
      disableStdin: true,
      convertEol: true,
      scrollback: 100_000,
      fontFamily: getComputedStyle(document.documentElement).getPropertyValue('--font-mono'),
      fontSize,
      theme: xtermTheme(),
      cursorStyle: 'bar',
      cursorInactiveStyle: 'none',
    })
    const fit = new FitAddon()
    const search = new SearchAddon()
    t.loadAddon(fit)
    t.loadAddon(search)
    t.loadAddon(new WebLinksAddon((_e, uri) => window.open(uri, '_blank', 'noopener,noreferrer')))
    t.open(host.current!)
    term.current = t
    written.current = ''
    onSearchReady?.(search)
    const ro = new ResizeObserver(() => requestAnimationFrame(() => host.current?.offsetParent && fit.fit()))
    ro.observe(host.current!)
    return () => {
      ro.disconnect()
      t.dispose()
      term.current = null
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fontSize])

  useEffect(() => {
    const t = term.current
    if (!t) return
    if (text.startsWith(written.current)) {
      t.write(text.slice(written.current.length))
    } else {
      t.reset()
      t.write(text)
    }
    written.current = text
    if (follow) t.scrollToBottom()
  }, [text, follow, fontSize])

  // Re-theme in place (the store applies data-theme before React re-renders).
  useEffect(() => {
    if (term.current) term.current.options.theme = xtermTheme()
  }, [theme])

  return <div className="wb-log" ref={host} />
}

export default AnsiLog
