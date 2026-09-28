// The debug console: the program's output (DAP output events, by category), the
// debugger's messages, and a REPL that evaluates in the selected frame (gdb takes its
// own commands here: `info frame`, `print x`), with completions on Tab when the
// debugger offers them and a history on Up/Down.

import { useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from 'react'
import { ChevronRight, Eraser } from 'lucide-react'
import { ApiError } from '@/api/client'
import { IconButton, showMenuAt } from '@/ui'
import { toastError } from '@/shell/actions'
import { debugApi } from './api'
import { applyCompletion, consoleText, isLive } from './logic'
import { useDebug } from './store'
import type { DebugSession } from './types'

const history: string[] = []

export function ConsoleView({ s }: { s: DebugSession }) {
  const lines = useDebug((st) => st.output[s.id])
  const clearOutput = useDebug((st) => st.clearOutput)
  const cleared = useDebug((st) => st.cleared[s.id] ?? 0)
  const stack = useDebug((st) => st.stacks[s.id])
  const frameIndex = useDebug((st) => st.selection[s.id]?.frameIndex ?? 0)
  const blocks = useMemo(() => consoleText((lines ?? []).filter((l) => l.seq > cleared)), [lines, cleared])
  const outRef = useRef<HTMLDivElement>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const stick = useRef(true)
  const [text, setText] = useState('')
  const [hist, setHist] = useState(-1)
  const [busy, setBusy] = useState(false)

  useLayoutEffect(() => {
    const el = outRef.current
    if (el && stick.current) el.scrollTop = el.scrollHeight
  }, [blocks])
  useEffect(() => {
    stick.current = true
  }, [s.id])

  const frameId = stack && stack.epoch === s.stopEpoch ? stack.frames[frameIndex]?.id : undefined
  const canEval = isLive(s) && s.state !== 'starting'

  const submit = async () => {
    const expr = text.trim()
    if (!expr || busy) return
    if (history[history.length - 1] !== expr) history.push(expr)
    if (history.length > 200) history.shift()
    setHist(-1)
    setText('')
    setBusy(true)
    stick.current = true
    try {
      // The server echoes the command and the result into the console.
      await debugApi.evaluate(s.projectId, s.id, expr, 'repl', s.state === 'stopped' ? frameId : undefined)
    } catch (e) {
      // A failed evaluation is shown in the console by the server; anything else toasts.
      if (!(e instanceof ApiError && e.status === 422)) toastError(e)
    } finally {
      setBusy(false)
    }
  }

  const complete = async () => {
    const el = inputRef.current
    if (!el || !s.capabilities.supportsCompletionsRequest) return
    const caret = el.selectionStart ?? text.length
    try {
      const r = await debugApi.completions(s.projectId, s.id, text, caret + 1, frameId)
      const items = r.targets.slice(0, 60)
      const apply = (i: number) => {
        const next = applyCompletion(text, caret, items[i])
        setText(next.text)
        requestAnimationFrame(() => {
          el.focus()
          el.setSelectionRange(next.caret, next.caret)
        })
      }
      if (items.length === 1) apply(0)
      else if (items.length > 1) showMenuAt(el, items.map((t, i) => ({ label: t.label, run: () => apply(i) })))
    } catch (e) {
      toastError(e)
    }
  }

  const onKey = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') {
      e.preventDefault()
      void submit()
    } else if (e.key === 'Tab' && !e.shiftKey && text.trim()) {
      e.preventDefault()
      void complete()
    } else if (e.key === 'ArrowUp' && history.length) {
      e.preventDefault()
      const i = hist < 0 ? history.length - 1 : Math.max(0, hist - 1)
      setHist(i)
      setText(history[i])
    } else if (e.key === 'ArrowDown' && hist >= 0) {
      e.preventDefault()
      const i = hist + 1
      if (i >= history.length) {
        setHist(-1)
        setText('')
      } else {
        setHist(i)
        setText(history[i])
      }
    }
  }

  return (
    <div className="wb-dbg-console">
      <div
        ref={outRef}
        className="wb-dbg-console-out"
        onScroll={(e) => {
          const el = e.currentTarget
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24
        }}
        role="log"
        aria-label="Debug console"
      >
        {blocks.map((b) => (
          <span key={b.seq} className={`c-${b.category}`}>
            {b.text}
          </span>
        ))}
        {!blocks.length && <span className="c-workbench">No output yet.</span>}
      </div>
      <div className="wb-dbg-repl">
        <ChevronRight size={14} className="wb-muted" />
        <input
          ref={inputRef}
          className="wb-dbg-repl-input"
          value={text}
          disabled={!canEval}
          placeholder={canEval ? (s.adapter === 'gdb' ? 'gdb command or expression (Tab completes, ↑ history)' : 'Evaluate in the selected frame (↑ history)') : 'The session has ended'}
          spellCheck={false}
          onChange={(e) => {
            setText(e.target.value)
            setHist(-1)
          }}
          onKeyDown={onKey}
          aria-label="Debug console input"
        />
        <IconButton icon={Eraser} size="small" label="Clear console" onClick={() => clearOutput(s.id)} />
      </div>
    </div>
  )
}
