// The microphone of the new-session prompt: speak the task instead of typing it. The
// words go into the prompt at its caret; the session starts only on Start (or Ctrl+Enter),
// so a misheard word is fixed before an agent acts on it. See lib/dictation.ts.

import { useEffect, useMemo, useRef, useState, type RefObject } from 'react'
import { Mic } from 'lucide-react'
import { toast } from '@/shell/actions'
import { IconButton } from '@/ui'
import { speechSupport, startDictation, type Dictation, type DictationState, type SpeechHost, type Support } from './lib/dictation'

export interface Dictating {
  support: Support
  state: DictationState
  toggle: () => void
  /** Stop at once, keeping the text: the user is editing the field or starting the session. */
  cancel: () => void
}

/**
 * Dictation into a text area. `setText` receives the area's whole new text and where its
 * caret belongs; the caller applies both (the caret after the React update).
 */
export function useDictation(field: RefObject<HTMLTextAreaElement | null>, setText: (value: string, caret: number) => void): Dictating {
  const support = useMemo(() => speechSupport(window as SpeechHost), [])
  const [state, setState] = useState<DictationState>('idle')
  const run = useRef<Dictation | null>(null)
  const latest = useRef(setText)
  useEffect(() => {
    latest.current = setText
  })
  useEffect(() => () => run.current?.cancel(), [])
  // A microphone must not stay open behind a tab or window the user has left.
  const listening = state !== 'idle'
  useEffect(() => {
    if (!listening) return
    const leave = () => {
      if (document.visibilityState === 'hidden') run.current?.stop()
    }
    document.addEventListener('visibilitychange', leave)
    return () => document.removeEventListener('visibilitychange', leave)
  }, [listening])

  const toggle = () => {
    if (state !== 'idle') {
      run.current?.stop()
      return
    }
    if (!support.ok) {
      toast('info', support.message)
      return
    }
    run.current = startDictation(
      support.ctor,
      {
        lang: navigator.language || 'en-US',
        // Touch screens: one utterance per tap (Android's recognizer repeats words otherwise).
        continuous: !window.matchMedia?.('(pointer: coarse)').matches,
      },
      {
        field: () => {
          const el = field.current
          return el ? { value: el.value, start: el.selectionStart, end: el.selectionEnd } : { value: '', start: 0, end: 0 }
        },
        text: (value, caret) => latest.current(value, caret),
        state: setState,
        problem: (p) => toast(p.level, p.message),
      },
    )
  }
  return { support, state, toggle, cancel: () => run.current?.cancel() }
}

/** The button; nothing when the browser cannot recognize speech at all. */
export function DictateButton({ d }: { d: Dictating }) {
  if (!d.support.ok && d.support.reason === 'unsupported') return null
  const available = d.support.ok
  const busy = d.state !== 'idle'
  // The same label while it listens: `aria-pressed` says whether it is on.
  const label = d.support.ok ? 'Speak the prompt (your browser’s speech recognition hears it, not Workbench)' : d.support.message
  return (
    <>
      {d.state === 'listening' && (
        <span className="wb-subtle wb-xs" role="status">
          Listening…
        </span>
      )}
      <IconButton
        icon={Mic}
        label={label}
        active={busy}
        aria-pressed={busy}
        // Not `disabled`: a tap on a phone, which has no hover, says why it cannot work.
        className={['wb-ag-mic', d.state === 'listening' && 'listening', !available && 'unavailable'].filter(Boolean).join(' ')}
        onClick={d.toggle}
      />
    </>
  )
}
