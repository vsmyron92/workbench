// The figure: an avatar's artwork on the app's own stage, bound to the window's engine.
//
// The artwork is a session's own drawing, so it stays inside what the app owns (rule 11):
// a fixed-size, clipped box of which nothing is seen (no backdrop, no border: only the
// figure), an `<svg>` that is hidden from assistive technology and takes no pointer.
// Everything around it (the glow of its state, the name under the large figure, the unseen
// dot, the "Voice off" chip) is the app's.
//
// How it is bound (10.7): the SVG tree is built once per look object and prefix and
// remounted for a new look; state, mouth, gaze and gestures reach the engine through its
// handle in effects, never through props of the SVG tree; the engine attaches in a layout
// effect and detaches in its cleanup; its targets are the nodes inside this stage, found
// through a ref, never by a document-wide id.
//
// Pure views: what is drawn and shown comes in as props, what the user does goes out
// through `onPress`. Nothing here fetches, stores or decides when.

import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, type CSSProperties, type ReactNode, type Ref } from 'react'
import { VolumeX } from 'lucide-react'
import { ErrorBoundary } from '@/ui'
import { defaultLook } from './lib/defaultLook'
import { compile } from './lib/engine'
import type { Engine, FigureHandle } from './lib/engine'
import { checkLook, cleanIdPrefix, faceViewBox, lookElements, viewBoxOf } from './lib/look'
import type { AvatarState, Look, SessionTone } from './lib/types'
import './avatar.css'

// ---------------------------------------------------------------- sizes and words

/** The stage in CSS px: at rest, and while its avatar is the active one (10.1). Never larger (rule 12). */
export const FIGURE_PX = { rest: 56, active: 132 } as const
/** The same on the phone. */
export const FIGURE_PX_PHONE = { rest: 48, active: 96 } as const
/**
 * What the "+n" chip is larger than a resting figure's stage, in CSS px: nothing. No ring is drawn
 * round a figure (its state is a glow on its own outline), so the chip is as large as a resting figure.
 */
export const FIGURE_RING_PX = 0
/** The small face: in the agents window's top bar, and in the phone's header. Its stage fills that size. */
export const FACE_PX = { bar: 28, phone: 40 } as const
const STAGE_MAX_PX = 132
const STAGE_MIN_PX = 12

/** Under a figure whose session has not drawn its look yet (4.7). */
export const DESIGNING = 'Designing its look…'
/** For a design the checker or the engine rejects: the default look is drawn instead (10.7). */
export const DESIGN_NOT_SHOWN = 'This design could not be shown here.'
/** The chip of a browser in which voice has not been chosen yet (10.2). */
export const VOICE_OFF = 'Voice off'

// ---------------------------------------------------------------- what is drawn

export interface DrawnLook {
  /** What is drawn: the session's own design, or the default look. */
  look: Look
  /** The session's own design is drawn. */
  own: boolean
  /** The session has a design, but it cannot be shown in this browser: the default look is drawn. */
  rejected: boolean
}

/** Design as the server sent it -> the checked look, or null when it was rejected. One check, one warning per object. */
const CHECKED = new WeakMap<object, Look | null>()
let warnedOdd = false

function reject(): null {
  console.warn('[workbench] an avatar’s design could not be shown here: the default look is drawn instead')
  return null
}

/**
 * What a figure draws for a record's `look`: the design when `checkLook` and `compile`
 * accept it, else the default look of the session (`seed`: the terminal id). The checked
 * look is the same object for the same design object, so a figure is not rebuilt when the
 * record changes for another reason.
 */
export function drawnLook(raw: unknown, seed: string): DrawnLook {
  if (raw === null || raw === undefined) return { look: defaultLook(seed), own: false, rejected: false }
  let look: Look | null = null
  if (typeof raw === 'object') {
    const known = CHECKED.get(raw)
    if (known !== undefined) {
      look = known
    } else {
      const checked = checkLook(raw)
      look = checked && compile(checked) ? checked : reject()
      CHECKED.set(raw, look)
    }
  } else if (!warnedOdd) {
    warnedOdd = true
    reject()
  }
  return look ? { look, own: true, rejected: false } : { look: defaultLook(seed), own: false, rejected: true }
}

/** A key per look object: a new look remounts the SVG tree instead of patching the old one. */
const KEYS = new WeakMap<Look, number>()
let nextKey = 1
function keyOf(look: Look): number {
  let key = KEYS.get(look)
  if (key === undefined) {
    key = nextKey++
    KEYS.set(look, key)
  }
  return key
}

// ---------------------------------------------------------------- ring and state words

/** What the ring shows: the session's tone, or what its avatar does right now. */
export type Ring = SessionTone | 'speaking' | 'listening'

/**
 * The ring of a figure. It is the session's own tone, so the avatar agrees with the tab,
 * the card and the pill; only while it speaks or listens (and the session neither needs
 * the user nor failed) does the ring say that instead.
 */
export function ringFor(state: AvatarState, tone: SessionTone): Ring {
  return state === 'speaking' || state === 'listening' ? state : tone
}

const RINGS: ReadonlySet<string> = new Set(['working', 'attention', 'error', 'unread', 'idle', 'starting', 'exited', 'speaking', 'listening'])

/** The ring as a class of the style sheet: one of its own words, whatever it is given. */
export function ringClass(ring: Ring): Ring {
  return RINGS.has(ring) ? ring : 'idle'
}

/** The state in words, for the tooltip and the accessible name: the tab's own, or Speaking, Listening, Asks you. */
export function stateWords(state: AvatarState, sessionLabel: string): string {
  switch (state) {
    case 'speaking':
      return 'Speaking'
    case 'listening':
      return 'Listening'
    case 'waiting':
      return 'Asks you'
    default:
      return sessionLabel
  }
}

/**
 * For `onPointerDown` and `onMouseDown` of every control that is no text field: the press
 * is handled, and focus stays where it was (in the terminal, usually). The click still comes.
 */
export function keepFocus(e: { preventDefault(): void }) {
  e.preventDefault()
}

// ---------------------------------------------------------------- the stage

/**
 * The stage's size as the style sheet reads it (`--wb-av-size`), within what a stage may be.
 * Set on the stage and on what draws the ring around it.
 */
export function sizeStyle(size: number): CSSProperties {
  const px = Math.min(Math.max(Number.isFinite(size) ? Math.round(size) : FIGURE_PX.rest, STAGE_MIN_PX), STAGE_MAX_PX)
  return { ['--wb-av-size' as string]: `${px}px` } as CSSProperties
}

interface ArtworkProps {
  look: Look
  idPrefix: string
  /** Only the head (`faceViewBox`), for the small face. */
  face: boolean
  onSvg?: (svg: SVGSVGElement | null) => void
}

/**
 * The look as SVG. Memoised: it renders again only for another look, prefix or crop. The
 * elements come from `lookElements` (tags and attributes from its tables, text as text
 * nodes); nothing else is put on the tree.
 */
const Artwork = memo(function Artwork({ look, idPrefix, face, onSvg }: ArtworkProps) {
  const elements = useMemo(() => lookElements(look, idPrefix), [look, idPrefix])
  const viewBox = useMemo(() => (face ? faceViewBox(look) : viewBoxOf(look)).join(' '), [look, face])
  return (
    <svg key={keyOf(look)} ref={onSvg} className="wb-av-art" viewBox={viewBox} aria-hidden focusable="false">
      {elements}
    </svg>
  )
})

/** The app's own box around the artwork: fixed size, clipped, and itself not seen. */
function Stage({ size, drawn, children }: { size: number; drawn: 'own' | 'default' | 'fallback'; children: ReactNode }) {
  return (
    <span className="wb-av-stage" data-drawn={drawn} style={sizeStyle(size)}>
      {children}
    </span>
  )
}

/**
 * A look on the stage in its rest pose, without an engine: the small face, and what a
 * figure falls back to. `look` is a checked look (`drawnLook`), never a record's raw one.
 */
export const StillLook = memo(function StillLook({ look, idPrefix, size, face = false, drawn = 'fallback' }: { look: Look; idPrefix: string; size: number; face?: boolean; drawn?: 'own' | 'default' | 'fallback' }) {
  return (
    <Stage size={size} drawn={drawn}>
      <Artwork look={look} idPrefix={idPrefix} face={face} />
    </Stage>
  )
})

export interface FigureProps {
  /** The record's `look` as the server sent it (it is checked here), or null for the default look. */
  look: unknown
  /** The terminal id: the hue of the default look, and half of the error boundary's key. */
  seed: string
  /** The record's `revision`: a new design starts the figure over, error boundary included. */
  revision: number
  /** What the artwork's ids are printed with. Unique per figure on the page, and not the prefix of the same avatar's face. */
  idPrefix: string
  /** The stage in CSS px (`FIGURE_PX`), at most 132. */
  size: number
  /** The window's engine; null: the figure is drawn at rest. */
  engine: Engine | null
  state: AvatarState
  /** The user's switch or the system setting. */
  reducedMotion: boolean
  /** How open the mouth is now (`speaker.mouth(terminalId)`: negative while no line is presented). */
  mouth: (() => number) | null
  /** Where the pupils look, as a vector of length up to 1; a reader that answers null lets the figure glance about. */
  gaze: (() => readonly [number, number] | null) | null
  /**
   * The record's `held` gesture: played when the figure attaches and whenever it changes
   * (`since` tells a second play of the same one), ended when it becomes null. Gestures
   * that play once go through `onHandle`; a held one is not played by hand as well.
   */
  held?: { name: string; mode: 'hold' | 'loop'; since?: number } | null
  /** The engine's handle of this figure while it is attached (for gestures that play once), then null. */
  onHandle?: (handle: FigureHandle | null) => void
}

type BoundProps = Omit<FigureProps, 'look' | 'seed' | 'revision'> & { look: Look; own: boolean }

/** The stage with its artwork, attached to the engine. */
function Bound({ look, own, idPrefix, size, engine, state, reducedMotion, mouth, gaze, held, onHandle }: BoundProps) {
  /** The nodes of this stage that carry an id, by that id (prefix included). Filled when the SVG mounts. */
  const nodes = useRef(new Map<string, Element>())
  const collect = useCallback((svg: SVGSVGElement | null) => {
    const map = nodes.current
    map.clear()
    if (!svg) return
    for (const el of svg.querySelectorAll('[id]')) map.set(el.id, el)
  }, [])

  const handle = useRef<FigureHandle | null>(null)
  // What the figure is told now, for the moment it attaches (the effects below pass on what changes later).
  const latest = useRef({ state, reducedMotion, mouth, gaze, onHandle })
  latest.current = { state, reducedMotion, mouth, gaze, onHandle }

  useLayoutEffect(() => {
    if (!engine) return
    const now = latest.current
    const prefix = cleanIdPrefix(idPrefix)
    const h = engine.attach(look, (id) => nodes.current.get(prefix + id) ?? null, { state: now.state, reducedMotion: now.reducedMotion, idPrefix })
    h.setMouth(now.mouth)
    h.setGaze(now.gaze)
    handle.current = h
    now.onHandle?.(h)
    return () => {
      handle.current = null
      latest.current.onHandle?.(null)
      // Puts every part back to the artwork's own values.
      h.detach()
    }
  }, [engine, look, idPrefix])

  useEffect(() => handle.current?.setState(state), [state])
  useEffect(() => handle.current?.setReducedMotion(reducedMotion), [reducedMotion])
  useEffect(() => handle.current?.setMouth(mouth), [mouth])
  useEffect(() => handle.current?.setGaze(gaze), [gaze])

  // The held gesture is a state of the record: it is played for every handle, and ended when the record lets go of it.
  const heldName = held?.name ?? null
  const heldMode = held?.mode === 'loop' ? 'loop' : 'hold'
  const heldSince = held?.since ?? 0
  const holding = useRef<string | null>(null)
  useEffect(() => {
    const h = handle.current
    if (!h) {
      holding.current = null
      return
    }
    if (heldName !== null) h.play(heldName, heldMode)
    else if (holding.current !== null) h.stop(holding.current)
    holding.current = heldName
  }, [engine, look, idPrefix, heldName, heldMode, heldSince])

  return (
    <Stage size={size} drawn={own ? 'own' : 'default'}>
      <Artwork look={look} idPrefix={idPrefix} face={false} onSvg={collect} />
    </Stage>
  )
}

/**
 * One avatar's figure: the look (or the default look, when there is none or it is
 * rejected) on the stage, animated by the engine. An error while drawing or binding it
 * stays inside: the default look is shown, still, in its place.
 */
export const Figure = memo(function Figure({ look, seed, revision, ...rest }: FigureProps) {
  const drawn = useMemo(() => drawnLook(look, seed), [look, seed])
  return (
    <ErrorBoundary key={`${seed}:${revision}`} label="avatar figure" fallback={() => <StillLook look={defaultLook(seed)} idPrefix={rest.idPrefix} size={rest.size} />}>
      <Bound {...rest} look={drawn.look} own={drawn.own} />
    </ErrorBoundary>
  )
})

// ---------------------------------------------------------------- the figure as the user meets it

export interface FigureButtonProps extends FigureProps {
  /** `ringFor(state, tone)`. */
  ring: Ring
  /** Under the figure: the agent's name, else the session's title. */
  label: string
  /** The session's title, for the tooltip. */
  title: string
  /** `stateWords(state, stateLabel(t))`. */
  stateLabel: string
  /** The active avatar: the overlay passes the large size with it. */
  active?: boolean
  /** Its newest agent line was not seen: an accent dot. */
  unseen?: boolean
  /** Voice was not chosen in this browser yet: the "Voice off" chip. */
  voiceOff?: boolean
  /** The session was started with the option and is still drawing: the label reads "Designing its look…". */
  designing?: boolean
  /** The controls that show on hover or focus (reply, open session, menu): `IconButton`s. */
  controls?: ReactNode
  /** A click, or Enter or Space on the figure. */
  onPress?: () => void
  buttonRef?: Ref<HTMLButtonElement>
}

/** A held Enter presses the figure once: its repeats would open and close the bubble in turn. */
function onceForHeldEnter(e: { key: string; repeat: boolean; preventDefault(): void }) {
  if (e.key === 'Enter' && e.repeat) e.preventDefault()
}

/**
 * The figure with what the app shows around it: the glow of its state, the unseen dot, and,
 * with the large figure, the label under it and the "Voice off" chip; the controls on hover.
 * The figure itself is a button, named with the label and the state.
 */
export function FigureButton({ ring, label, title, stateLabel, active, unseen, voiceOff, designing, controls, onPress, buttonRef, ...figure }: FigureButtonProps) {
  const drawn = useMemo(() => drawnLook(figure.look, figure.seed), [figure.look, figure.seed])
  // "Designing its look…" only while the default look stands in for a design that is still to come.
  const drawing = designing === true && !drawn.own && !drawn.rejected
  return (
    <div className={['wb-av-figure', ringClass(ring), active && 'active', unseen && 'unseen'].filter(Boolean).join(' ')} style={sizeStyle(figure.size)}>
      <button ref={buttonRef} type="button" className="wb-av-figure-btn" aria-label={`${label}, ${stateLabel}`} title={`${title} · ${stateLabel}`} onPointerDown={keepFocus} onMouseDown={keepFocus} onKeyDown={onceForHeldEnter} onClick={onPress}>
        <span className="wb-av-ring">
          <Figure {...figure} />
        </span>
        {unseen && <span className="wb-av-dot" />}
      </button>
      <span className="wb-av-under">
        {drawing ? (
          <span className="wb-av-label designing">{DESIGNING}</span>
        ) : (
          <span className="wb-av-label" dir="auto">
            {label}
          </span>
        )}
        {voiceOff && (
          <span className="wb-av-chip">
            <VolumeX size={11} aria-hidden />
            {VOICE_OFF}
          </span>
        )}
        {drawn.rejected && <span className="wb-av-note">{DESIGN_NOT_SHOWN}</span>}
      </span>
      {controls ? <div className="wb-av-controls">{controls}</div> : null}
    </div>
  )
}
