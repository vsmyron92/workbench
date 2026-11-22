// The microphone of the new-session prompt: speak the task instead of typing it. The
// words go into the prompt at its caret; the session starts only on Start (or Ctrl+Enter),
// so a misheard word is fixed before an agent acts on it. See lib/dictation.ts.
//
// Contract module for the avatar feature: `useDictation`, `UseDictationOptions`, `DictateButton`, `Dictating` (its reply box dictates the same way).

import { useEffect, useMemo, useRef, useState, type RefObject } from 'react'
import { Mic } from 'lucide-react'
import { toast } from '@/shell/actions'
import { IconButton } from '@/ui'
import { speechSupport, startDictation, type Dictation, type DictationState, type Problem, type SpeechHost, type Support } from './lib/dictation'

export interface Dictating {
  support: Support
  state: DictationState
  toggle: () => void
  /** Stop at once, keeping the text: the user is editing the field or starting the session. */
  cancel: () => void
}

/** What a field other than the new-session prompt changes about its dictation. */
export interface UseDictationOptions {
  /** The language to listen for (a BCP 47 tag), taken when listening starts; without one, the browser's. */
  lang?: string
  /** Receives what went wrong in place of the toast, so the caller shows it where the user is looking. */
  onProblem?: (p: Problem) => void
}

/**
 * Dictation into a text area. `setText` receives the area's whole new text and where its
 * caret belongs; the caller applies both (the caret after the React update). Without
 * `opts` it listens in the browser's language and reports problems as toasts.
 */
export function useDictation(field: RefObject<HTMLTextAreaElement | null>, setText: (value: string, caret: number) => void, opts?: UseDictationOptions): Dictating {
  const support = useMemo(() => speechSupport(window as SpeechHost), [])
  const [state, setState] = useState<DictationState>('idle')
  const run = useRef<Dictation | null>(null)
  const latest = useRef(setText)
  const onProblem = useRef(opts?.onProblem)
  useEffect(() => {
    latest.current = setText
    onProblem.current = opts?.onProblem
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

  // A problem may arrive long after the tap: it goes to whoever asks for it now.
  const tell = (p: Problem) => {
    const told = onProblem.current
    if (told) told(p)
    else toast(p.level, p.message)
  }
  const toggle = () => {
    if (state !== 'idle') {
      run.current?.stop()
      return
    }
    if (!support.ok) {
      tell({ level: 'info', message: support.message })
      return
    }
    run.current = startDictation(
      support.ctor,
      {
        lang: opts?.lang || navigator.language || 'en-US',
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
        problem: tell,
      },
    )
  }
  return { support, state, toggle, cancel: () => run.current?.cancel() }
}

/**
 * The button; nothing when the browser cannot recognize speech at all. `label` names what
 * is dictated when it is not the new-session prompt; it should say where the audio goes.
 */
export function DictateButton({ d, label }: { d: Dictating; label?: string }) {
  if (!d.support.ok && d.support.reason === 'unsupported') return null
  const available = d.support.ok
  const busy = d.state !== 'idle'
  // The same label while it listens: `aria-pressed` says whether it is on. Where it cannot
  // work, the reason is the label whatever the caller calls it.
  const text = d.support.ok ? label || 'Speak the prompt (your browser’s speech recognition hears it, not Workbench)' : d.support.message
  return (
    <>
      {d.state === 'listening' && (
        <span className="wb-subtle wb-xs" role="status">
          Listening…
        </span>
      )}
      <IconButton
        icon={Mic}
        label={text}
        active={busy}
        aria-pressed={busy}
        // Not `disabled`: a tap on a phone, which has no hover, says why it cannot work.
        className={['wb-ag-mic', d.state === 'listening' && 'listening', !available && 'unavailable'].filter(Boolean).join(' ')}
        onClick={d.toggle}
      />
    </>
  )
}
