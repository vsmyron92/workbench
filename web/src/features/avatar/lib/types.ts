// The avatar slice's wire types (what the server stores and sends) and the few pure helpers
// every other module of `lib/` shares. No React, no DOM, no globals.
//
// A look is data written by an agent: it is never trusted. `checkLook` (look.ts) rebuilds
// it from allow-listed keys before anything else reads it.

/** The parts of the artwork Workbench's built-in motion knows how to drive. */
export type Role = 'body' | 'head' | 'eyes' | 'pupils' | 'armL' | 'armR'

export const ROLES: readonly Role[] = ['body', 'head', 'eyes', 'pupils', 'armL', 'armR']

/** What the figure shows. Chosen by Workbench from the session's state, never by the agent. */
export type AvatarState = 'idle' | 'working' | 'speaking' | 'listening' | 'waiting' | 'attention' | 'error' | 'done' | 'sleeping'

export const AVATAR_STATES: readonly AvatarState[] = ['idle', 'working', 'speaking', 'listening', 'waiting', 'attention', 'error', 'done', 'sleeping']

/** What became of an agent line, best first: `spoken > skipped > failed > shown > hidden`; `unheard` is the server's own. */
export type Outcome = 'spoken' | 'skipped' | 'failed' | 'shown' | 'hidden' | 'unheard'

/** The agents feature's `Tone` (features/agents/lib/sessions.ts), repeated here so `lib/` imports no feature. */
export type SessionTone = 'working' | 'attention' | 'error' | 'unread' | 'idle' | 'starting' | 'exited'

export type Tag =
  | 'g'
  | 'path'
  | 'circle'
  | 'ellipse'
  | 'rect'
  | 'line'
  | 'polyline'
  | 'polygon'
  | 'text'
  | 'defs'
  | 'linearGradient'
  | 'radialGradient'
  | 'stop'
  | 'clipPath'
  | 'use'
  | 'mask'
  | 'filter'
  | 'feGaussianBlur'
  | 'feDropShadow'
  | 'feOffset'
  | 'feFlood'
  | 'feColorMatrix'
  | 'feComposite'
  | 'feBlend'
  | 'feMerge'
  | 'feMergeNode'
  | 'feMorphology'
  | 'feTurbulence'
  | 'feDisplacementMap'

/** `[x, y, width, height]`. */
export type Box = [number, number, number, number]

/**
 * The validated design: artwork tree with boxes, pivots, roles, animations, state map, mouth
 * and gaze, and the session's own rig: `parents` and `drives`, present only when they hold something.
 */
export interface Look {
  viewBox: Box
  nodes: LookNode[]
  /** Part id -> the point it turns and scales about, in the coordinates its own geometry is written in. */
  pivots: Record<string, [number, number]>
  roles: Partial<Record<Role, string>>
  animations: Record<string, Animation>
  states: Partial<Record<AvatarState, string[]>>
  /** 0 to 4 drives that share one openness. */
  mouth: MouthDrive[]
  /** Pupil travel in viewBox units; null: 3% of the viewBox width. */
  gaze: number | null
  /** Part id -> the part it moves with, as if it were nested in it, while it keeps its place in the drawing order. */
  parents?: Record<string, string>
  /** Up to 16: the live inputs (where it looks, its mouth, its walk) moving its own parts. */
  drives?: Drive[]
}

export interface LookNode {
  tag: Tag
  id?: string
  /** Keys from the closed table only (React SVG prop names). */
  attrs?: Record<string, string | number>
  /**
   * Every node with an id: its rest-pose box in the coordinates its own geometry is written
   * in (its own transform excluded).
   */
  box?: Box
  children?: LookNode[]
  text?: string
}

export type EaseName = 'linear' | 'in' | 'out' | 'inOut' | 'outBack' | 'step' | 'inBack' | 'inOutBack' | 'outElastic' | 'outBounce'

/** A cubic bezier `[x1, y1, x2, y2]`: x from 0 to 1, y from -2 to 3. */
export type Bezier = [number, number, number, number]

export type Ease = EaseName | Bezier

export type Prop =
  | 'translateX'
  | 'translateY'
  | 'rotate'
  | 'skewX'
  | 'skewY'
  | 'scale'
  | 'scaleX'
  | 'scaleY'
  | 'opacity'
  | 'fillOpacity'
  | 'strokeOpacity'
  | 'strokeDashoffset'
  | 'fill'
  | 'stroke'
  | 'strokeWidth'
  | 'd'
  | 'cx'
  | 'cy'
  | 'r'
  | 'rx'
  | 'ry'
  | 'x'
  | 'y'
  | 'width'
  | 'height'
  | 'x1'
  | 'y1'
  | 'x2'
  | 'y2'

export interface Animation {
  /** Seconds, 0.1 to 30. */
  duration: number
  /** Repeats while its state is active, or as a gesture in mode loop. */
  loop?: boolean
  /** Seconds between starts, random in [min, max]. `loop` and `every` exclude each other; neither: a one-shot. */
  every?: [number, number]
  /** Default for its tracks. */
  times?: number[]
  /** Default for its tracks. */
  ease?: Ease | Ease[]
  /** 1 to 16. */
  tracks: Track[]
}

export interface Track {
  /** Ids in the artwork (g, shapes or text). */
  target: string | string[]
  prop: Prop
  /** 2 to 16 keyframes; null: the artwork's own value (absolute props only). */
  values: (number | string | null)[]
  /** Same length as `values`, strictly ascending, first 0, last 1; default: even. */
  times?: number[]
  /** One for all segments or one per segment; default linear. */
  ease?: Ease | Ease[]
  /** 0 to 1, loops: sampled at (t / duration + phase) mod 1. */
  phase?: number
  /** Seconds, one-shots: this track starts later. */
  delay?: number
}

/**
 * A live input: where it looks (`lookX`, `lookY`: -1 to 1, x to the right, y down), how open
 * its mouth is (0 to 1), how fast it walks (`pace`: 0 to 1), which way it faces (`turn`: -1
 * left, 0 the viewer, 1 right), and whether the user holds it (`held`: 0 to 1).
 */
export type DriveInput = 'lookX' | 'lookY' | 'mouth' | 'pace' | 'turn' | 'held'

export const DRIVE_INPUTS: readonly DriveInput[] = ['lookX', 'lookY', 'mouth', 'pace', 'turn', 'held']

/** An input moving parts: its values sit along the input's range, as a track's keyframes sit along its time. */
export interface Drive {
  input: DriveInput
  /** Ids in the artwork (g, shapes, text or use). */
  target: string | string[]
  prop: Prop
  /** 2 to 16; null: the artwork's own value. */
  values: (number | string | null)[]
  /** Where along the input's range each value sits, 0 to 1, rising, first 0, last 1; default: even. */
  at?: number[]
  /** One for all segments or one per segment; default linear. */
  ease?: Ease | Ease[]
}

export type MouthProp = 'scaleY' | 'scaleX' | 'scale' | 'translateY' | 'ry' | 'r' | 'height'

export type MouthDrive =
  /** A path morph; `closed` defaults to the element's own `d`. */
  | { target: string; open: string; closed?: string }
  /** A number; `closed` defaults to the artwork's own value (1 for scales, 0 for translate). */
  | { target: string; prop: MouthProp; open: number; closed?: number }

export interface Voice {
  /** BCP 47; default "en-US". */
  lang?: string
  /** Up to 5 fragments of browser voice names, tried in order, case-insensitive. */
  prefer?: string[]
  /** 0.7 to 1.4; default: derived per session. */
  pitch?: number
  /** 0.8 to 1.3, default 1 (the user's speed preference multiplies afterwards). */
  rate?: number
}

export interface Line {
  /** "L<boot>-<n>": never repeats. */
  id: string
  at: number
  from: 'agent' | 'user'
  text: string
  lang?: string
  gesture?: string
  askId?: string
  via?: 'click' | 'typed' | 'voice'
  /** Agent lines only. */
  outcome?: Outcome
  /** The browser voice that spoke it. */
  voice?: string
}

export interface Ask {
  /** "Q<boot>-<n>". */
  id: string
  lineId: string
  text: string
  /** 0 to 4, each 1 to 40 characters. */
  choices: string[]
  freeText: boolean
  askedAt: number
  /** A tool or hook call is waiting for the answer. */
  waiting: boolean
}

/** The per-terminal record. `look === null`: the built-in default look. */
export interface AvatarInfo {
  terminalId: string
  projectId: string | null
  /** false: the user turned it off; nothing is shown or spoken. */
  on: boolean
  /** The agent's own name, or "". */
  name: string
  designed: boolean
  /** +1 on every design change (only). Compare with `!==` only. */
  revision: number
  look: Look | null
  voice: Voice
  via: 'tools' | 'command'
  held: { name: string; mode: 'hold' | 'loop'; since: number } | null
  ask: Ask | null
  /** Newest last, at most 30. */
  lines: Line[]
  createdAt: number
  /** A client ignores a fetched record older than the one it holds. */
  updatedAt: number
}

/** `GET /api/avatar`. */
export interface AvatarList {
  enabled: boolean
  sessions: 'asked' | 'any'
  avatars: AvatarInfo[]
  /** Terminal ids of running sessions that called an avatar tool without having one. */
  asked: string[]
}

/**
 * The figure's state, first match wins. `blocked`: the session needs a permission or an
 * answer (the avatar is quiet then). The app's own ring and tooltip keep using the agents
 * feature's `tone(t)` and `stateLabel(t)`, so the avatar agrees with the tab and the card.
 */
export function avatarState(i: { blocked: boolean; presenting: boolean; listening: boolean; askOpen: boolean; tone: SessionTone }): AvatarState {
  if (i.blocked || i.tone === 'attention') return 'attention'
  if (i.tone === 'error') return 'error'
  if (i.presenting) return 'speaking'
  if (i.listening) return 'listening'
  if (i.askOpen) return 'waiting'
  if (i.tone === 'working' || i.tone === 'starting') return 'working'
  if (i.tone === 'unread') return 'done'
  if (i.tone === 'exited') return 'sleeping'
  return 'idle'
}

/**
 * A stable hash of a string (FNV-1a, 32 bits, never negative): the default look's hue, the
 * voice an avatar gets and its pitch all derive from the terminal id through it, so a
 * session looks and sounds the same after every reload.
 */
export function stableHash(text: string): number {
  let h = 0x811c9dc5
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i)
    h = Math.imul(h, 0x01000193)
  }
  return h >>> 0
}

/** Freezes a value and everything inside it: for the app's own looks and animations, which are shared between figures. */
export function deepFreeze<T>(value: T): T {
  if (typeof value === 'object' && value !== null && !Object.isFrozen(value)) {
    Object.freeze(value)
    for (const key of Object.keys(value)) deepFreeze((value as Record<string, unknown>)[key])
  }
  return value
}
