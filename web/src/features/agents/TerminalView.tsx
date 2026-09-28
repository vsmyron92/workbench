// The interactive terminal: xterm.js 6 attached to /api/terminals/{id}/ws.
//
// Lessons carried over from Mr. Mak (see docs/ARCHITECTURE.md history):
// * every (re)connection starts with a snapshot: reset(), resize to the server size,
//   write, then fit and report our size;
// * the resize is only sent while the view is visible (and again when it becomes
//   visible), so hidden tabs never fight over the PTY size;
// * when several views show one terminal (desktop + phone), the most recently active
//   one decides the size: the server announces it ({"t":"size"}), the other views render
//   at that size ("letterboxed") and take it back when the user clicks or types there;
// * programs may write the clipboard (OSC 52) only while this terminal has focus, and
//   can never read it;
// * fit padding lives on an outer wrapper — padding on the fitted element clips rows;
// * wheel: local scrollback on the normal buffer even when the program reports the
//   mouse, pass-through on the alternate buffer;
// * Ctrl+C copies when there is a selection, otherwise sends ^C; pastes go through the
//   DOM paste event (images are uploaded and their path inserted).

import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef, useState } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { SearchAddon } from '@xterm/addon-search'
import { Unicode11Addon } from '@xterm/addon-unicode11'
import { WebLinksAddon } from '@xterm/addon-web-links'
import { ClipboardAddon, type ClipboardSelectionType, type IClipboardProvider } from '@xterm/addon-clipboard'
import { WebglAddon } from '@xterm/addon-webgl'
import '@xterm/xterm/css/xterm.css'
import { ArrowDown, ArrowUp, Maximize2, Unplug, X } from 'lucide-react'
import { toast, toastError } from '@/shell/actions'
import { useUi } from '@/state/store'
import { cssVar, xtermTheme } from '@/theme/palette'
import { IconButton, Input } from '@/ui'
import { terminalsApi } from './api'
import { osc52WriteAllowed, validSize, wheelDecision, wheelLines } from './lib/protocol'
import { quotePath } from './lib/sessions'
import { TermSocket, type SocketStatus } from './lib/termSocket'
import { attachTouchScroll } from './lib/touchScroll'
import { markVisible } from './store'

export interface TerminalViewHandle {
  /** Raw input (keys, control sequences). */
  send(data: string): void
  /** Paste text the way the program expects (bracketed when enabled). */
  paste(text: string): void
  focus(): void
  openSearch(): void
  /** The user is acting on this view (e.g. the phone's keys bar): take the PTY size. */
  claim(): void
}

interface Props {
  terminalId: string
  /** On screen now. Size reports and focus only happen while visible. */
  visible: boolean
  /** Focus when the view becomes visible. */
  autoFocus?: boolean
  /** Phone layout: touch scrolling, smaller font. */
  mobile?: boolean
}

const MAX_ATTACH = 25 * 1024 * 1024
const PATH_MIME = 'application/x-workbench-path'

/** Whether the terminal host is laid out and large enough to fit. */
function measurable(host: HTMLElement | null, visible: boolean): boolean {
  return visible && !!host && !!host.offsetParent && host.clientWidth >= 20 && host.clientHeight >= 20
}

function openExternal(uri: string) {
  if (/^https?:\/\//i.test(uri)) window.open(uri, '_blank', 'noopener,noreferrer')
}

async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text)
  } catch {
    toast('warning', 'Copy needs a secure context (HTTPS or localhost)')
  }
}

/**
 * OSC 52 for programs in the terminal: a query always gets an empty clipboard (terminal
 * output can come from anywhere — ssh, logs, untrusted text — and must not read the
 * user's clipboard); a write is honoured only while this terminal has focus.
 */
function clipboardProvider(focused: () => boolean): IClipboardProvider {
  return {
    readText: () => '',
    writeText: async (selection: ClipboardSelectionType, text: string) => {
      if (!osc52WriteAllowed(selection, text, focused())) return
      try {
        await navigator.clipboard.writeText(text)
        toast('info', 'A program in the terminal copied text to the clipboard')
      } catch {
        /* no secure context or no permission */
      }
    },
  }
}

export const TerminalView = forwardRef<TerminalViewHandle, Props>(function TerminalView({ terminalId, visible, autoFocus, mobile }, ref) {
  const hostRef = useRef<HTMLDivElement>(null)
  const termRef = useRef<Terminal | null>(null)
  const fitRef = useRef<FitAddon | null>(null)
  const sockRef = useRef<TermSocket | null>(null)
  const searchRef = useRef<SearchAddon | null>(null)
  const visibleRef = useRef(visible)
  visibleRef.current = visible
  const fontSize = useUi((s) => s.prefs.terminalFontSize)
  const theme = useUi((s) => s.prefs.theme)
  const [status, setStatus] = useState<SocketStatus>('connecting')
  const [everOpen, setEverOpen] = useState(false)
  const [searchOpen, setSearchOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [dragOver, setDragOver] = useState(false)
  /** The PTY size as the server last announced it. */
  const serverSizeRef = useRef<{ cols: number; rows: number } | null>(null)
  /** Set while we resize xterm to the server's size: that is not our size to report. */
  const followingRef = useRef(false)
  /** Another view has the PTY size: we render at its size until the user acts here. */
  const [sizedElsewhere, setSizedElsewhere] = useState(false)
  const elsewhereRef = useRef(false)
  const markElsewhere = useCallback((v: boolean) => {
    elsewhereRef.current = v
    setSizedElsewhere(v)
  }, [])

  const fit = useCallback(() => {
    if (!measurable(hostRef.current, visibleRef.current)) return
    try {
      fitRef.current?.fit()
    } catch {
      /* not measurable yet */
    }
  }, [])

  /** Follow the layout, unless another view has the size (then keep rendering at it). */
  const fitUnlessElsewhere = useCallback(() => {
    if (!elsewhereRef.current) fit()
  }, [fit])

  /** The size this view fits, if it can be measured now. */
  const fittedSize = useCallback((): { cols: number; rows: number } | null => {
    if (!measurable(hostRef.current, visibleRef.current)) return null
    try {
      const d = fitRef.current?.proposeDimensions()
      return d && validSize(d.cols, d.rows) ? { cols: d.cols, rows: d.rows } : null
    } catch {
      return null
    }
  }, [])

  /** Fit this view and report its size (the server makes the PTY follow the latest report). */
  const report = useCallback(() => {
    const t = termRef.current
    if (!t || !visibleRef.current) return
    fit()
    markElsewhere(false)
    sockRef.current?.resize(t.cols, t.rows, true)
  }, [fit, markElsewhere])

  /** The user acts in this view: take the PTY size back if another view has it. */
  const claim = useCallback(() => {
    const t = termRef.current
    if (!t || !visibleRef.current) return
    const want = fittedSize()
    const server = serverSizeRef.current
    const settled = !!want && !!server && server.cols === want.cols && server.rows === want.rows && t.cols === want.cols && t.rows === want.rows
    if (!settled) report()
  }, [fittedSize, report])

  /** The server announced the PTY size: render at it (it may be another view's). */
  const followServer = useCallback(
    (cols: number, rows: number) => {
      serverSizeRef.current = { cols, rows }
      const t = termRef.current
      if (!t) return
      if (t.cols !== cols || t.rows !== rows) {
        followingRef.current = true
        try {
          t.resize(cols, rows)
        } finally {
          followingRef.current = false
        }
      }
      const want = fittedSize()
      markElsewhere(!!want && (want.cols !== cols || want.rows !== rows))
    },
    [fittedSize, markElsewhere],
  )

  const insertPaths = useCallback((paths: string[]) => {
    const t = termRef.current
    if (!t || !paths.length) return
    t.paste(paths.map(quotePath).join(' ') + ' ')
    t.focus()
  }, [])

  const uploadImages = useCallback(
    async (files: File[]) => {
      const paths: string[] = []
      for (const f of files.slice(0, 8)) {
        if (f.size > MAX_ATTACH) {
          toast('warning', `${f.name || 'Image'} is larger than 25 MB`)
          continue
        }
        try {
          paths.push((await terminalsApi.attach(terminalId, f)).path)
        } catch (e) {
          toastError(e, 'Could not attach the image')
        }
      }
      insertPaths(paths)
    },
    [terminalId, insertPaths],
  )

  useEffect(() => {
    const host = hostRef.current!
    const ui = useUi.getState().prefs
    const term = new Terminal({
      allowProposedApi: true,
      fontFamily: cssVar('--font-mono', 'monospace'),
      fontSize: mobile ? Math.min(ui.terminalFontSize, 12) : ui.terminalFontSize,
      lineHeight: 1.1,
      theme: xtermTheme(),
      scrollback: 10_000,
      cursorBlink: false,
      cursorInactiveStyle: 'outline',
      macOptionIsMeta: true,
      drawBoldTextInBrightColors: false,
      linkHandler: { activate: (_e, uri) => openExternal(uri), allowNonHttpProtocols: false },
    })
    const fitAddon = new FitAddon()
    const search = new SearchAddon()
    term.loadAddon(fitAddon)
    term.loadAddon(search)
    term.loadAddon(new Unicode11Addon())
    term.unicode.activeVersion = '11'
    term.loadAddon(new WebLinksAddon((_e, uri) => openExternal(uri)))
    term.loadAddon(new ClipboardAddon(undefined, clipboardProvider(() => document.hasFocus() && host.contains(document.activeElement))))
    term.open(host)
    // WebGL on desktop. Phones get the DOM renderer: browsers cap WebGL contexts and
    // high-DPR mobile GPUs mis-scale the glyph atlas.
    if (!mobile) {
      try {
        const gl = new WebglAddon()
        gl.onContextLoss(() => gl.dispose())
        term.loadAddon(gl)
      } catch {
        /* the DOM renderer is the fallback */
      }
    }
    termRef.current = term
    fitRef.current = fitAddon
    searchRef.current = search

    const sock = new TermSocket(terminalId, {
      onStatus: (s) => {
        setStatus(s)
        if (s === 'open') setEverOpen(true)
      },
      onAction: (a) => {
        if (a.kind === 'snapshot') {
          term.reset()
          if (a.cols && a.rows) followServer(a.cols, a.rows)
          // Every connection reports its size (the server forgets views that leave);
          // opening a view is using it, so it takes the size.
          term.write(a.data, report)
        } else if (a.kind === 'data') {
          term.write(a.data)
        } else if (a.kind === 'size') {
          followServer(a.cols, a.rows)
        } else if (a.kind === 'running') {
          // A new process started at the size the server chose; make sure it is ours.
          if (visibleRef.current) claim()
        }
      },
    })
    sockRef.current = sock
    const d1 = term.onData((d) => sock.send(d))
    const d2 = term.onBinary((d) => sock.send(Uint8Array.from(d, (c) => c.charCodeAt(0) & 0xff)))
    const d3 = term.onResize(({ cols, rows }) => {
      if (visibleRef.current && !followingRef.current) sock.resize(cols, rows)
    })
    // Clicking or focusing this view takes the PTY size back from another view.
    const onPointer = () => claim()
    host.addEventListener('pointerdown', onPointer, true)
    host.addEventListener('focusin', onPointer)

    term.attachCustomKeyEventHandler((e) => {
      if (e.type !== 'keydown') return true
      const ctrl = e.ctrlKey && !e.altKey && !e.metaKey
      const k = e.key.toLowerCase()
      if (ctrl && k === 'c' && (e.shiftKey || term.hasSelection())) {
        if (term.hasSelection()) {
          void copyText(term.getSelection())
          term.clearSelection()
        }
        e.preventDefault()
        return false
      }
      // Let the browser deliver a paste event (text and images are handled there).
      if (ctrl && k === 'v') return false
      if (ctrl && !e.shiftKey && k === 'f') {
        e.preventDefault()
        setSearchOpen(true)
        return false
      }
      return true
    })

    let wheelAcc = 0
    term.attachCustomWheelEventHandler((ev) => {
      const buf = term.buffer.active
      if (wheelDecision(buf.type, buf.baseY, term.modes.mouseTrackingMode) !== 'local') return true
      const lineHeight = (host.querySelector('.xterm-screen')?.clientHeight ?? 0) / term.rows || 16
      wheelAcc += wheelLines(ev.deltaY, ev.deltaMode, lineHeight, term.rows)
      const n = Math.trunc(wheelAcc)
      if (n) {
        wheelAcc -= n
        term.scrollLines(n)
      }
      ev.preventDefault()
      return false
    })

    const onPaste = (e: ClipboardEvent) => {
      const files = Array.from(e.clipboardData?.files ?? []).filter((f) => f.type.startsWith('image/'))
      if (!files.length) return // text: xterm pastes it (bracketed when enabled)
      e.preventDefault()
      e.stopPropagation()
      void uploadImages(files)
    }
    host.addEventListener('paste', onPaste, true)
    const detachTouch = mobile ? attachTouchScroll(term, host) : () => {}

    // While another view has the size, keep rendering at it (the user takes it back by
    // clicking or typing here); otherwise follow the layout.
    const ro = new ResizeObserver(() => requestAnimationFrame(fitUnlessElsewhere))
    ro.observe(host)
    const onWake = () => {
      if (document.visibilityState === 'visible') sock.kick()
    }
    document.addEventListener('visibilitychange', onWake)
    window.addEventListener('online', onWake)

    return () => {
      document.removeEventListener('visibilitychange', onWake)
      window.removeEventListener('online', onWake)
      host.removeEventListener('pointerdown', onPointer, true)
      host.removeEventListener('focusin', onPointer)
      host.removeEventListener('paste', onPaste, true)
      detachTouch()
      ro.disconnect()
      d1.dispose()
      d2.dispose()
      d3.dispose()
      sock.dispose()
      term.dispose()
      termRef.current = null
      sockRef.current = null
    }
    // A view is bound to one terminal for its lifetime.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [terminalId])

  // Becoming visible: fit, report the size, optionally focus.
  useEffect(() => {
    if (visible) {
      markVisible(terminalId, true)
      const raf = requestAnimationFrame(() => {
        report()
        if (autoFocus) termRef.current?.focus()
      })
      return () => {
        cancelAnimationFrame(raf)
        markVisible(terminalId, false)
      }
    }
    return undefined
  }, [visible, autoFocus, terminalId, report])

  useEffect(() => {
    const t = termRef.current
    if (!t) return
    t.options.fontSize = mobile ? Math.min(fontSize, 12) : fontSize
    t.options.theme = xtermTheme()
    requestAnimationFrame(fitUnlessElsewhere)
  }, [fontSize, theme, mobile, fitUnlessElsewhere])

  useImperativeHandle(
    ref,
    () => ({
      send: (data) => sockRef.current?.send(data),
      paste: (text) => termRef.current?.paste(text),
      focus: () => termRef.current?.focus(),
      openSearch: () => setSearchOpen(true),
      claim,
    }),
    [claim],
  )

  const find = (dir: 'next' | 'prev') => {
    const s = searchRef.current
    if (!s || !query) return
    if (dir === 'next') s.findNext(query, { caseSensitive: false })
    else s.findPrevious(query, { caseSensitive: false })
  }

  const closeSearch = () => {
    setSearchOpen(false)
    searchRef.current?.clearDecorations()
    termRef.current?.clearSelection()
    termRef.current?.focus()
  }

  return (
    <div
      className={['wb-ag-term', dragOver && 'drag-over'].filter(Boolean).join(' ')}
      onDragOver={(e) => {
        const types = e.dataTransfer.types
        if (types.includes(PATH_MIME) || types.includes('Files')) {
          e.preventDefault()
          e.dataTransfer.dropEffect = 'copy'
          setDragOver(true)
        }
      }}
      onDragLeave={() => setDragOver(false)}
      onDrop={(e) => {
        setDragOver(false)
        const p = e.dataTransfer.getData(PATH_MIME)
        if (p) {
          e.preventDefault()
          insertPaths(p.split('\n').map((x) => x.trim()).filter(Boolean))
          return
        }
        const files = Array.from(e.dataTransfer.files)
        if (!files.length) return
        e.preventDefault()
        const images = files.filter((f) => f.type.startsWith('image/'))
        if (images.length) void uploadImages(images)
        if (images.length < files.length) {
          toast('info', 'Only images can be dropped from your computer', {
            detail: 'Drag files from the Files tool window to insert their paths.',
          })
        }
      }}
    >
      {/* Padding lives here, not on the fitted element (it would clip the last rows). */}
      <div className="wb-ag-term-host" ref={hostRef} />
      {searchOpen && (
        <div className="wb-ag-search" onKeyDown={(e) => e.key === 'Escape' && closeSearch()}>
          <Input
            small
            autoFocus
            placeholder="Find"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') find(e.shiftKey ? 'prev' : 'next')
            }}
          />
          <IconButton icon={ArrowUp} size="small" label="Previous match (Shift+Enter)" onClick={() => find('prev')} />
          <IconButton icon={ArrowDown} size="small" label="Next match (Enter)" onClick={() => find('next')} />
          <IconButton icon={X} size="small" label="Close (Esc)" onClick={closeSearch} />
        </div>
      )}
      {status !== 'open' && everOpen ? (
        <div className="wb-ag-reconnecting">
          <Unplug size={12} /> Reconnecting…
        </div>
      ) : (
        sizedElsewhere && (
          <button className="wb-ag-sized" title="Another window or device is using this terminal at its own size" onClick={() => claim()}>
            <Maximize2 size={12} /> Sized for another screen · Fit here
          </button>
        )
      )}
    </div>
  )
})
