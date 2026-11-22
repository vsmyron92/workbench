// The overlay: the avatars on screen. A lazy chunk the provider mounts when there is an
// avatar to show; it owns the window's one engine, speaker and arbiter.
//
// It decides nothing by itself that a pure module can decide: who is on stage comes from
// the provider (lib/stage.ts), who is the active one and when its bubble opens and closes
// from `settle` (lib/bubble.ts), where things sit from lib/layout.ts, what a reply answers
// from the draft rules of lib/bubble.ts. Here those are connected to React, the stores,
// the speaker's events and the page.
//
// Two layouts. Wide (desktop shell, workspace window open): the figures walk about the
// workspace window by themselves (lib/roam.ts), each of them draggable, and one that is
// wanted stands still with its bubble beside it; with walking switched off or reduced
// motion they stand in a column that is dragged as a whole. Compact (workspace window
// collapsed, or the phone shell): the avatar in front is a face in the top bar, and the
// bubble a panel under it.
//
// What a session wrote is only ever shown through `Figure` and `Bubble`: nothing here
// puts data into a class, a style or an attribute name.

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type FocusEvent as ReactFocusEvent, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent, type ReactNode, type RefObject } from 'react'
import { createPortal } from 'react-dom'
import { MoreHorizontal, Reply, SquareArrowOutUpRight } from 'lucide-react'
import type { AgentProvider } from '@/api/types'
import { useEvent } from '@/api/events'
import { useProjects, useTerminals } from '@/api/queries'
import { useDictation } from '@/features/agents/Dictate'
import { useAgentsUi } from '@/features/agents/store'
import { confirmDialog, openPanel, openSettings, toast } from '@/shell/actions'
import { useIsMobile } from '@/shell/windowFit'
import { useUi } from '@/state/store'
import { IconButton, showMenuAt } from '@/ui'
import { AVATAR_EVENTS, avatarApi, useAvatars, type AvatarLineChanged } from './api'
import { Bubble, WORDS, calledAs, type BubbleCaption, type BubbleNote } from './Bubble'
import { Face, MoreFaces } from './Face'
import { FACE_PX, FIGURE_PX, FIGURE_PX_PHONE, FIGURE_RING_PX, FigureButton, drawnLook, keepFocus, ringFor, stateWords, type Ring } from './Figure'
import { lineKey, type ArbiterState } from './lib/arbiter'
import { EMPTY, HOVER_DWELL_MS, NO_DRAFT, armGuard, askChanged, askedToHear, clicksThrough, closeByUser, edited, guardStep, guardWake, holding, noteEdited, noteStep, noteWake, noted, openByUser, planChoice, planSend, questionChanged, replyNoted, replyShown, sentNote, settle, voiceForm, whyDisabled, type Draft, type Facts, type Guard, type LastLine, type Noted, type ReplyBody, type Scene } from './lib/bubble'
import type { FigureHandle } from './lib/engine'
import { EDGE_PX, SIDE_EDGE_PX, bubbleShift, corner, defaultAnchor, dragStarts, dragged, layoutMode, panelPlace, panelTop, phoneFace, place, smallFigures, storedAnchor, toStored, type Anchor, type Corner, type Placement } from './lib/layout'
import { attend, claimOf, feetBounds, gazeToward, newWorld, placePost, postFor, samplePoints, step, type Box, type Cue, type Obstacle, type Point, type Post, type Walker, type World } from './lib/roam'
import type { SpeakPrefs } from './lib/speech'
import { DESIGNING_MS, figures, front, isDesigning, mostUrgent, presented, seenAfter, transcript, unseen, type Candidate, type Staged } from './lib/stage'
import { avatarState, type Ask, type AvatarInfo, type AvatarState, type Line } from './lib/types'
import { inside, useAttention, useGeometry, useMedia, usePointer, useSize, type PointerAt } from './measure'
import { avatarMenu } from './menu'
import { createRig, type Rig } from './rig'
import { hadGesture, onAct, onGesture, setOverlayActions, useAvatarRuntime } from './runtime'
import { useAvatarPrefs } from './store'
import { sessionViews, type SessionView } from './view'
import { useBrowserVoices, useVoicesListed, voicesFor } from './voices'

// ---------------------------------------------------------------- small things

const clock = (): number => performance.now()
/** What the artwork's ids are printed with: one prefix per figure, and another for the same avatar's face. */
const figurePrefix = (id: string): string => `av-f-${id}-`
const facePrefix = (id: string): string => `av-c-${id}-`
const errorText = (e: unknown): string => (e instanceof Error && e.message ? e.message : 'The reply could not be sent.')
/** How far the pointer has to be from a figure for its pupils to look all the way towards it. */
const GAZE_FULL_PX = 260
/** Where the pupils look while a question is open: at the bubble beside the figure. */
const GAZE_AT_BUBBLE = 0.85
/** The pupils follow the pointer for this long after it last moved. */
const GAZE_POINTER_MS = 3000
const within = (el: Element | null | undefined, node: Element | null): boolean => !!el && !!node && el.contains(node)
/** As many avatars walk about; the rest are behind the chip. */
const MAX_ROAMERS = 8
/** What a bubble needs beside its figure: its width and the gap between them. */
const BUBBLE_REACH_PX = 312
/** The gap between two figures of the column they start from. */
const COLUMN_GAP_PX = 8
/** How high a walking figure hops and how far it leans. */
const HOP_PX = 5
const LEAN_DEG = 5
/** What is in the way of the walkers is measured this often. */
const OBSTACLES_EVERY_MS = 120
/** A pointer that has not moved for this long is parked: no figure waits beside it to be pressed. */
const POINTER_FRESH_MS = 2500
/** What stands under the large figure (its name, "Voice off"): it is kept inside the area too. */
const UNDER_PX = 44
/** How high the room is that an avatar takes beside its figure: with an open bubble, and with the app's own line. */
const BUBBLE_ROOM_PX = 280
const LINE_ROOM_PX = 132
/** What a name under a resting figure takes: the feet stay this far above the area's lower edge. */
const LABEL_PX = 22
/** The pointer counts as coming onto a figure only if it moved this lately: a figure that walks under a resting pointer is not pointed at. */
const HOVER_FRESH_MS = 400
/** After another gesture a figure's own walk loop is not started again for this long. */
const AFTER_GESTURE_MS = 1600
/** A figure has grown, and its bubble is measured again, this long after it became the large one. */
const GROWN_AFTER_MS = 230
/** What the user presses or types into: a figure does not come to rest on top of one of these. */
const IN_USE = 'button, a[href], input, textarea, select, summary, label, [role="button"], [role="tab"], [role="menuitem"], [role="option"], [role="link"], [role="checkbox"], [role="switch"], [role="slider"], [role="textbox"], [role="treeitem"], [contenteditable]:not([contenteditable="false"]), .xterm, .monaco-editor'
/** An entry of a record that is keyed by data (a terminal id, a project id): its own entries only, never what an object inherits. */
const own = <V,>(record: Readonly<Record<string, V>>, key: string): V | undefined => (Object.hasOwn(record, key) ? record[key] : undefined)

function useLatest<T>(value: T): { readonly current: T } {
  const ref = useRef(value)
  ref.current = value
  return ref
}

/** One avatar on stage, with its session. */
interface Member {
  id: string
  foreign: boolean
  /** The record as this window presents it (of another project's avatar only its question). */
  record: AvatarInfo
  view: SessionView
}

/** The line this tab presents now, as the speaker tells it. */
interface Present {
  key: string
  terminalId: string
  lineId: string
  engine: 'browser' | 'silent'
  chunks: readonly string[]
  index: number
  text: string
  lang: string | undefined
}

const speakPrefs = (p: { voice: boolean | null; volume: number; speed: number; online: boolean; voices: Record<string, string> }): SpeakPrefs => ({ voice: p.voice === true, volume: p.volume, speed: p.speed, online: p.online, voices: p.voices })

const newestAgentLine = (lines: readonly Line[]): Line | null => {
  for (let i = lines.length - 1; i >= 0; i--) if (lines[i]?.from === 'agent') return lines[i]
  return null
}

// ---------------------------------------------------------------- the app's line for a session that needs the user

interface BlockedFormProps {
  provider: AgentProvider
  title: string
  project: string | null
  name: string
  tail: 'left' | 'right'
  pointer: { current: PointerAt | null }
  onOpen: () => void
}

/** What the pointer does that may end a guard: it moves, it is pressed or lifted (a lifted touch or pen is nowhere). */
const GUARD_EVENTS = ['pointermove', 'pointerdown', 'pointerup', 'pointercancel'] as const

/**
 * Beside a figure whose session needs a permission or an answer: the app's own line and
 * the way to the session, nothing of the agent's. It appears by itself, so it guards
 * against stray clicks like any bubble that does (`guardStep`).
 */
function BlockedForm({ provider, title, project, name, tail, pointer, onOpen }: BlockedFormProps) {
  const root = useRef<HTMLDivElement | null>(null)
  const [guard, setGuard] = useState<Guard | null>(() => armGuard(clock(), pointer.current))
  useEffect(() => {
    if (!guard) return
    const step = () => setGuard((g) => guardStep(g, clock(), inside(root.current, pointer.current), pointer.current))
    // Weighed in the next frame, once the overlay has taken note of where the pointer is now.
    let frame: number | null = null
    const soon = () => {
      if (frame === null)
        frame = window.requestAnimationFrame(() => {
          frame = null
          step()
        })
    }
    const wake = guardWake(guard, clock())
    const timer = wake === null ? null : window.setTimeout(step, Math.max(0, wake - clock()) + 5)
    for (const type of GUARD_EVENTS) window.addEventListener(type, soon, { capture: true, passive: true })
    return () => {
      if (timer !== null) window.clearTimeout(timer)
      if (frame !== null) window.cancelAnimationFrame(frame)
      for (const type of GUARD_EVENTS) window.removeEventListener(type, soon, { capture: true })
    }
  }, [guard, pointer])
  return <Bubble blocked provider={provider} title={title} project={project} name={name} tail={tail} clickThrough={guard?.phase === 'timed'} holding={guard?.phase === 'hold'} rootRef={root} onOpen={onOpen} />
}

// ---------------------------------------------------------------- the active avatar's bubble, with its microphone

interface BubbleHostProps {
  provider: AgentProvider
  title: string
  project: string | null
  name: string
  caption: BubbleCaption | null
  aloud: boolean
  log: readonly Line[] | null
  unseenFrom: string | null
  ask: Ask | null
  disabled: 'dialog' | 'starting' | null
  /** What the reply box holds; null: no reply box. */
  draft: Draft | null
  sending: boolean
  note: BubbleNote | null
  voiceChoice: 'offer' | 'online' | 'none' | null
  locked: boolean
  tail: 'left' | 'right' | null
  panel: boolean
  clickThrough: boolean
  holding: boolean
  figure?: ReactNode
  /** The language to listen for: the question's. */
  lang: string | undefined
  fieldRef: RefObject<HTMLTextAreaElement | null>
  rootRef: (el: HTMLDivElement | null) => void
  onChoice: (index: number) => void
  /** `begunFor`: for dictated words, the question that was on screen when the microphone was pressed. */
  onDraft: (text: string, dictated: boolean, begunFor?: string | null) => void
  onSend: () => void
  onDismiss?: () => void
  onStop?: () => void
  onHear: () => void
  onCaptionsOnly: () => void
  onAllowOnline: () => void
  onUnlock: () => void
  onClose: () => void
  onProblem: (text: string, level: 'info' | 'error') => void
  onListening: (on: boolean) => void
}

/**
 * The bubble with the dictation of its reply box. Dictated words are text in the box
 * until the user sends them, like typed ones; typing, sending or a click on a choice ends
 * the dictation, because late words would overwrite the edit or begin a reply to a question
 * already answered, and so does the box going away.
 */
function BubbleHost(props: BubbleHostProps) {
  const { fieldRef, onDraft, onListening } = props
  const caretAt = useRef<number | null>(null)
  /** The question on screen when the user pressed the microphone: the words that come later answer it. */
  const begunFor = useRef<string | null>(null)
  const dictation = useDictation(
    fieldRef,
    (value, caret) => {
      caretAt.current = caret
      onDraft(value, true, begunFor.current)
    },
    { lang: props.lang, onProblem: (p) => props.onProblem(p.message, p.level) },
  )
  const dictating = {
    ...dictation,
    toggle: () => {
      if (dictation.state === 'idle') begunFor.current = props.ask?.id ?? null
      dictation.toggle()
    },
  }
  // The reply box went (a question that takes choices only, one that closed, a dialog in the
  // session) and with it the microphone's button: the dictation ends rather than listen on.
  const fieldGone = props.draft === null || props.disabled !== null
  const cancel = useLatest(dictation.cancel)
  useEffect(() => {
    if (fieldGone) cancel.current()
  }, [fieldGone, cancel])
  const text = props.draft?.text ?? ''
  // The caret goes where the dictated words end, once React has put them into the box.
  useLayoutEffect(() => {
    const caret = caretAt.current
    if (caret === null) return
    caretAt.current = null
    fieldRef.current?.setSelectionRange(caret, caret)
  }, [text, fieldRef])
  const listening = dictation.state !== 'idle'
  const told = useLatest(onListening)
  useEffect(() => {
    told.current(listening)
    return () => {
      if (listening) told.current(false)
    }
  }, [listening, told])
  return (
    <Bubble
      provider={props.provider}
      title={props.title}
      project={props.project}
      name={props.name}
      caption={props.caption}
      aloud={props.aloud}
      log={props.log}
      unseenFrom={props.unseenFrom}
      ask={props.ask}
      disabled={props.disabled}
      reply={props.draft ? { draft: props.draft.text, sending: props.sending, fieldRef, dictating } : null}
      note={props.note}
      voiceChoice={props.voiceChoice}
      locked={props.locked}
      tail={props.tail}
      panel={props.panel}
      clickThrough={props.clickThrough}
      holding={props.holding}
      figure={props.figure}
      rootRef={props.rootRef}
      onChoice={(index) => {
        // A choice is the answer: what the microphone hears after it would begin a reply to a question already answered.
        dictation.cancel()
        props.onChoice(index)
      }}
      onDraft={(next) => {
        dictation.cancel()
        onDraft(next, false)
      }}
      onSend={() => {
        dictation.cancel()
        props.onSend()
      }}
      onDismiss={props.onDismiss}
      onStop={props.onStop}
      onHear={props.onHear}
      onCaptionsOnly={props.onCaptionsOnly}
      onAllowOnline={props.onAllowOnline}
      onUnlock={props.onUnlock}
      onClose={props.onClose}
    />
  )
}

// ---------------------------------------------------------------- the overlay

export function Overlay({ staged }: { staged: readonly Staged[] }) {
  const list = useAvatars().data
  const terminals = useTerminals().data
  const projects = useProjects().data
  const projectId = useUi((s) => s.projectId)
  const workOpen = useUi((s) => s.workOpen)
  const mobile = useIsMobile()
  const prefs = useAvatarPrefs()
  const voices = useBrowserVoices()
  const voicesListed = useVoicesListed()
  const barSlot = useAvatarRuntime((s) => s.barSlot)
  const columnTab = useAgentsUi((s) => own(s.columnTab, projectId ?? '') ?? null)
  const mobileTerminal = useAgentsUi((s) => s.mobileTerminal)
  const onScreen = mobile ? mobileTerminal : columnTab
  const attention = useAttention()
  const systemReduced = useMedia('(prefers-reduced-motion: reduce)')
  const reducedMotion = prefs.reduceMotion || systemReduced

  // ---- who is on stage

  const viewsRef = useRef<ReadonlyMap<string, SessionView>>(new Map())
  const views = useMemo(() => (viewsRef.current = sessionViews(viewsRef.current, terminals)), [terminals])

  const cast = useMemo<Member[]>(() => {
    const records = new Map<string, AvatarInfo>()
    for (const a of list?.avatars ?? []) records.set(a.terminalId, a)
    const out: Member[] = []
    for (const s of staged) {
      const record = records.get(s.terminalId)
      const view = views.get(s.terminalId)
      if (record && view) out.push({ id: s.terminalId, foreign: s.foreign, record: presented(record, s.foreign), view })
    }
    return out
  }, [staged, list, views])
  const castById = useMemo(() => new Map(cast.map((m) => [m.id, m])), [cast])
  const order = useMemo(() => cast.map((m) => m.id), [cast])

  /** Every agent line the list holds, by the key the speaker knows it by. */
  const lineIndex = useMemo(() => {
    const index = new Map<string, { terminalId: string; line: Line }>()
    for (const a of list?.avatars ?? []) for (const line of a.lines ?? []) if (line?.from === 'agent') index.set(lineKey(a.terminalId, line), { terminalId: a.terminalId, line })
    return index
  }, [list])

  // ---- the rig: one engine, one speaker, one arbiter

  const [rig, setRig] = useState<Rig | null>(null)
  const rigRef = useLatest(rig)
  const [present, setPresentState] = useState<Present | null>(null)
  const presentRef = useRef<Present | null>(null)
  /** Each avatar's line this tab presented last, and when that began: a note about a reply goes when another one is presented (`noteStep`). */
  const lastLines = useRef(new Map<string, LastLine>())
  /** The parts each avatar's last presented line was shown in: its caption keeps them once the line is over, so a screen reader does not announce it again. */
  const presentedParts = useRef(new Map<string, { lineId: string; chunks: readonly string[] }>())
  const setPresent = useCallback((next: Present | null) => {
    const was = presentRef.current
    if (next && (was?.terminalId !== next.terminalId || was.lineId !== next.lineId)) lastLines.current.set(next.terminalId, { lineId: next.lineId, since: clock() })
    if (next) presentedParts.current.set(next.terminalId, { lineId: next.lineId, chunks: next.chunks })
    presentRef.current = next
    setPresentState(next)
  }, [])
  const [unlocked, setUnlockedState] = useState(() => hadGesture(window))
  const unlockedRef = useRef(unlocked)
  /** The speaker has said its empty utterance inside a user gesture since voice was turned on. */
  const primed = useRef(false)
  const handles = useRef(new Map<string, FigureHandle>())
  const lineIndexRef = useLatest(lineIndex)
  const stagedIds = useMemo(() => new Set(staged.map((s) => s.terminalId)), [staged])
  const stagedIdsRef = useLatest(stagedIds)
  const viewsNow = useLatest(views)
  const onScreenRef = useLatest(onScreen)

  const arbiterState = useCallback((): ArbiterState => {
    const p = useAvatarPrefs.getState()
    return {
      voice: p.voice === true,
      unlocked: unlockedRef.current,
      show: p.show,
      shown: (tid) => stagedIdsRef.current.has(tid),
      blocked: (tid) => viewsNow.current.get(tid)?.blocked === true,
      onScreen: (tid) => onScreenRef.current === tid,
    }
  }, [stagedIdsRef, viewsNow, onScreenRef])

  const setUnlocked = useCallback(
    (on: boolean) => {
      if (unlockedRef.current === on) return
      unlockedRef.current = on
      setUnlockedState(on)
      rigRef.current?.arbiter.setState(arbiterState())
    },
    [arbiterState, rigRef],
  )

  useEffect(() => {
    const made = createRig(window, speakPrefs(useAvatarPrefs.getState()), arbiterState(), {
      start(key, engine, chunks, voiceName) {
        const hit = lineIndexRef.current.get(key)
        const terminalId = hit?.terminalId ?? made.speaker.current()?.terminalId
        if (!terminalId) return
        const first = presentRef.current?.key !== key
        setPresent({ key, terminalId, lineId: hit?.line.id ?? '', engine, chunks, index: 0, text: hit?.line.text ?? chunks.join(' '), lang: hit?.line.lang })
        // The avatar keeps the voice it got: the next page load picks the same one.
        if (engine === 'browser' && voiceName) useAvatarPrefs.getState().rememberVoice(terminalId, voiceName)
        // A gesture the line came with plays as the line begins.
        if (first && typeof hit?.line.gesture === 'string' && handles.current.get(terminalId)?.play(hit.line.gesture, 'once')) otherGesture.current(terminalId)
      },
      chunk(key, index) {
        const p = presentRef.current
        if (p && p.key === key && p.index !== index) setPresent({ ...p, index })
      },
      end(key) {
        if (presentRef.current?.key === key) setPresent(null)
      },
      locked() {
        // The browser refused to speak: a click is needed first.
        primed.current = false
        unlockedRef.current = false
        setUnlockedState(false)
        made.arbiter.setState(arbiterState())
      },
    })
    setRig(made)
    // The page goes away: speech stops and the leases are given back, while a request can still leave.
    const leaving = () => made.arbiter.release()
    window.addEventListener('pagehide', leaving)
    return () => {
      window.removeEventListener('pagehide', leaving)
      made.dispose()
      presentRef.current = null
      setPresentState(null)
      setRig(null)
    }
  }, [arbiterState, lineIndexRef, setPresent])

  // The preferences reach the speaker from every window of this browser (a storage event too): turning voice off stops the line.
  const { voice, volume, speed, online, voices: remembered, show } = prefs
  useEffect(() => {
    rig?.speaker.setPrefs(speakPrefs({ voice, volume, speed, online, voices: remembered }))
  }, [rig, voice, volume, speed, online, remembered])

  // What the arbiter decides with, whenever any of it changed.
  const blockedIds = useMemo(() => cast.filter((m) => m.view.blocked).map((m) => m.id).join(','), [cast])
  const stagedKey = useMemo(() => staged.map((s) => `${s.terminalId}${s.foreign ? '*' : ''}`).join(','), [staged])
  useEffect(() => {
    rig?.arbiter.setState(arbiterState())
  }, [rig, arbiterState, voice, show, unlocked, stagedKey, blockedIds, onScreen])

  // Every record is offered on every change of the list: a line said before this chunk loaded, or learned from a refetch, takes the same path as a live one.
  const offered = useRef(new Set<string>())
  useEffect(() => {
    if (!rig) return
    const foreign = new Set(staged.filter((s) => s.foreign).map((s) => s.terminalId))
    const now = new Set<string>()
    for (const a of list?.avatars ?? []) {
      now.add(a.terminalId)
      rig.arbiter.offer(presented(a, foreign.has(a.terminalId)))
    }
    for (const id of offered.current) if (!now.has(id)) rig.arbiter.forget(id)
    offered.current = now
  }, [rig, list, staged])
  useEffect(
    () => () => {
      offered.current = new Set()
    },
    [rig],
  )

  // A line got its outcome on some device: one that is said here stops when it was skipped there.
  useEvent<AvatarLineChanged>(AVATAR_EVENTS.line, (ev) => {
    const d = ev.data
    if (d && typeof d.terminalId === 'string' && d.line) rigRef.current?.arbiter.settled(d.terminalId, d.line)
  })
  useEvent<{ id?: unknown }>('terminal.removed', (ev) => {
    if (typeof ev.data?.id === 'string') rigRef.current?.arbiter.forget(ev.data.id)
  })

  // Speech is locked until the page has had a user gesture; the first one unlocks it (the speaker says nothing, inside the handler).
  useEffect(
    () =>
      onGesture(() => {
        const r = rigRef.current
        if (!r) return
        if (useAvatarPrefs.getState().voice === true && !primed.current) {
          primed.current = true
          r.speaker.unlock()
        }
        setUnlocked(true)
      }),
    [rigRef, setUnlocked],
  )

  // Gestures an event asked for: played once, or stopped. Held ones are the record's, and the figure plays them itself.
  // Every event comes once (one subscription, one emit), and the server's numbers start again at 0 when it restarts: nothing is dropped as a repeat.
  useEffect(
    () =>
      onAct((act) => {
        const handle = handles.current.get(act.terminalId)
        if (!handle) return
        if (typeof act.stop === 'string') handle.stop(act.stop)
        else if (typeof act.play === 'string') {
          handle.play(act.play, 'once', act.times)
          otherGesture.current(act.terminalId)
        }
      }),
    [],
  )

  // ---- what the user is doing with it

  const [scene, setScene] = useState<Scene>(EMPTY)
  const sceneRef = useRef(scene)
  const [, setTick] = useState(0)
  const [drafts, setDrafts] = useState<ReadonlyMap<string, Draft>>(() => new Map())
  const [notes, setNotes] = useState<ReadonlyMap<string, Noted<BubbleNote>>>(() => new Map())
  const [sending, setSending] = useState<ReadonlySet<string>>(() => new Set())
  const [voiceForms, setVoiceForms] = useState<ReadonlyMap<string, 'online' | 'none'>>(() => new Map())
  const [listeningId, setListeningId] = useState<string | null>(null)
  /** Where the transcript the user opened starts to read: taken when it opened, because opening it is what makes its lines seen. */
  const [logFrom, setLogFrom] = useState<{ id: string; from: string | null } | null>(null)
  const [hovered, setHovered] = useState<string | null>(null)
  const [focus, setFocus] = useState<{ id: string | null; inBubble: boolean; inReply: boolean }>({ id: null, inBubble: false, inReply: false })
  const [over, setOver] = useState<{ group: boolean; bubble: boolean }>({ group: false, bubble: false })
  const [drag, setDrag] = useState<{ from: Anchor; dx: number; dy: number } | null>(null)
  const [groupEl, setGroupEl] = useState<HTMLDivElement | null>(null)
  const [bubbleEl, setBubbleEl] = useState<HTMLDivElement | null>(null)
  const [faceEl, setFaceEl] = useState<HTMLButtonElement | null>(null)
  const [shift, setShift] = useState(0)
  /** The right edge of the face in the desktop shell's top bar: the compact panel's right edge is under it. */
  const [faceRight, setFaceRight] = useState<number | null>(null)
  const slots = useRef(new Map<string, HTMLDivElement>())
  const fieldRef = useRef<HTMLTextAreaElement | null>(null)
  const moreRef = useRef<HTMLButtonElement | null>(null)
  /** Where focus was before it went into the reply box. */
  const returnTo = useRef<HTMLElement | null>(null)
  /** The reply box that holds focus now, if one does. */
  const replyFocused = useRef<HTMLTextAreaElement | null>(null)
  /** The palette asked for the reply box of this avatar: focus goes into it once it is there. */
  const wantsReply = useRef<string | null>(null)
  const gaze = useRef(new Map<string, readonly [number, number]>())
  const atBubble = useRef(new Map<string, readonly [number, number]>())
  // The walk (lib/roam.ts): where every figure is, what each is told at the next step, and what a frame wrote last.
  const world = useRef<World>(newWorld())
  const walkers = useRef<Walker[]>([])
  /** Where an avatar that is wanted stands and on which side its bubble opens: kept while it is wanted. */
  const posts = useRef(new Map<string, Post>())
  /** The avatar the user drags, and where the drag has it. */
  const roamDrag = useRef<{ id: string; x: number; y: number } | null>(null)
  const [draggedId, setDraggedId] = useState<string | null>(null)
  /** Where a figure looks when it looks at another one. */
  const roamLooks = useRef(new Map<string, readonly [number, number]>())
  /** The figures that play a walk of their own design now, and until when one that just played another gesture does not start it. */
  const ownWalk = useRef(new Set<string>())
  const gestured = useRef(new Map<string, number>())
  /** The area the posts were found in: when it changes they are found again. */
  const postsArea = useRef('')
  /** Another gesture was played on a figure: a walk loop of its own, if it played one, is over (the engine plays one gesture at a time). */
  const otherGesture = useRef((id: string) => {
    ownWalk.current.delete(id)
    gestured.current.set(id, clock() + AFTER_GESTURE_MS)
  })
  /** Sessions that finished since the last step: those near them cheer. */
  const finished = useRef<string[]>([])
  const lastStates = useRef(new Map<string, AvatarState>())
  /** The avatars that reached the place their bubble has room at. */
  const posted = useRef(new Set<string>())
  const written = useRef(new Map<string, string>())
  const obstacles = useRef<{ at: number; boxes: Obstacle[] }>({ at: Number.NEGATIVE_INFINITY, boxes: [] })

  const put = <K, V>(set: (update: (prev: ReadonlyMap<K, V>) => ReadonlyMap<K, V>) => void, key: K, value: V | null) =>
    set((prev) => {
      if (value === null ? !prev.has(key) : prev.get(key) === value) return prev
      const next = new Map(prev)
      if (value === null) next.delete(key)
      else next.set(key, value)
      return next
    })
  /**
   * A note for an avatar's bubble, about what begins now. A line of the avatar presented
   * after it takes it away, except `line`: by default the line presented last, which may be
   * said again. (A note about a reply that came back is `replyNoted`'s.)
   */
  const setNote = (id: string, note: BubbleNote | null, line?: string | null) => {
    put(setNotes, id, note === null ? null : noted(note, clock(), line === undefined ? (lastLines.current.get(id)?.lineId ?? null) : line))
  }
  /** The user changed the draft: an error or an info about the last reply goes (`noteEdited`). */
  const draftEdited = (id: string) =>
    setNotes((prev) => {
      const n = prev.get(id)
      if (!n || noteEdited(n) === n) return prev
      const next = new Map(prev)
      next.delete(id)
      return next
    })
  const setDraft = (id: string, draft: Draft) => put(setDrafts, id, draft.text === '' ? null : draft)

  // ---- geometry

  const geometry = useGeometry(mobile, workOpen, barSlot)
  const mode = layoutMode({ mobile, workOpen, work: geometry.work })
  /** The figures walk about the workspace window; false: they stand in their column. */
  const roaming = mode === 'wide' && prefs.roam && !reducedMotion
  const groupSize = useSize(mode === 'wide' ? groupEl : null)
  const small = smallFigures(mode, geometry.viewport.width)
  const sizes = small ? FIGURE_PX_PHONE : FIGURE_PX
  /** The compact form while the session on screen has a permission request to answer: the panel stays out of its way. */
  const held = mode !== 'wide' && onScreen !== null && views.get(onScreen)?.pending === true

  const placement = useMemo<Placement | null>(() => {
    if (mode !== 'wide' || !geometry.work) return null
    const size = groupSize ?? { width: 0, height: 0 }
    const base: Anchor = drag ? dragged(drag.from, drag.dx, drag.dy) : prefs.pos ? storedAnchor(prefs.pos, geometry.viewport, geometry.work) : defaultAnchor(geometry.dock ?? geometry.work)
    return place(base, size, geometry.work, geometry.viewport)
  }, [mode, geometry, groupSize, drag, prefs.pos])
  const placementRef = useLatest(placement)

  // ---- the scene

  const pointer = usePointer(() => {
    recheck.current()
    // A bubble that holds a resting pointer lets go once the pointer has moved on: while it holds, every move is weighed.
    const g = sceneRef.current.guard
    if (g && (g.phase === 'hold' || g.from === null)) setTick((n) => n + 1)
  })
  const facts = (): Facts => ({
    now: clock(),
    // A kept draft holds the stage only while its reply box is on screen: hidden, it is only kept.
    shown: cast.map<Candidate>((m) => ({ id: m.id, blocked: m.view.blocked, askId: m.record.ask?.id ?? null, draft: replyShown(sceneRef.current, m.id, m.record.ask) && (drafts.get(m.id)?.text ?? '').trim() !== '', guest: m.foreign })),
    onScreen,
    presenting: present ? { terminalId: present.terminalId, lineId: present.lineId, silent: present.engine === 'silent', chars: present.text.length } : null,
    // The one face of the compact forms stands for whoever is in front: under the pointer it does not become another's.
    hovered: mode === 'wide' ? hovered : over.group ? frontId : null,
    focused: focus.id,
    overGroup: over.group,
    inBubble: over.bubble,
    pointer: pointer.current,
    focusInBubble: focus.inBubble,
    focusInReply: focus.inReply,
    // The microphone lives in the active avatar's bubble: a click on it leaves focus where it was.
    listening: listeningId !== null && listeningId === sceneRef.current.active.id,
    held,
  })
  const act = (next: Scene) => {
    if (next === sceneRef.current) return
    sceneRef.current = next
    setScene(next)
  }
  // After every render: the scene as the facts have it now. Nothing happens while they say the same.
  useLayoutEffect(() => {
    act(settle(sceneRef.current, facts()))
  })
  useEffect(() => {
    if (scene.wakeAt === null) return
    const timer = window.setTimeout(() => setTick((n) => n + 1), Math.max(0, scene.wakeAt - clock()) + 4)
    return () => window.clearTimeout(timer)
  }, [scene.wakeAt])

  const activeId = scene.active.id
  const bubbleOpen = scene.bubble !== 'closed'
  /** Those that wait with a question the user can answer here. A guest among them rests: where there is one face only, it is its face. */
  const askers = useMemo(() => cast.filter((m) => m.record.ask !== null && !m.view.blocked).map((m) => m.id), [cast])
  const frontId = front(order, activeId, onScreen, askers)
  const frontRef = useLatest(frontId)
  const { visible, more } = useMemo(
    () => (mode === 'wide' ? figures(order, activeId, roaming ? MAX_ROAMERS : undefined) : { visible: frontId ? [frontId] : [], more: order.filter((id) => id !== frontId) }),
    [mode, roaming, order, activeId, frontId],
  )

  /** Where the pointer and focus are, measured against what is on screen now. */
  const recheck = useLatest(() => {
    const at = pointer.current
    // Walking figures are no group: the pointer is over "the group" when it is over one of them.
    const overFigures = roaming ? [...slots.current.values()].some((el) => inside(el, at)) || inside(groupEl, at) : inside(mode === 'wide' ? groupEl : faceEl, at)
    const next = { group: overFigures, bubble: inside(bubbleEl, at) }
    setOver((prev) => (prev.group === next.group && prev.bubble === next.bubble ? prev : next))
    // Where each figure's pupils look: towards the pointer, further the further away it is.
    gaze.current.clear()
    if (!at) return
    for (const [id, el] of slots.current) {
      const r = el.getBoundingClientRect()
      const dx = at.x - (r.left + r.width / 2)
      const dy = at.y - (r.top + Math.min(r.width, r.height) / 2)
      const far = Math.max(Math.hypot(dx, dy), GAZE_FULL_PX)
      gaze.current.set(id, [dx / far, dy / far])
    }
  })
  // A bubble that opens under a pointer that does not move is tested against it all the same.
  useLayoutEffect(() => {
    recheck.current()
  })
  // And where focus is: an element that is taken away while it holds focus tells nobody.
  useLayoutEffect(() => {
    noteFocus.current()
  })

  const noteFocus = useLatest(() => {
    const el = document.activeElement
    let next = { id: null as string | null, inBubble: false, inReply: false }
    if (el && el !== document.body) {
      if (within(bubbleEl, el)) next = { id: sceneRef.current.active.id, inBubble: true, inReply: el === fieldRef.current }
      else if (within(faceEl, el)) next = { id: frontRef.current, inBubble: false, inReply: false }
      else for (const [id, slot] of slots.current) if (slot.contains(el)) next = { id, inBubble: false, inReply: false }
    }
    setFocus((prev) => (prev.id === next.id && prev.inBubble === next.inBubble && prev.inReply === next.inReply ? prev : next))
  })
  const onFocus = (e: ReactFocusEvent) => {
    if (e.target === fieldRef.current && fieldRef.current !== null) {
      // Remember where focus came from when it enters the reply box, to give it back when the box closes.
      if (replyFocused.current === null && e.relatedTarget instanceof HTMLElement && !within(bubbleEl, e.relatedTarget)) returnTo.current = e.relatedTarget
      replyFocused.current = fieldRef.current
    }
    noteFocus.current()
  }
  const onBlur = (e: ReactFocusEvent) => {
    // The user moved focus out of the reply box (a box that is taken away while it holds focus is another matter, below).
    if (e.target === replyFocused.current && replyFocused.current?.isConnected) {
      replyFocused.current = null
      if (!within(bubbleEl, e.relatedTarget instanceof Element ? e.relatedTarget : null)) returnTo.current = null
    }
    // Where focus goes is known only after it left.
    window.setTimeout(() => noteFocus.current(), 0)
  }
  // The reply box closed while it held focus (the question was answered, the bubble closed, another avatar took the stage): focus returns to the element that had it before.
  useLayoutEffect(() => {
    const field = replyFocused.current
    if (field === null || field.isConnected) return
    replyFocused.current = null
    const back = returnTo.current
    returnTo.current = null
    if (back?.isConnected) back.focus({ preventScroll: true })
    noteFocus.current()
  })

  const openFor = (id: string) => {
    const m = castById.get(id)
    if (!m) return false
    setLogFrom({ id, from: transcript(m.record.lines, own(prefs.seen, id)).unseenFrom })
    // A session that needs the user: its transcript all the same, with the reply disabled.
    act(openByUser(sceneRef.current, id, m.view.blocked))
    return true
  }
  const closeBubble = () => act(closeByUser(sceneRef.current, clock()))
  const press = (id: string) => {
    if (sceneRef.current.active.id === id && sceneRef.current.bubble !== 'closed') closeBubble()
    else openFor(id)
  }

  // ---- what happened to which line: seen, waved, nodded

  // A line counts as seen once its caption was on screen in a visible, focused window, or its transcript was opened.
  useEffect(() => {
    if (!attention.visible || !attention.focused || !activeId || !bubbleOpen) return
    const m = castById.get(activeId)
    if (!m) return
    const seen = own(prefs.seen, activeId)
    let next: string | undefined = seen
    if (scene.bubble === 'user') next = unseen(m.record.lines, seen).newest ?? seen
    else {
      for (const shownId of [m.record.ask?.lineId, scene.lineId]) if (shownId) next = seenAfter(m.record.lines, next, shownId)
    }
    if (next !== undefined && next !== seen) prefs.markSeen(activeId, next)
  }, [attention, activeId, bubbleOpen, scene.bubble, scene.lineId, castById, prefs])

  // The first design of a session: its avatar waves once.
  const wasDesigned = useRef(new Map<string, boolean>())
  useEffect(() => {
    const next = new Map<string, boolean>()
    for (const m of cast) {
      const designed = m.record.designed === true
      if (wasDesigned.current.get(m.id) === false && designed && handles.current.get(m.id)?.play('wave', 'once')) otherGesture.current(m.id)
      next.set(m.id, designed)
    }
    wasDesigned.current = next
  }, [cast])

  // What was kept for an avatar goes when it leaves the stage, and what belonged to a bubble when that closes.
  useEffect(() => {
    const keep = <V,>(prev: ReadonlyMap<string, V>): ReadonlyMap<string, V> => ([...prev.keys()].every((id) => castById.has(id)) ? prev : new Map([...prev].filter(([id]) => castById.has(id))))
    setDrafts(keep)
    setNotes(keep)
    setVoiceForms(keep)
    setSending((prev) => ([...prev].every((id) => castById.has(id)) ? prev : new Set([...prev].filter((id) => castById.has(id)))))
    for (const id of lastLines.current.keys()) if (!castById.has(id)) lastLines.current.delete(id)
    for (const id of presentedParts.current.keys()) if (!castById.has(id)) presentedParts.current.delete(id)
  }, [castById])
  const openKey = bubbleOpen ? activeId : null
  useEffect(() => {
    if (openKey === null) return
    return () => {
      setNotes((prev) => (prev.has(openKey) ? new Map([...prev].filter(([id]) => id !== openKey)) : prev))
      setVoiceForms((prev) => (prev.has(openKey) ? new Map([...prev].filter(([id]) => id !== openKey)) : prev))
    }
  }, [openKey])

  // A note goes by itself (`noteStep`): when another line of its avatar is presented, and "Sent." a few seconds after it appeared at the latest.
  const presentingKey = present ? `${present.terminalId}\n${present.lineId}` : ''
  useEffect(() => {
    const step = () =>
      setNotes((prev) => {
        const now = clock()
        let next: Map<string, Noted<BubbleNote>> | null = null
        for (const [id, n] of prev) {
          if (noteStep(n, now, lastLines.current.get(id)) === n) continue
          next ??= new Map(prev)
          next.delete(id)
        }
        return next ?? prev
      })
    step()
    let wake: number | null = null
    for (const n of notes.values()) {
      const at = noteWake(n)
      if (at !== null && (wake === null || at < wake)) wake = at
    }
    if (wake === null) return
    const timer = window.setTimeout(step, Math.max(0, wake - clock()) + 4)
    return () => window.clearTimeout(timer)
  }, [notes, presentingKey])
  // The question on screen changed under a draft: the bubble says so, with the new question's line.
  const asksSeen = useRef(new Map<string, string | null>())
  useEffect(() => {
    const seen = new Map<string, string | null>()
    for (const m of cast) {
      const ask = m.record.ask?.id ?? null
      if (questionChanged(drafts.get(m.id), asksSeen.current.get(m.id), ask)) setNote(m.id, { kind: 'changed' }, m.record.ask?.lineId ?? null)
      seen.set(m.id, ask)
    }
    asksSeen.current = seen
  }, [cast, drafts])

  // "Designing its look…" ends by itself three minutes after the session started.
  useEffect(() => {
    let at: number | null = null
    const now = Date.now()
    for (const m of cast) {
      const ends = m.view.createdAt + DESIGNING_MS
      if (m.view.withOption && !m.record.designed && ends > now && (at === null || ends < at)) at = ends
    }
    if (at === null) return
    const timer = window.setTimeout(() => setTick((n) => n + 1), at - now + 50)
    return () => window.clearTimeout(timer)
  }, [cast])

  // ---- voice

  /** The preferences as they are now reach the speaker and the arbiter at once: this runs inside the click that changed them. */
  const applyVoice = () => {
    const r = rigRef.current
    if (!r) return
    const p = useAvatarPrefs.getState()
    r.speaker.setPrefs(speakPrefs(p))
    if (p.voice === true) {
      primed.current = true
      r.speaker.unlock()
      unlockedRef.current = true
      setUnlockedState(true)
    }
    r.arbiter.setState(arbiterState())
  }
  const replay = (m: Member, line: Line | null) => {
    if (!line) return
    // The user asked for it: the bubble does not take its caption for a change by itself.
    act(askedToHear(sceneRef.current, m.id, line.id))
    rigRef.current?.arbiter.replay(m.record, line)
  }
  /** "Hear it": what this browser can say the line's language with decides what the click does. */
  const hear = (m: Member, line: Line | null) => {
    const kind = voicesFor(voices, line?.lang ?? m.record.voice?.lang ?? 'en-US')
    if (kind === 'none') return put(setVoiceForms, m.id, 'none')
    if (kind === 'online' && !prefs.online) return put(setVoiceForms, m.id, 'online')
    put(setVoiceForms, m.id, null)
    prefs.setPrefs({ voice: true })
    applyVoice()
    replay(m, line)
  }
  const allowOnline = (m: Member, line: Line | null) => {
    put(setVoiceForms, m.id, null)
    prefs.setPrefs({ online: true, voice: true })
    applyVoice()
    replay(m, line)
  }
  const unlock = (m: Member, line: Line | null) => {
    applyVoice()
    replay(m, line)
  }
  /** Voice on or off from the menu, the status bar or the palette. Turning it on is a click too. */
  const turnVoice = useLatest((on: boolean, lang?: string) => {
    if (!on) {
      prefs.setPrefs({ voice: false })
      applyVoice()
      return
    }
    const frontMember = frontId ? castById.get(frontId) : undefined
    const kind = voicesFor(voices, lang ?? frontMember?.record.voice?.lang ?? 'en-US')
    if (kind === 'none') {
      toast('info', WORDS.noVoices)
      return
    }
    if (kind === 'online' && !prefs.online) {
      void confirmDialog({ title: 'Allow online voices?', message: WORDS.online, confirmLabel: WORDS.allowOnline }).then((ok) => {
        if (!ok) return
        useAvatarPrefs.getState().setPrefs({ online: true, voice: true })
        applyVoice()
      })
      return
    }
    prefs.setPrefs({ voice: true })
    applyVoice()
  })
  const stopSpeaking = useLatest((): boolean => {
    const r = rigRef.current
    if (!r || !r.speaker.current()) return false
    // What this tab presents, aloud or as a caption: the arbiter tells the server it was skipped.
    r.speaker.stop()
    return true
  })

  // ---- answering

  const deliver = async (id: string, body: ReplyBody, clearDraft: boolean) => {
    const working = castById.get(id)?.view.working === true
    // When the reply went, and the line on screen then: "Sent." goes with a line the avatar says while it is on its way (`replyNoted`).
    const sent = { line: lastLines.current.get(id)?.lineId ?? null, since: clock() }
    setSending((prev) => new Set(prev).add(id))
    setNote(id, null)
    try {
      const answer = await avatarApi.reply(id, body)
      if (clearDraft) setDraft(id, NO_DRAFT)
      put(setNotes, id, replyNoted<BubbleNote>({ kind: sentNote(answer.delivered, working) }, clock(), sent, lastLines.current.get(id)))
      // The avatar nods once: the reply arrived.
      if (handles.current.get(id)?.play('nod', 'once')) otherGesture.current(id)
    } catch (e) {
      // A refusal is shown where the user is looking, not as a toast, and stays until the next line.
      put(setNotes, id, replyNoted<BubbleNote>({ kind: 'error', text: errorText(e) }, clock(), sent, lastLines.current.get(id)))
    } finally {
      setSending((prev) => {
        const next = new Set(prev)
        next.delete(id)
        return next
      })
    }
  }
  const send = (m: Member) => {
    if (sending.has(m.id)) return
    const plan = planSend(drafts.get(m.id), m.record.ask)
    if (plan.kind === 'confirm') {
      // Enter only took note of the change: the bubble says why nothing was sent. The note is
      // about the question now on screen, so it stays when that question's line is presented.
      setDraft(m.id, plan.draft)
      setNote(m.id, { kind: 'changed' }, m.record.ask?.lineId ?? null)
    } else if (plan.kind === 'send') void deliver(m.id, plan.body, true)
  }
  const choose = (m: Member, ask: Ask | null, index: number) => {
    if (sending.has(m.id)) return
    const body = planChoice(ask, index, sceneRef.current, clock())
    if (body) void deliver(m.id, body, false)
  }
  const dismiss = (m: Member, ask: Ask) => {
    setNote(m.id, null)
    avatarApi.dismissAsk(m.id, ask.id).catch((e: unknown) => setNote(m.id, { kind: 'error', text: errorText(e) }))
  }

  // ---- what the rest of the app sees and asks

  const visibleOwn = useMemo(() => visible.filter((id) => castById.get(id)?.foreign === false), [visible, castById])
  const tell = useAvatarRuntime((s) => s.tell)
  const presentingId = present?.terminalId ?? null
  const presentingLine = present?.lineId ?? null
  const aloudNow = present?.engine === 'browser'
  useEffect(() => {
    tell({
      staged: attention.visible ? visibleOwn : [],
      active: activeId,
      open: activeId !== null && scene.bubble !== 'closed' ? { terminalId: activeId, by: scene.bubble } : null,
      presenting: presentingId !== null && presentingLine !== null ? { terminalId: presentingId, lineId: presentingLine, aloud: aloudNow } : null,
      locked: voice === true && !unlocked,
    })
  }, [tell, attention.visible, visibleOwn, activeId, scene.bubble, presentingId, presentingLine, aloudNow, voice, unlocked])
  const openForRef = useLatest(openFor)
  useEffect(() => {
    setOverlayActions({
      stop: () => stopSpeaking.current(),
      open: (id) => openForRef.current(id),
      reply: (id) => {
        const target = id ?? sceneRef.current.active.id ?? frontRef.current
        if (!target || !openForRef.current(target)) return false
        // The one thing that moves focus into the reply box by itself: the user asked for it by name.
        wantsReply.current = target
        setTick((n) => n + 1)
        return true
      },
      voice: (on) => turnVoice.current(on),
    })
    return () => {
      setOverlayActions(null)
      tell({ staged: [], active: null, open: null, presenting: null, locked: false })
    }
  }, [tell, stopSpeaking, openForRef, frontRef, turnVoice])
  useEffect(() => {
    if (wantsReply.current === null || wantsReply.current !== openKey) return
    // Asked once, for the bubble that opened for it: a reply box that appears some time later takes no focus by itself.
    wantsReply.current = null
    if (!fieldRef.current) return
    const from = document.activeElement
    if (from instanceof HTMLElement && from !== fieldRef.current && !within(bubbleEl, from)) returnTo.current = from
    fieldRef.current.focus({ preventScroll: true })
  })

  // The frame loop runs only while the tab is visible and a figure is on screen.
  const figuresOnScreen = mode === 'wide' ? visible.length > 0 : bubbleOpen && frontId !== null
  useEffect(() => {
    rig?.engine.setRunning(attention.visible && figuresOnScreen)
  }, [rig, attention.visible, figuresOnScreen])

  // ---- dragging the group

  const pressAt = useRef<{ pointer: number; x: number; y: number; dx: number; dy: number; from: Anchor; moved: boolean } | null>(null)
  /** A drag just ended: the click that follows it is not a press on the figure. */
  const justDragged = useRef(false)
  /** Where a drag that ends now leaves the group: stored as fractions of the viewport, clamped into the workspace window. */
  const drop = useLatest((from: Anchor, dx: number, dy: number) => {
    setDrag(null)
    if (!geometry.work) return
    const size = groupSize ?? { width: 0, height: 0 }
    prefs.setPrefs({ pos: toStored(place(dragged(from, dx, dy), size, geometry.work, geometry.viewport), size, geometry.work, geometry.viewport) })
  })
  const endPress = useRef<() => void>(() => {})
  const onGroupDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.pointerType === 'mouse' && e.button !== 0) return
    const target = e.target as Element | null
    const p = placementRef.current
    // The figures are the handle: a press on a bubble or a control moves nothing.
    if (!p || !target?.closest?.('.wb-av-figure-btn')) return
    endPress.current()
    const group = e.currentTarget
    pressAt.current = { pointer: e.pointerId, x: e.clientX, y: e.clientY, dx: 0, dy: 0, from: { x: p.x, y: p.y, edge: p.edge }, moved: false }
    const move = (ev: PointerEvent) => {
      const at = pressAt.current
      if (!at || ev.pointerId !== at.pointer) return
      at.dx = ev.clientX - at.x
      at.dy = ev.clientY - at.y
      if (!at.moved) {
        // A press becomes a drag after a few pixels: until then it is a click on the figure.
        if (!dragStarts(at.dx, at.dy)) return
        at.moved = true
        try {
          group.setPointerCapture(at.pointer)
        } catch {
          // A pointer that is gone already: the window's listeners still follow it.
        }
      }
      setDrag({ from: at.from, dx: at.dx, dy: at.dy })
    }
    // The drag ends when the pointer is released, cancelled, or its capture is lost.
    const end = () => {
      window.removeEventListener('pointermove', move, true)
      window.removeEventListener('pointerup', end, true)
      window.removeEventListener('pointercancel', end, true)
      group.removeEventListener('lostpointercapture', end)
      endPress.current = () => {}
      const at = pressAt.current
      pressAt.current = null
      if (!at?.moved) return
      justDragged.current = true
      window.setTimeout(() => (justDragged.current = false), 80)
      drop.current(at.from, at.dx, at.dy)
    }
    endPress.current = end
    window.addEventListener('pointermove', move, true)
    window.addEventListener('pointerup', end, true)
    window.addEventListener('pointercancel', end, true)
    group.addEventListener('lostpointercapture', end)
  }
  useEffect(() => () => endPress.current(), [])
  const onGroupClick = (e: ReactMouseEvent) => {
    if (!justDragged.current) return
    e.stopPropagation()
    e.preventDefault()
  }

  // ---- walking about (lib/roam.ts)

  const castByIdRef = useLatest(castById)
  /** Where a figure appears: its place in the column the figures stand in when they do not walk. */
  const spawnAt = (index: number): Point => {
    const p = placementRef.current
    if (!p) return { x: 0, y: 0 }
    const pitch = sizes.rest + COLUMN_GAP_PX
    // The chip for those behind it stands where the column begins: the figures start beside it, not on it.
    const n = index + (more.length > 0 ? 1 : 0)
    return { x: p.x, y: p.edge === 'bottom' ? p.y - n * pitch : p.y + sizes.rest + n * pitch }
  }
  /** Where the figures walk: the dock area (not its stripes of tool-window buttons), else the whole workspace window. */
  const walkArea = geometry.dock ?? geometry.work
  /** Where feet may be: the whole resting figure stays inside that area. */
  const feetArea = (): Box | null => (walkArea ? feetBounds({ ...walkArea, bottom: Math.max(walkArea.top, walkArea.bottom - LABEL_PX) }, sizes.rest, EDGE_PX, SIDE_EDGE_PX) : null)
  /** An animation of this name in the avatar's own design (`walk`, `greet`), and whether it loops. */
  const designedAs = (id: string, name: string): { loop: boolean } | null => {
    const m = castByIdRef.current.get(id)
    if (!m) return null
    const drawn = drawnLook(m.record.look, m.id)
    const animation = drawn.own && Object.hasOwn(drawn.look.animations, name) ? drawn.look.animations[name] : undefined
    return animation ? { loop: animation.loop === true } : null
  }
  /** Every figure put where the walk has it, with what follows from the walk: its hop and lean, where it looks, a walk of its own design. */
  const showWalk = () => {
    const bodies = world.current.bodies
    const half = sizes.rest / 2
    roamLooks.current.clear()
    for (const [id, el] of slots.current) {
      const b = bodies.get(id)
      if (!b) continue
      const handle = handles.current.get(id)
      if (b.walking && !ownWalk.current.has(id)) {
        // A looping `walk` of its own design replaces the hop, unless the session holds a gesture or the figure plays another one just now.
        const free = castByIdRef.current.get(id)?.record.held == null && clock() >= (gestured.current.get(id) ?? 0)
        if (handle && free && designedAs(id, 'walk')?.loop && handle.play('walk', 'loop')) ownWalk.current.add(id)
      } else if (!b.walking && ownWalk.current.delete(id)) {
        handle?.stop('walk')
      }
      const hop = ownWalk.current.has(id) ? 0 : -b.hop * HOP_PX
      // Whole pixels while it stands, so that the text of its bubble is sharp; fractions while it walks, so that it glides.
      const [x, y] = b.walking ? [b.x.toFixed(1), b.y.toFixed(1)] : [String(Math.round(b.x)), String(Math.round(b.y))]
      const now = `${x} ${y} ${hop.toFixed(1)} ${(b.lean * LEAN_DEG).toFixed(1)}`
      if (written.current.get(id) !== now || el.style.transform === '') {
        written.current.set(id, now)
        el.style.transform = `translate3d(${x}px, ${y}px, 0) translate(-50%, -100%)`
        // The hop and the lean are the figure's own: written on its button, so that nothing else is styled anew every frame.
        const button = el.querySelector<HTMLElement>('.wb-av-figure-btn')
        if (button) button.style.transform = hop === 0 && b.lean === 0 ? '' : `translateY(${hop.toFixed(1)}px) rotate(${(b.lean * LEAN_DEG).toFixed(1)}deg)`
      }
      const other = b.looks ? bodies.get(b.looks) : undefined
      if (other) roamLooks.current.set(id, gazeToward({ x: b.x, y: b.y - half }, { x: other.x, y: other.y - half }, GAZE_FULL_PX / 2))
    }
  }
  /** What a meeting asks of a figure: a greeting of its own design or a wave, a nod, a cheer. */
  const playCue = (cue: Cue) => {
    const handle = handles.current.get(cue.id)
    // A gesture the session holds is its own: nothing is played over it.
    if (!handle || castByIdRef.current.get(cue.id)?.record.held != null) return
    const played = cue.kind === 'cheer' || cue.kind === 'nod' ? handle.play(cue.kind, 'once') : handle.play(designedAs(cue.id, 'greet') ? 'greet' : 'wave', 'once')
    if (played) otherGesture.current(cue.id)
  }
  /** What the walkers keep out of: bubbles (an open one, the app's own line beside a figure) and the large figure. */
  const inTheWay = (now: number): Obstacle[] => {
    if (now - obstacles.current.at < OBSTACLES_EVERY_MS) return obstacles.current.boxes
    const boxes: Obstacle[] = []
    const add = (el: Element, of?: string) => {
      const r = el.getBoundingClientRect()
      if (r.width > 0 && r.height > 0) boxes.push({ left: r.left, top: r.top, right: r.right, bottom: r.bottom, of })
    }
    const active = sceneRef.current.active.id
    for (const [id, slot] of slots.current) {
      // Each belongs to its avatar, which is not in its own way.
      if (id === active) add(slot, id)
      for (const bubble of slot.querySelectorAll('.wb-av-bubble')) add(bubble, id)
    }
    // The chip for those that are not on stage stays in sight.
    if (groupEl) add(groupEl)
    obstacles.current = { at: now, boxes }
    return boxes
  }
  /** Whether a figure with its feet at this point would stand on nothing the user presses or types into. */
  const quietAt = (at: Point): boolean => {
    // Its middle and four points of its box: a figure beside a button covers it as well.
    for (const p of samplePoints(at, sizes.rest)) {
      const under = document.elementsFromPoint(p.x, p.y).find((el) => !el.closest('.wb-av-layer'))
      if (under?.closest(IN_USE)) return false
    }
    return true
  }
  /** One frame of the walk. */
  const walk = useLatest((now: number, dt: number) => {
    const area = feetArea()
    if (!roaming || !area) return
    const drag = roamDrag.current
    const at = pointer.current
    step(world.current, {
      now,
      dt,
      bounds: area,
      walkers: drag ? walkers.current.map((w) => (w.id === drag.id ? { ...w, held: { x: drag.x, y: drag.y } } : w)) : walkers.current,
      spawn: (_id, index) => spawnAt(index),
      obstacles: inTheWay(now),
      // A pointer that was parked somewhere stops nobody: only one that moved lately may be on its way to a figure.
      pointer: at && now - at.movedAt < POINTER_FRESH_MS ? { x: at.x, y: at.y } : null,
      size: sizes.rest,
      speaker: presentRef.current?.terminalId ?? null,
      finished: finished.current.splice(0),
      random: Math.random,
      quiet: quietAt,
    })
    showWalk()
    for (const cue of world.current.cues.splice(0)) playCue(cue)
    // One that reached the place its bubble has room at: the bubble is measured again where it stands now.
    let arrived = false
    for (const w of walkers.current) {
      const b = world.current.bodies.get(w.id)
      if (!w.post) posted.current.delete(w.id)
      else if (b && Math.hypot(b.x - w.post.x, b.y - w.post.y) < 1 && !posted.current.has(w.id)) {
        posted.current.add(w.id)
        arrived = true
      }
    }
    if (arrived) setTick((n) => n + 1)
  })
  // Before the page is painted: a figure that came on stage stands in its place at once, and every figure where the walk has it.
  useLayoutEffect(() => {
    const area = feetArea()
    if (!roaming || !area) return
    attend(world.current, { walkers: walkers.current, spawn: (_id, index) => spawnAt(index), bounds: area, now: clock(), random: Math.random })
    showWalk()
  })
  const walkingNow = roaming && attention.visible && visible.length > 0
  useEffect(() => {
    if (!walkingNow) return
    let last = clock()
    let frame = window.requestAnimationFrame(function loop() {
      // Asked for first: a frame that throws does not end the walk.
      frame = window.requestAnimationFrame(loop)
      const now = clock()
      const dt = now - last
      last = now
      walk.current(now, dt)
    })
    return () => window.cancelAnimationFrame(frame)
  }, [walkingNow, walk])
  // Walking ended (the switch, reduced motion, the compact layout): the next walk begins in the column again.
  useEffect(() => {
    if (roaming) return
    for (const id of ownWalk.current) handles.current.get(id)?.stop('walk')
    world.current = newWorld()
    for (const kept of [posts, written, roamLooks]) kept.current.clear()
    ownWalk.current.clear()
    posted.current.clear()
    roamDrag.current = null
  }, [roaming])

  /** A press on a walking figure: after a few pixels it is a drag, and the figure goes where the pointer takes it. */
  const onRoamDown = (id: string) => (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.pointerType === 'mouse' && e.button !== 0) return
    const target = e.target as Element | null
    const body = world.current.bodies.get(id)
    // The figure is the handle: a press on its bubble or on a control moves nothing.
    if (!body || !target?.closest?.('.wb-av-figure-btn')) return
    endPress.current()
    const slot = e.currentTarget
    const from = { pointer: e.pointerId, x: e.clientX, y: e.clientY, bx: body.x, by: body.y }
    let moved = false
    const move = (ev: PointerEvent) => {
      if (ev.pointerId !== from.pointer) return
      const dx = ev.clientX - from.x
      const dy = ev.clientY - from.y
      if (!moved) {
        if (!dragStarts(dx, dy)) return
        moved = true
        setDraggedId(id)
        try {
          slot.setPointerCapture(from.pointer)
        } catch {
          // A pointer that is gone already: the window's listeners still follow it.
        }
      }
      roamDrag.current = { id, x: from.bx + dx, y: from.by + dy }
    }
    const end = () => {
      window.removeEventListener('pointermove', move, true)
      window.removeEventListener('pointerup', end, true)
      window.removeEventListener('pointercancel', end, true)
      slot.removeEventListener('lostpointercapture', end)
      endPress.current = () => {}
      roamDrag.current = null
      if (!moved) return
      setDraggedId(null)
      justDragged.current = true
      window.setTimeout(() => (justDragged.current = false), 80)
      // Where it was dropped is where it stands now: a place kept from before the drag is forgotten.
      posts.current.delete(id)
      posted.current.delete(id)
      setTick((n) => n + 1)
    }
    endPress.current = end
    window.addEventListener('pointermove', move, true)
    window.addEventListener('pointerup', end, true)
    window.addEventListener('pointercancel', end, true)
    slot.addEventListener('lostpointercapture', end)
  }

  // ---- the compact panel sits under the face in the top bar

  useLayoutEffect(() => {
    const next = mode === 'bar' && faceEl ? Math.round(faceEl.getBoundingClientRect().right) : null
    if (next !== faceRight) setFaceRight(next)
  })

  // ---- the bubble stays inside the workspace window

  useLayoutEffect(() => {
    if (mode !== 'wide' || !bubbleEl || !activeId || !geometry.work) {
      if (shift !== 0) setShift(0)
      return
    }
    const slot = slots.current.get(activeId)
    if (!slot) return
    const next = bubbleShift(slot.getBoundingClientRect().top, bubbleEl.getBoundingClientRect().height, geometry.work)
    if (next !== shift) setShift(next)
  })

  // A walking figure grows upwards from its feet: its top, and so its bubble's place, is known once it has grown.
  const grows = roaming && activeId !== null && bubbleOpen
  useEffect(() => {
    if (!grows) return
    const timer = window.setTimeout(() => setTick((n) => n + 1), GROWN_AFTER_MS)
    return () => window.clearTimeout(timer)
  }, [grows, activeId])

  // ---- keys

  const onKeyDown = (e: ReactKeyboardEvent) => {
    if (e.key !== 'Escape' || e.nativeEvent.isComposing || sceneRef.current.bubble === 'closed') return
    e.stopPropagation()
    e.preventDefault()
    closeBubble()
  }

  // ---- stable readers for the engine (a new function would bind the figure again)

  const readers = useRef(new Map<string, { mouth: () => number; gaze: () => readonly [number, number] | null; handle: (h: FigureHandle | null) => void }>())
  const readersFor = (id: string) => {
    let r = readers.current.get(id)
    if (!r) {
      r = {
        mouth: () => rigRef.current?.speaker.mouth(id) ?? -1,
        gaze: () => {
          const at = pointer.current
          if (at && clock() - at.movedAt < GAZE_POINTER_MS) return gaze.current.get(id) ?? null
          // Its bubble while a question is open; else the avatar it meets or the one that speaks.
          return atBubble.current.get(id) ?? roamLooks.current.get(id) ?? null
        },
        handle: (h) => {
          if (h) handles.current.set(id, h)
          else handles.current.delete(id)
          // A new figure (a new design, a figure that came back) plays nothing yet: its walk begins again.
          ownWalk.current.delete(id)
        },
      }
      readers.current.set(id, r)
    }
    return r
  }

  // ---- rendering

  const projectName = (m: Member): string | null => {
    if (!m.foreign) return null
    const pid = m.record.projectId ?? m.view.projectId
    return projects?.find((p) => p.id === pid)?.name ?? null
  }
  const stateOf = (m: Member): AvatarState =>
    avatarState({
      blocked: m.view.blocked,
      presenting: present?.terminalId === m.id,
      listening: listeningId === m.id || (focus.inReply && focus.id === m.id && (drafts.get(m.id)?.text ?? '').trim() !== ''),
      askOpen: m.record.ask !== null,
      tone: m.view.tone,
    })
  /** Another project's avatar is here for its question: it wears the attention ring unless it speaks or listens. */
  const ringOf = (m: Member, state: AvatarState): Ring => (m.foreign && state !== 'speaking' && state !== 'listening' ? 'attention' : ringFor(state, m.view.tone))
  const isUnseen = (m: Member): boolean => unseen(m.record.lines, own(prefs.seen, m.id)).from !== null
  const openSession = (m: Member) => openPanel({ kind: 'terminal', id: `terminal:${m.id}`, title: m.view.title, params: { terminalId: m.id } })

  const menuFor = (m: Member, el: HTMLElement) =>
    showMenuAt(
      el,
      avatarMenu(
        {
          mode,
          presenting: present !== null,
          voice: prefs.voice,
          reduceMotion: prefs.reduceMotion,
          roam: prefs.roam,
          systemReduced,
          designed: m.record.designed === true,
          rejected: drawnLook(m.record.look, m.id).rejected,
          moved: prefs.pos !== null,
          others: cast.filter((o) => o.id !== m.id).map((o) => ({ id: o.id, label: calledAs(o.record.name, o.view.title) })),
        },
        {
          stop: () => void stopSpeaking.current(),
          voice: (on) => turnVoice.current(on, m.record.voice?.lang),
          hide: () => prefs.setPrefs({ show: false }),
          defaultLook: () =>
            void confirmDialog({ title: 'Use the default look?', message: `The look ${calledAs(m.record.name, m.view.title)} drew for itself is discarded.`, confirmLabel: 'Use the default look' }).then((ok) => {
              if (ok) avatarApi.reset(m.id).catch((e: unknown) => toast('error', errorText(e)))
            }),
          turnOff: () => void avatarApi.off(m.id).catch((e: unknown) => toast('error', errorText(e))),
          move: (which: Corner) => {
            if (geometry.work) prefs.setPrefs({ pos: corner(which, geometry.dock ?? geometry.work, geometry.work, geometry.viewport) })
          },
          resetPosition: () => prefs.setPrefs({ pos: null }),
          reduceMotion: (on) => prefs.setPrefs({ reduceMotion: on }),
          roam: (on) => prefs.setPrefs({ roam: on }),
          settings: () => openSettings('avatar'),
          show: (id) => void openFor(id),
        },
      ),
    )

  const figureOf = (m: Member, active: boolean): ReactNode => {
    const state = stateOf(m)
    const r = readersFor(m.id)
    const called = calledAs(m.record.name, m.view.title)
    const aloud = present?.terminalId === m.id && present.engine === 'browser'
    return (
      <FigureButton
        look={m.record.look}
        seed={m.id}
        revision={m.record.revision}
        idPrefix={figurePrefix(m.id)}
        size={active ? sizes.active : sizes.rest}
        engine={rig?.engine ?? null}
        state={state}
        reducedMotion={reducedMotion}
        mouth={r.mouth}
        gaze={r.gaze}
        held={m.record.held}
        onHandle={r.handle}
        ring={ringOf(m, state)}
        label={m.record.name || m.view.title}
        title={m.view.title}
        stateLabel={stateWords(state, m.view.label)}
        active={active}
        unseen={isUnseen(m)}
        voiceOff={prefs.voice === null}
        designing={isDesigning({ withOption: m.view.withOption, designed: m.record.designed === true, working: m.view.working, startedAt: m.view.createdAt, now: Date.now() })}
        controls={
          <>
            <IconButton icon={Reply} size="small" label={`Reply to ${called}`} onPointerDown={keepFocus} onMouseDown={keepFocus} onClick={() => void openFor(m.id)} />
            <IconButton icon={SquareArrowOutUpRight} size="small" label="Open the session" onPointerDown={keepFocus} onMouseDown={keepFocus} onClick={() => openSession(m)} />
            <IconButton icon={MoreHorizontal} size="small" label="Avatar menu" aria-haspopup="menu" onPointerDown={keepFocus} onMouseDown={keepFocus} onClick={(e) => menuFor(m, e.currentTarget)} />
          </>
        }
        onPress={() => {
          if (justDragged.current) return
          // While its line is said aloud, a click on the figure stops the line.
          if (aloud) void stopSpeaking.current()
          else press(m.id)
        }}
      />
    )
  }

  const bubbleOf = (m: Member, tail: 'left' | 'right' | null, panel: boolean, figure?: ReactNode): ReactNode => {
    const ask = m.record.ask
    const byUser = scene.bubble === 'user'
    const presenting = present !== null && present.terminalId === m.id
    const shownLine = scene.lineId ? m.record.lines.find((l) => l.id === scene.lineId) : undefined
    const parts = presentedParts.current.get(m.id)
    /** The line on screen was presented in these parts: once it is over they stay, all of them said. */
    const said = shownLine && parts?.lineId === shownLine.id && parts.chunks.length > 0 ? parts.chunks : null
    let caption: BubbleCaption | null = null
    if (presenting && present.lineId === scene.lineId) caption = { lineId: present.lineId || undefined, text: present.text, chunks: present.chunks, index: present.index, lang: present.lang }
    else if (shownLine && (!byUser || said)) caption = { lineId: shownLine.id, text: shownLine.text, lang: shownLine.lang, ...(said ? { chunks: said, index: said.length } : {}) }
    const log = byUser ? transcript(m.record.lines, undefined).lines : null
    const asked = ask ? m.record.lines.find((l) => l.id === ask.lineId) : undefined
    /** The bubble shows something the avatar said: the first voice choice and "Click to hear" belong under it. */
    const spoke = caption !== null || ask !== null || (log !== null && log.some((l) => l.from === 'agent'))
    /** The line "Hear it" and "Click to hear" say: the one on screen, else the question, else the newest. */
    const toSay = (shownLine?.from === 'agent' ? shownLine : undefined) ?? asked ?? newestAgentLine(m.record.lines)
    const draft = drafts.get(m.id) ?? NO_DRAFT
    // The reply box: with a question that takes typed answers, and in the transcript the user opened.
    const replies = replyShown(scene, m.id, ask)
    // The note as it is now: one that has just gone is not shown for the moment before it is dropped.
    const kept = noteStep(notes.get(m.id), clock(), lastLines.current.get(m.id))?.note ?? null
    const note: BubbleNote | null = kept && (kept.kind !== 'changed' || replies) ? kept : null
    return (
      <BubbleHost
        key={m.id}
        provider={m.view.provider}
        title={m.view.title}
        project={projectName(m)}
        name={m.record.name}
        caption={caption}
        aloud={presenting && present.engine === 'browser'}
        log={log}
        unseenFrom={logFrom?.id === m.id ? logFrom.from : null}
        ask={ask}
        disabled={whyDisabled({ composeBlocked: m.view.refuses !== null, starting: m.view.refuses === 'starting', waiting: ask?.waiting === true })}
        draft={replies ? draft : null}
        sending={sending.has(m.id)}
        note={note}
        voiceChoice={voiceForm({ voice: prefs.voice, online: prefs.online, can: voicesFor(voices, toSay?.lang ?? m.record.voice?.lang ?? 'en-US'), listed: voicesListed, asked: voiceForms.get(m.id), spoke })}
        locked={prefs.voice === true && !unlocked && spoke}
        tail={tail}
        panel={panel}
        clickThrough={clicksThrough(scene)}
        holding={holding(scene)}
        figure={figure}
        lang={asked?.lang ?? m.record.voice?.lang}
        fieldRef={fieldRef}
        rootRef={setBubbleEl}
        onChoice={(index) => choose(m, ask, index)}
        onDraft={(text, dictated, begunFor) => {
          draftEdited(m.id)
          const was = drafts.get(m.id)
          const next = edited(was, text, ask?.id ?? null, dictated, begunFor)
          setDraft(m.id, next)
          // Dictated words that begin a draft for a question no longer on screen: the bubble says so, as when it changes under a draft.
          if (!was?.text && askChanged(next, ask?.id ?? null)) setNote(m.id, { kind: 'changed' }, ask?.lineId ?? null)
        }}
        onSend={() => send(m)}
        onDismiss={ask ? () => dismiss(m, ask) : undefined}
        onStop={() => void stopSpeaking.current()}
        onHear={() => hear(m, toSay)}
        onCaptionsOnly={() => prefs.setPrefs({ voice: false })}
        onAllowOnline={() => allowOnline(m, toSay)}
        onUnlock={() => unlock(m, toSay)}
        onClose={closeBubble}
        onProblem={(text, level) => setNote(m.id, { kind: level === 'error' ? 'error' : 'info', text })}
        onListening={(on) => setListeningId((prev) => (on ? m.id : prev === m.id ? null : prev))}
      />
    )
  }

  const slotRef = (id: string) => (el: HTMLDivElement | null) => {
    if (el) slots.current.set(id, el)
    else slots.current.delete(id)
  }
  const hoverTimer = useRef<number | null>(null)
  const hoverFor = useRef<string | null>(null)
  const hoveredNow = useLatest(hovered)
  const enter = (id: string) => (e: ReactPointerEvent) => {
    if (e.pointerType !== 'mouse') return
    // A figure that walks under a pointer that rests is not pointed at: the pointer has to come to it.
    const at = pointer.current
    if (roaming && (!at || clock() - at.movedAt > HOVER_FRESH_MS)) return
    // Already pointed at, or on its way to be (a move on the figure does not start the wait again).
    if (hoveredNow.current === id || (hoverTimer.current !== null && hoverFor.current === id)) return
    if (hoverTimer.current !== null) window.clearTimeout(hoverTimer.current)
    hoverFor.current = id
    // Passing over a figure does nothing: the pointer has to rest on it.
    hoverTimer.current = window.setTimeout(() => {
      hoverTimer.current = null
      setHovered(id)
    }, HOVER_DWELL_MS)
  }
  const leave = (id: string) => () => {
    if (hoverTimer.current !== null) window.clearTimeout(hoverTimer.current)
    hoverTimer.current = null
    hoverFor.current = null
    setHovered((prev) => (prev === id ? null : prev))
  }
  useEffect(
    () => () => {
      if (hoverTimer.current !== null) window.clearTimeout(hoverTimer.current)
    },
    [],
  )

  const activeMember = activeId ? castById.get(activeId) : undefined
  /** The app's own line beside a figure whose session needs the user, unless the user opened its transcript. */
  const showsBlocked = (m: Member): boolean => m.view.blocked && !(m.id === activeId && scene.bubble === 'user') && !(bubbleOpen && m.id !== activeId)

  // What the walk is told of each figure at its next step, and where the ones that are wanted stand.
  if (roaming && walkArea) {
    // What stands under the large figure stays inside the area as well.
    const work = { ...walkArea, bottom: Math.max(walkArea.top, walkArea.bottom - UNDER_PX) }
    const onStage = new Set(visible)
    // What is kept per figure is kept for the figures on stage only: one that comes back starts afresh.
    for (const kept of [posts.current, written.current, lastStates.current, gestured.current]) for (const id of kept.keys()) if (!onStage.has(id)) kept.delete(id)
    for (const kept of [posted.current, ownWalk.current]) for (const id of kept) if (!onStage.has(id)) kept.delete(id)
    // The area changed (a tool window opened, the window was resized): where the bubbles have room is found again.
    const area = `${walkArea.left} ${walkArea.top} ${walkArea.right} ${walkArea.bottom}`
    if (postsArea.current !== area) {
      postsArea.current = area
      posts.current.clear()
      posted.current.clear()
    }
    // Who needs room beside its figure: an open bubble, or the app's own line. One that is only looked at (the
    // pointer, the keyboard) needs none and stays where it is. Those that need none any more give theirs up first.
    const needs = new Map<string, number>()
    for (const id of visible) {
      const m = castById.get(id)
      if (!m) continue
      if (id === activeId && bubbleOpen) needs.set(id, BUBBLE_ROOM_PX)
      else if (showsBlocked(m)) needs.set(id, LINE_ROOM_PX)
    }
    for (const id of posts.current.keys()) if (!needs.has(id)) posts.current.delete(id)
    const told: Walker[] = []
    for (const id of visible) {
      const m = castById.get(id)
      if (!m) continue
      const state = stateOf(m)
      const was = lastStates.current.get(id)
      // Its session finished: those near it cheer. Only where somebody walks to see it: a hidden tab does not save its cheers up.
      if (was === 'working' && (state === 'done' || state === 'idle') && walkingNow) finished.current.push(id)
      lastStates.current.set(id, state)
      const room = needs.get(id)
      let post = room === undefined ? null : (posts.current.get(id) ?? null)
      if (room !== undefined && !post) {
        // Where its bubble fits, and where its figure and bubble cover nobody else's (`placePost`).
        const b = world.current.bodies.get(id)
        const wish = postFor(b ? { x: b.x, y: b.y } : spawnAt(told.length), work, sizes.active, BUBBLE_REACH_PX, EDGE_PX)
        const taken = [...posts.current.entries()].filter(([other]) => other !== id).map(([other, p]) => claimOf(p, sizes.active, BUBBLE_REACH_PX, needs.get(other) ?? LINE_ROOM_PX))
        post = placePost(wish, taken, work, sizes.active, BUBBLE_REACH_PX, EDGE_PX, room)
        posts.current.set(id, post)
      }
      const wanted = room !== undefined || m.foreign || focus.id === id || m.record.ask !== null || m.record.held != null || state === 'speaking' || state === 'listening' || state === 'attention' || state === 'error'
      // The pointer rests on it, or it holds the stage for no other reason: it waits where it is in nobody's way (the walk decides).
      told.push({ id, stays: wanted, post: post?.at ?? null, held: null, busy: state === 'working', asleep: state === 'sleeping', pointed: hovered === id || id === activeId })
    }
    walkers.current = told
  }
  /** On which side of its bubble an avatar's tail is: where it stands while it walks about, else the column's side. */
  const tailOf = (id: string): 'left' | 'right' => (roaming ? posts.current.get(id)?.tail : undefined) ?? placement?.tail ?? 'right'

  // While a question is open the active figure looks at its bubble.
  atBubble.current.clear()
  if (activeMember && bubbleOpen && activeMember.record.ask && placement) atBubble.current.set(activeMember.id, [tailOf(activeMember.id) === 'right' ? -GAZE_AT_BUBBLE : GAZE_AT_BUBBLE, 0.1])

  const layerClass = prefs.reduceMotion ? 'wb-av-layer still' : 'wb-av-layer'

  if (mode === 'wide') {
    if (!placement || cast.length === 0) return null
    const hidden = more.map((id) => castById.get(id)).filter((m): m is Member => !!m)
    const slotOf = (id: string) => {
      const m = castById.get(id)
      if (!m) return null
      const active = id === activeId
      const withBubble = active && bubbleOpen
      const blockedForm = showsBlocked(m)
      const tail = tailOf(id)
      const style = withBubble && shift !== 0 ? ({ ['--wb-av-bubble-top' as string]: `${shift}px` } as CSSProperties) : undefined
      // A walking figure grows when it has something to show (its bubble is open), not under the pointer alone: it
      // would grow away from the pointer that is about to press it.
      const large = active && (!roaming || bubbleOpen)
      // It is placed by the walk, frame by frame (`showWalk`), and is its own handle for a drag.
      const walking = roaming
        ? { className: ['wb-av-slot roam', large && 'active', draggedId === id && 'dragging'].filter(Boolean).join(' '), onPointerDown: onRoamDown(id), onClickCapture: onGroupClick, onPointerMove: enter(id) }
        : { className: 'wb-av-slot' }
      return (
        <div key={id} ref={slotRef(id)} {...walking} style={style} onPointerEnter={enter(id)} onPointerLeave={leave(id)}>
          {figureOf(m, large)}
          {withBubble && !blockedForm ? bubbleOf(m, tail, false) : null}
          {blockedForm ? <BlockedForm provider={m.view.provider} title={m.view.title} project={projectName(m)} name={m.record.name} tail={tail} pointer={pointer} onOpen={() => openSession(m)} /> : null}
        </div>
      )
    }
    const moreFaces = hidden.length > 0 && (
      <MoreFaces
        count={hidden.length}
        size={FIGURE_PX.rest + FIGURE_RING_PX}
        ring={mostUrgent(hidden.map((m) => ringOf(m, stateOf(m))))}
        unseen={hidden.some(isUnseen)}
        label={`${hidden.length} more ${hidden.length === 1 ? 'avatar' : 'avatars'}`}
        buttonRef={moreRef}
        onPress={() => {
          if (moreRef.current) showMenuAt(moreRef.current, hidden.map((m) => ({ label: calledAs(m.record.name, m.view.title), run: () => void openFor(m.id) })))
        }}
      />
    )
    if (roaming) {
      // No column: each figure is on the layer by itself. Only the chip for those behind it keeps the column's place.
      return (
        <div className={layerClass} onKeyDown={onKeyDown} onFocus={onFocus} onBlur={onBlur}>
          {visible.map(slotOf)}
          {moreFaces ? (
            <div ref={setGroupEl} className={placement.side === 'left' ? 'wb-av-group left' : 'wb-av-group'} style={placement.css}>
              {moreFaces}
            </div>
          ) : null}
        </div>
      )
    }
    const groupClass = ['wb-av-group', 'draggable', placement.side === 'left' && 'left', drag && 'dragging'].filter(Boolean).join(' ')
    return (
      <div className={layerClass} onKeyDown={onKeyDown} onFocus={onFocus} onBlur={onBlur}>
        <div ref={setGroupEl} className={groupClass} style={placement.css} onPointerDown={onGroupDown} onClickCapture={onGroupClick}>
          {visible.map(slotOf)}
          {moreFaces}
        </div>
      </div>
    )
  }

  // ---- compact: a face in the top bar, the bubble as a panel under it

  const frontMember = frontId ? castById.get(frontId) : undefined
  if (!frontMember) return null
  const frontState = stateOf(frontMember)
  /** While the panel is kept out of the way of a permission request: how many of them wait. */
  const waiting = held ? [...views.values()].filter((v) => v.pending && (v.projectId ?? null) === (projectId ?? null)).length : 0
  const behind = cast.length - 1
  const panelOpen = bubbleOpen && activeId === frontMember.id
  const face = (
    <Face
      look={frontMember.record.look}
      seed={frontMember.id}
      idPrefix={facePrefix(frontMember.id)}
      size={mode === 'phone' ? FACE_PX.phone : FACE_PX.bar}
      ring={ringOf(frontMember, frontState)}
      unseen={cast.some(isUnseen)}
      count={held ? waiting : behind}
      countTone={held ? 'attention' : 'plain'}
      label={`${calledAs(frontMember.record.name, frontMember.view.title)}, ${stateWords(frontState, frontMember.view.label)}`}
      open={panelOpen}
      onPress={() => press(frontMember.id)}
      buttonRef={setFaceEl}
    />
  )
  const panelAt = panelPlace(mode, panelTop(mode === 'phone' ? geometry.header : geometry.bar, mode === 'phone' ? 44 : 36), faceRight, geometry.viewport)
  return (
    <div className={layerClass} onKeyDown={onKeyDown} onFocus={onFocus} onBlur={onBlur}>
      {mode === 'phone' ? (
        <div className="wb-av-group" style={phoneFace(geometry.header, FACE_PX.phone, 44)}>
          {face}
        </div>
      ) : barSlot ? (
        createPortal(prefs.reduceMotion ? <span className="wb-av-still">{face}</span> : face, barSlot)
      ) : null}
      {panelOpen && (
        <div className="wb-av-panel-place" style={panelAt}>
          {bubbleOf(frontMember, null, true, <div ref={slotRef(frontMember.id)}>{figureOf(frontMember, true)}</div>)}
        </div>
      )}
    </div>
  )
}
