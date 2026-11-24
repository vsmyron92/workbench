// The bubble: what an avatar says and asks, and where the user answers.
//
// It is a Workbench component around agent-written text (rule 11). The session's identity
// comes first and is the app's (provider icon, session title, project); the agent's own
// name follows in quotation marks. Captions, names, choices and replies are text nodes:
// never Markdown, never HTML. The choices have a style of their own, and over them the app
// says in its own words that an answer goes to the session as a message, so a question of the
// avatar is never mistaken for a permission request; the bubble never shows one: for a
// session that needs a decision it says so and offers to open the session.
//
// A pure view: everything it shows comes in as props, everything the user does goes out
// as callbacks. It keeps no state, fetches nothing, and sends nothing by itself: a typed
// or dictated reply is text until Enter or a click on Send.

import { useLayoutEffect, useRef, type ReactNode, type Ref } from 'react'
import { Asterisk, CircleStop, CodeXml, Moon, SendHorizontal, Sparkles, SquareTerminal, TriangleAlert, Volume2, Wrench, X, type LucideIcon } from 'lucide-react'
import type { AgentProvider } from '@/api/types'
import { DictateButton, type Dictating } from '@/features/agents/Dictate'
import { Button, IconButton, Spinner, TextArea } from '@/ui'
import type { Ask, Line } from './lib/types'
import './avatar.css'

// ---------------------------------------------------------------- wording (10.8)

export const WORDS = {
  hear: 'Hear it',
  captionsOnly: 'Captions only',
  allowOnline: 'Allow online voices',
  /** Rule 7: what allowing online voices means. */
  online: 'What an avatar says is sent to the browser maker’s speech service to be spoken.',
  noVoices: 'This browser has no voices: avatars show captions.',
  stop: 'Stop',
  open: 'Open',
  close: 'Close',
  send: 'Send (Enter)',
  dismiss: 'Dismiss',
  answers: 'Answers',
  you: 'You',
  blocked: 'Waiting for your decision in the session',
  dialog: 'Answer the dialog in the terminal first',
  starting: 'The session is starting',
  sent: 'Sent.',
  sentLater: 'Sent. It reads this at its next step.',
  changed: 'The question changed.',
  /** The microphone of the reply box: it says where the audio goes. */
  dictate: 'Speak your reply (your browser’s speech recognition hears it, not Workbench)',
} as const

/** The longest name the app's own phrases carry whole: an agent's name fits, a long session title is cut. */
const MAX_CALLED_CHARS = 32
/** As much of the session's title as goes in front of the agent's name in them. */
const MAX_CALLED_TITLE_CHARS = 28

const cutTo = (text: string, max: number): string => {
  const chars = Array.from(text)
  return chars.length > max ? `${chars.slice(0, max - 1).join('').trimEnd()}…` : chars.join('')
}

/**
 * What the avatar is called in the app's phrases (menus, dialogs, the palette, the figure's
 * controls and its face): as the bubble's header names it, the session's title, then the
 * agent's own name in quotation marks, so that a name never reads as the app's own words.
 * The session's title alone while the agent has no name. Both cut to a few words.
 */
export function calledAs(name: string, title: string): string {
  if (!name) return cutTo(title, MAX_CALLED_CHARS)
  const session = cutTo(title, MAX_CALLED_TITLE_CHARS)
  const quoted = `“${cutTo(name, MAX_CALLED_CHARS)}”`
  return session ? `${session} ${quoted}` : quoted
}

/**
 * What the bubble calls its avatar in its own phrases, under the header that names the
 * session: the agent's own name in quotation marks, else the session's title, cut.
 */
export function calledHere(name: string, title: string): string {
  return name ? `“${cutTo(name, MAX_CALLED_CHARS)}”` : cutTo(title, MAX_CALLED_CHARS)
}

export const clickToHear = (called: string): string => `Click to hear ${called}`
/** Workbench's own line over a question's choices and its reply box: what an answer is and where it goes. */
export const answerGoesTo = (title: string): string => `Your answer goes to ${title.trim() || 'the session'} as a message.`
/** The reply box's placeholder. A name that was cut already ends in the ellipsis this phrase ends in. */
export const replyTo = (called: string): string => `Reply to ${called.replace(/…$/, '')}…`

/** The server takes a reply of at most this many characters (`MAX_REPLY_CHARS`). */
export const MAX_REPLY_CHARS = 4000
const MAX_CHOICES = 4
const LANG = /^[A-Za-z]{2,3}(?:-[A-Za-z0-9]{2,8}){0,2}$/

// ---------------------------------------------------------------- small decisions

/**
 * Whether a key press in the reply box sends the reply: Enter, but not with Shift (that
 * breaks the line), and never while an input method is composing (Enter then accepts the
 * composition; `keyCode` 229 is how older browsers say the same). An Enter held down sends
 * once: its repeats would send a refused reply again many times a second.
 */
export function sendsOn(e: { key: string; shiftKey: boolean; isComposing?: boolean; keyCode?: number; repeat?: boolean }): boolean {
  return e.key === 'Enter' && !e.shiftKey && e.isComposing !== true && e.keyCode !== 229 && e.repeat !== true
}

/** Whether a press landed in a text field: everything else keeps focus where it was. */
export function isTextField(target: unknown): boolean {
  const tag = (target as { tagName?: unknown } | null)?.tagName
  return tag === 'TEXTAREA' || tag === 'INPUT'
}

/**
 * Whether a key press is an Enter held down on a control of the bubble that is not a text
 * field: its default is cancelled, so a focused Send or choice clicks once, not on every
 * repeat (a refused reply would be posted again many times a second, and the Enter that
 * only takes note of a changed question would be followed by one that sends). A held
 * Space clicks once anyway, when it is let go; the reply box has its own rule (`sendsOn`).
 */
export function repeatsActivation(e: { key: string; repeat?: boolean; target: unknown }): boolean {
  return e.key === 'Enter' && e.repeat === true && !isTextField(e.target)
}

function cancelRepeats(e: { key: string; repeat: boolean; target: unknown; preventDefault(): void }) {
  if (repeatsActivation(e)) e.preventDefault()
}

function keepFocusOutsideFields(e: { target: unknown; preventDefault(): void }) {
  if (!isTextField(e.target)) e.preventDefault()
}

/** The icons of the agent CLIs, as the agents views show them. A literal table: the kind only picks from it. */
const PROVIDER_ICONS: ReadonlyMap<string, { icon: LucideIcon; tint: string }> = new Map([
  ['claude', { icon: Asterisk, tint: 'claude' }],
  ['codex', { icon: CodeXml, tint: 'codex' }],
  ['kimi', { icon: Moon, tint: 'kimi' }],
  ['gemini', { icon: Sparkles, tint: 'gemini' }],
  ['aider', { icon: Wrench, tint: 'aider' }],
  ['custom', { icon: SquareTerminal, tint: 'custom' }],
])

function ProviderMark({ kind }: { kind: AgentProvider }) {
  const known = PROVIDER_ICONS.get(kind) ?? { icon: SquareTerminal, tint: 'custom' }
  const Icon = known.icon
  return <Icon size={13} className={`wb-av-picon ${known.tint}`} aria-hidden />
}

// ---------------------------------------------------------------- props

/** The line on screen. With `chunks`, the one at `index` is being said (or shown) now. */
export interface BubbleCaption {
  /** The line's id, so the transcript can mark it. */
  lineId?: string
  text: string
  /** The line as the speaker cut it (`SpeakerEvents.start`). */
  chunks?: readonly string[]
  /** The chunk being presented (`SpeakerEvents.chunk`). */
  index?: number
  /** The line's language (a BCP 47 tag). */
  lang?: string
}

export type BubbleNote =
  /** Delivered while the session was idle. */
  | { kind: 'sent' }
  /** Delivered while the session was working. */
  | { kind: 'sentLater' }
  /** The question changed under a draft: sending needs another Enter. */
  | { kind: 'changed' }
  /** What the server answered (a 409), or what went wrong with the microphone. */
  | { kind: 'error'; text: string }
  | { kind: 'info'; text: string }

export interface BubbleReply {
  draft: string
  /** The reply is on its way: the server waits for a pause in the session's typing, which takes some seconds. */
  sending?: boolean
  fieldRef?: Ref<HTMLTextAreaElement>
  /** The dictation of this field (`useDictation` in the container); null or absent: no microphone. */
  dictating?: Dictating | null
}

export interface BubbleProps {
  // Who speaks.
  provider: AgentProvider
  /** The session's title. */
  title: string
  /** The session's project, only when it is not the current one. */
  project?: string | null
  /** The agent's own name, or "". */
  name: string

  /** The session needs a permission or an answer: one line and Open, nothing of the agent's. */
  blocked?: boolean

  /** The line on screen: the whole content of a bubble that opened by itself. */
  caption?: BubbleCaption | null
  /** This tab says the caption aloud: Stop is shown, and a screen reader does not announce it a second time. */
  aloud?: boolean
  /** The last lines, oldest first: the content of a bubble the user opened. `caption`, if its line is among them, marks it. */
  log?: readonly Line[] | null
  /** The id of the first of these lines the user has not seen: the transcript starts there. */
  unseenFrom?: string | null

  /** The open question: its choices are shown. */
  ask?: Ask | null
  /** Why the choices and the reply box cannot be used now. */
  disabled?: 'dialog' | 'starting' | null
  /** The reply box. null or absent: none. */
  reply?: BubbleReply | null
  note?: BubbleNote | null

  /**
   * The first voice choice of this browser. `offer`: Hear it and Captions only. `online`:
   * the browser has only online voices, so the sentence about them with Allow online
   * voices. `none`: it has no voices.
   */
  voiceChoice?: 'offer' | 'online' | 'none' | null
  /** Voice is on, but the browser wants a click first: "Click to hear <name>". */
  locked?: boolean

  /** Which side the figure is on. */
  tail?: 'left' | 'right' | null
  /** The compact form: a panel under the top bar. */
  panel?: boolean
  /** It opened or changed by itself a moment ago: clicks go through it. */
  clickThrough?: boolean
  /**
   * It opened or changed by itself under a pointer that rests on it: it takes the pointer,
   * but its controls do not act on it yet and look inactive. Keys still work.
   */
  holding?: boolean
  /** The stage, inside the panel. */
  figure?: ReactNode
  /** The bubble's own box, for the overlay to measure (where it is, whether the pointer is inside). */
  rootRef?: Ref<HTMLDivElement>

  /** A choice was clicked: its index, from 0. */
  onChoice?: (index: number) => void
  onDraft?: (text: string) => void
  /** Enter in the reply box, or a click on Send. */
  onSend?: () => void
  onDismiss?: () => void
  onStop?: () => void
  onHear?: () => void
  onCaptionsOnly?: () => void
  onAllowOnline?: () => void
  onUnlock?: () => void
  /** Open the session (the blocked form). */
  onOpen?: () => void
  onClose?: () => void
}

// ---------------------------------------------------------------- pieces

const langOf = (lang: unknown): string | undefined => (typeof lang === 'string' && LANG.test(lang) ? lang : undefined)

const chunksOf = (caption: BubbleCaption): string[] => (Array.isArray(caption.chunks) ? caption.chunks.filter((c): c is string => typeof c === 'string') : [])
const chunkIndex = (caption: BubbleCaption): number => (typeof caption.index === 'number' && Number.isInteger(caption.index) ? caption.index : -1)
/** The caption's line was presented to its end (its index is past the last chunk). */
const presentedToEnd = (caption: BubbleCaption): boolean => {
  const chunks = chunksOf(caption)
  return chunks.length > 0 && chunkIndex(caption) >= chunks.length
}

/**
 * A line's text, with the chunk that is being said set off from what was said and what is
 * to come. A line that was presented to its end keeps its parts, plain: the text a screen
 * reader was given stays where it was, so it is not announced again.
 */
function Spoken({ caption, nowRef }: { caption: BubbleCaption; nowRef?: Ref<HTMLSpanElement> }) {
  const chunks = chunksOf(caption)
  const index = chunkIndex(caption)
  if (chunks.length === 0 || index < 0) return <>{caption.text}</>
  const done = index >= chunks.length
  return (
    <>
      {chunks.map((chunk, i) => (
        <span key={i} ref={i === index ? nowRef : undefined} className={done ? undefined : i < index ? 'wb-av-said' : i === index ? 'wb-av-now' : 'wb-av-next'}>
          {i > 0 ? ` ${chunk}` : chunk}
        </span>
      ))}
    </>
  )
}

/** Keeps `target` in view inside `box` (the chunk being said, the first unseen line). */
function reveal(box: HTMLElement | null, target: HTMLElement | null, toEnd: boolean) {
  if (!box) return
  if (!target) {
    if (toEnd) box.scrollTop = box.scrollHeight
    return
  }
  const top = target.offsetTop
  if (top < box.scrollTop || top + target.offsetHeight > box.scrollTop + box.clientHeight) box.scrollTop = top
}

function Caption({ caption, aloud }: { caption: BubbleCaption; aloud: boolean }) {
  const box = useRef<HTMLDivElement>(null)
  const now = useRef<HTMLSpanElement>(null)
  // A long line is four lines high: what is being said stays in view.
  useLayoutEffect(() => reveal(box.current, now.current, false), [caption.text, caption.index])
  return (
    <div ref={box} className="wb-av-caption" aria-live={aloud ? 'off' : 'polite'} lang={langOf(caption.lang)} dir="auto">
      <Spoken caption={caption} nowRef={now} />
    </div>
  )
}

function Log({ lines, unseenFrom, caption, aloud }: { lines: readonly Line[]; unseenFrom: string | null; caption: BubbleCaption | null; aloud: boolean }) {
  const box = useRef<HTMLDivElement>(null)
  const first = useRef<HTMLDivElement>(null)
  const last = lines.length > 0 ? lines[lines.length - 1].id : ''
  // It opens at the first line the user has not seen, else at the newest; a new line scrolls into view.
  useLayoutEffect(() => reveal(box.current, first.current, true), [unseenFrom, last])
  let unseen = false
  return (
    // A log is announced as it grows; not while this tab says its line aloud.
    <div ref={box} className="wb-av-log" role="log" aria-live={aloud ? 'off' : 'polite'}>
      {lines.map((line) => {
        const starts = unseenFrom !== null && line.id === unseenFrom
        if (starts) unseen = true
        const mine = line.from === 'user'
        const spoken = !mine && caption !== null && caption.lineId === line.id
        const now = spoken && !presentedToEnd(caption)
        return (
          <div key={line.id} ref={starts ? first : undefined} className={['wb-av-line', mine && 'user', unseen && !mine && 'unseen', now && 'now'].filter(Boolean).join(' ')}>
            {mine && <span className="wb-av-who">{WORDS.you}</span>}
            <span className="wb-av-line-text" lang={mine ? undefined : langOf(line.lang)} dir="auto">
              {spoken && caption ? <Spoken caption={caption} /> : line.text}
            </span>
          </div>
        )
      })}
    </div>
  )
}

function NoteLine({ note }: { note: BubbleNote }) {
  switch (note.kind) {
    case 'sent':
      return (
        <div className="wb-av-notice" role="status">
          {WORDS.sent}
        </div>
      )
    case 'sentLater':
      return (
        <div className="wb-av-notice" role="status">
          {WORDS.sentLater}
        </div>
      )
    case 'changed':
      return (
        <div className="wb-av-notice changed" role="status">
          {WORDS.changed}
        </div>
      )
    case 'error':
      return (
        <div className="wb-av-notice error" role="alert">
          <TriangleAlert size={13} aria-hidden />
          <span>{note.text}</span>
        </div>
      )
    default:
      return (
        <div className="wb-av-notice" role="status">
          {note.text}
        </div>
      )
  }
}

// ---------------------------------------------------------------- the bubble

export function Bubble(props: BubbleProps) {
  const { provider, title, project, name, blocked, caption, aloud, log, unseenFrom, ask, disabled, reply, note, voiceChoice, locked, tail, panel, clickThrough, holding, figure } = props
  const called = calledHere(name, title)
  const reason = disabled === 'dialog' ? WORDS.dialog : disabled === 'starting' ? WORDS.starting : null
  const off = reason !== null
  const sending = reply?.sending === true
  const canSend = !!reply && !off && !sending && reply.draft.trim() !== ''
  const choices = ask && Array.isArray(ask.choices) ? ask.choices.filter((c): c is string => typeof c === 'string').slice(0, MAX_CHOICES) : []
  const lines = Array.isArray(log) ? log : null
  // In the transcript the question is the line that says it; a bubble without that line shows it above the choices.
  const questionShown = !!ask && ((!lines && caption?.lineId === ask.lineId) || (!!lines && lines.some((l) => l.id === ask.lineId)))
  const classes = ['wb-av-bubble', panel && 'panel', tail === 'left' && 'tail-left', tail === 'right' && 'tail-right', clickThrough ? 'through' : holding && 'hold', blocked && 'blocked'].filter(Boolean).join(' ')

  return (
    <div ref={props.rootRef} className={classes} role="group" aria-label={`Avatar of ${title}`} onPointerDown={keepFocusOutsideFields} onMouseDown={keepFocusOutsideFields} onKeyDown={cancelRepeats}>
      <div className="wb-av-head">
        <ProviderMark kind={provider} />
        <span className="wb-av-title">{title}</span>
        {project ? <span className="wb-av-project">{project}</span> : null}
        {name ? (
          <span className="wb-av-name" dir="auto">
            {`“${name}”`}
          </span>
        ) : null}
        <span className="wb-av-head-space" />
        {aloud && !blocked && props.onStop ? (
          // The icon of the menu's "Stop speaking": a bare square reads as an empty check box.
          <Button size="small" icon={CircleStop} className="wb-av-stop" onClick={props.onStop}>
            {WORDS.stop}
          </Button>
        ) : null}
        {props.onClose ? <IconButton icon={X} size="small" label={WORDS.close} onClick={props.onClose} /> : null}
      </div>

      {blocked ? (
        <div className="wb-av-blocked">
          <span>{WORDS.blocked}</span>
          <Button size="small" onClick={props.onOpen}>
            {WORDS.open}
          </Button>
        </div>
      ) : (
        <div className="wb-av-main">
          {figure ? <div className="wb-av-bubble-figure">{figure}</div> : null}
          <div className="wb-av-body">
            {lines ? <Log lines={lines} unseenFrom={unseenFrom ?? null} caption={caption ?? null} aloud={aloud === true} /> : caption ? <Caption caption={caption} aloud={aloud === true} /> : null}

            {ask ? (
              <div className="wb-av-ask">
                {!questionShown && (
                  <div className="wb-av-question" dir="auto">
                    {ask.text}
                  </div>
                )}
                {/* The app's own words between the session's question and its choices: what an answer is, and where it goes. */}
                <div className="wb-av-where">{answerGoesTo(title)}</div>
                {choices.length > 0 && (
                  <div className="wb-av-choices" role="group" aria-label={WORDS.answers}>
                    {choices.map((choice, i) => (
                      <Button key={`${ask.id}:${i}`} className="wb-av-choice" disabled={off || sending} onClick={() => props.onChoice?.(i)}>
                        <span dir="auto">{choice}</span>
                      </Button>
                    ))}
                  </div>
                )}
                {props.onDismiss ? (
                  <Button size="small" variant="ghost" className="wb-av-dismiss" disabled={sending} onClick={props.onDismiss}>
                    {WORDS.dismiss}
                  </Button>
                ) : null}
              </div>
            ) : null}

            {reason !== null && (ask || reply) ? (
              <div className="wb-av-reason" role="status">
                {reason}
              </div>
            ) : null}

            {reply ? (
              <div className="wb-av-reply">
                <TextArea
                  ref={reply.fieldRef}
                  className="wb-av-reply-text"
                  rows={1}
                  value={reply.draft}
                  placeholder={replyTo(called)}
                  aria-label={replyTo(called)}
                  maxLength={MAX_REPLY_CHARS}
                  disabled={off}
                  readOnly={sending}
                  onChange={(e) => props.onDraft?.(e.target.value)}
                  onKeyDown={(e) => {
                    const enter = { key: e.key, shiftKey: e.shiftKey, isComposing: e.nativeEvent.isComposing, keyCode: e.keyCode }
                    if (!sendsOn(enter)) return
                    // Enter never breaks the line here, also when there is nothing to send yet, or when it is held down.
                    e.preventDefault()
                    if (canSend && sendsOn({ ...enter, repeat: e.repeat })) props.onSend?.()
                  }}
                />
                {reply.dictating && !off && !sending ? <DictateButton d={reply.dictating} label={WORDS.dictate} /> : null}
                {sending ? <Spinner /> : <IconButton icon={SendHorizontal} label={WORDS.send} disabled={!canSend} onClick={props.onSend} />}
              </div>
            ) : null}

            {note ? <NoteLine note={note} /> : null}

            {voiceChoice === 'offer' ? (
              <div className="wb-av-voice">
                <Button size="small" icon={Volume2} onClick={props.onHear}>
                  {WORDS.hear}
                </Button>
                <Button size="small" variant="ghost" onClick={props.onCaptionsOnly}>
                  {WORDS.captionsOnly}
                </Button>
              </div>
            ) : voiceChoice === 'online' ? (
              <div className="wb-av-voice online">
                <p>{WORDS.online}</p>
                <Button size="small" icon={Volume2} onClick={props.onAllowOnline}>
                  {WORDS.allowOnline}
                </Button>
                <Button size="small" variant="ghost" onClick={props.onCaptionsOnly}>
                  {WORDS.captionsOnly}
                </Button>
              </div>
            ) : voiceChoice === 'none' ? (
              <div className="wb-av-voice none" role="status">
                <p>{WORDS.noVoices}</p>
              </div>
            ) : locked ? (
              <div className="wb-av-voice">
                <Button size="small" icon={Volume2} onClick={props.onUnlock}>
                  {clickToHear(called)}
                </Button>
              </div>
            ) : null}
          </div>
        </div>
      )}
    </div>
  )
}
