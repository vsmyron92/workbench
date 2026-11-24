// The avatar preferences of this browser, persisted as `wb.avatar.v1`: whether avatars are
// shown, the voice choice, speed, volume, reduced motion, where the group was dragged to,
// and per session the voice it got and the last line the user saw.
//
// Every window of the browser shares them: a `storage` event from another window is
// merged into the running store, so turning voice off in one window silences the others.
// The one exception is `pos`: each window keeps its own position while it is open.
//
// Contract module for the platform feature: `useAvatarPrefs` (with the `AvatarPrefs` type).
// Its Settings section and the phone's More tab read and change the same preferences.
// Nothing else is exported, on purpose.

import { create } from 'zustand'
import { createJSONStorage, persist, type StateStorage } from 'zustand/middleware'

export interface AvatarPrefs {
  /** Avatars are shown in this browser. false: nothing is shown and nothing is spoken here. */
  show: boolean
  /** Lines are said aloud. null: not chosen yet in this browser (captions only, and the choice is offered). */
  voice: boolean | null
  /** Online browser voices are allowed: what an avatar says is then sent to the browser maker's speech service. */
  online: boolean
  /** 0 to 1. */
  volume: number
  /** 0.7 to 1.5; multiplies each avatar's own rate. */
  speed: number
  /** The user's own switch; the system setting applies besides it. */
  reduceMotion: boolean
  /** The avatars walk about the workspace window and meet. false: they stand in their column. Never with reduced motion. */
  roam: boolean
  /** Where the group was dragged to, as fractions of the viewport (0 to 1); null: the default place. Not shared between open windows. */
  pos: { x: number; y: number } | null
  /** Terminal id -> the browser voice that session's avatar speaks with. */
  voices: Record<string, string>
  /** Terminal id -> the id of the newest agent line the user has seen. */
  seen: Record<string, string>
}

type Settings = Pick<AvatarPrefs, 'show' | 'voice' | 'online' | 'volume' | 'speed' | 'reduceMotion' | 'roam' | 'pos'>

interface AvatarPrefsStore extends AvatarPrefs {
  /** Changes settings. Numbers are kept in their ranges; what is no valid value is ignored. */
  setPrefs: (patch: Partial<Settings>) => void
  rememberVoice: (terminalId: string, voiceName: string) => void
  markSeen: (terminalId: string, lineId: string) => void
  /** The terminal is gone: its voice and its last seen line are forgotten. */
  forget: (terminalId: string) => void
  /** Forgets every terminal that is not among `alive` (terminals removed while this browser was closed). */
  prune: (alive: Iterable<string>) => void
}

const NAME = 'wb.avatar.v1'
const VERSION = 1
/** At most this many sessions are remembered in `voices` and in `seen`; the ones written longest ago go first. */
const MAX_REMEMBERED = 256
const MAX_VALUE_CHARS = 200
/** A terminal id as the server makes them (terminals/mod.rs `valid_id`). */
const TERMINAL_ID = /^[a-z0-9]{1,40}$/

const DEFAULTS: AvatarPrefs = { show: true, voice: null, online: false, volume: 1, speed: 1, reduceMotion: false, roam: true, pos: null, voices: {}, seen: {} }

const inRange = (value: unknown, min: number, max: number, fallback: number): number => (typeof value === 'number' && Number.isFinite(value) ? Math.min(Math.max(value, min), max) : fallback)
const flag = (value: unknown, fallback: boolean): boolean => (typeof value === 'boolean' ? value : fallback)

function cleanPos(value: unknown, fallback: AvatarPrefs['pos']): AvatarPrefs['pos'] {
  if (value === null) return null
  if (typeof value !== 'object' || Array.isArray(value)) return fallback
  const { x, y } = value as { x?: unknown; y?: unknown }
  if (typeof x !== 'number' || typeof y !== 'number' || !Number.isFinite(x) || !Number.isFinite(y)) return fallback
  return { x: Math.min(Math.max(x, 0), 1), y: Math.min(Math.max(y, 0), 1) }
}

/** A terminal-to-text record rebuilt from its own entries: only terminal ids, only short texts, at most `MAX_REMEMBERED`. */
function cleanRecord(value: unknown): Record<string, string> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return {}
  const entries: [string, string][] = []
  for (const key of Object.keys(value)) {
    const text: unknown = (value as Record<string, unknown>)[key]
    if (TERMINAL_ID.test(key) && typeof text === 'string' && text.length > 0 && text.length <= MAX_VALUE_CHARS) entries.push([key, text])
  }
  return Object.fromEntries(entries.slice(-MAX_REMEMBERED))
}

/** The settings of `value`, each one valid or else the one of `fallback`. */
function cleanSettings(value: Partial<Record<keyof Settings, unknown>>, fallback: Settings): Settings {
  return {
    show: flag(value.show, fallback.show),
    voice: value.voice === null ? null : typeof value.voice === 'boolean' ? value.voice : fallback.voice,
    online: flag(value.online, fallback.online),
    volume: inRange(value.volume, 0, 1, fallback.volume),
    speed: inRange(value.speed, 0.7, 1.5, fallback.speed),
    reduceMotion: flag(value.reduceMotion, fallback.reduceMotion),
    roam: flag(value.roam, fallback.roam),
    pos: cleanPos(value.pos, fallback.pos),
  }
}

const samePos = (a: AvatarPrefs['pos'], b: AvatarPrefs['pos']): boolean => a === b || (a !== null && b !== null && a.x === b.x && a.y === b.y)

function sameRecord(a: Record<string, string>, b: Record<string, string>): boolean {
  const keys = Object.keys(a)
  if (keys.length !== Object.keys(b).length) return false
  return keys.every((key) => Object.hasOwn(b, key) && a[key] === b[key])
}

/** The record with one entry written last, bounded. */
function put(record: Record<string, string>, key: string, value: string): Record<string, string> {
  return Object.fromEntries([...Object.entries(record).filter(([k]) => k !== key), [key, value] as [string, string]].slice(-MAX_REMEMBERED))
}

const without = (record: Record<string, string>, keep: (key: string) => boolean): Record<string, string> => Object.fromEntries(Object.entries(record).filter(([key]) => keep(key)))

/**
 * What was stored, laid over the running state. `keepPos`: another window wrote it while
 * this one is open, and this window's position stays its own. Whatever is no valid value
 * in storage (a hand-edited or older entry) falls back to the default. The same state
 * object comes back when nothing differs, so nobody is told about nothing.
 */
function merge(stored: unknown, current: AvatarPrefsStore, keepPos: boolean): AvatarPrefsStore {
  if (typeof stored !== 'object' || stored === null || Array.isArray(stored)) return current
  const from = stored as Partial<Record<keyof AvatarPrefs, unknown>>
  const settings = cleanSettings(from, DEFAULTS)
  if (keepPos) settings.pos = current.pos
  else if (samePos(settings.pos, current.pos)) settings.pos = current.pos
  const voices = cleanRecord(from.voices)
  const seen = cleanRecord(from.seen)
  const next: AvatarPrefs = { ...settings, voices: sameRecord(voices, current.voices) ? current.voices : voices, seen: sameRecord(seen, current.seen) ? current.seen : seen }
  const same = (Object.keys(next) as (keyof AvatarPrefs)[]).every((key) => next[key] === current[key])
  return same ? current : { ...current, ...next }
}

/** localStorage where there is one; a browser that blocks it (or no browser at all) keeps the preferences for the page load. */
const browserStorage: StateStorage = {
  getItem: (name) => {
    try {
      return typeof window === 'undefined' ? null : window.localStorage.getItem(name)
    } catch {
      return null
    }
  },
  setItem: (name, value) => {
    try {
      if (typeof window !== 'undefined') window.localStorage.setItem(name, value)
    } catch {
      // Blocked or full: the preference holds until the page is closed.
    }
  },
  removeItem: (name) => {
    try {
      if (typeof window !== 'undefined') window.localStorage.removeItem(name)
    } catch {
      // Nothing to remove then.
    }
  },
}

/** Set once the stored preferences were read at start-up: every later read is another window's writing. */
let started = false

export const useAvatarPrefs = create<AvatarPrefsStore>()(
  persist(
    (set, get) => ({
      ...DEFAULTS,
      setPrefs: (patch) => {
        if (typeof patch !== 'object' || patch === null) return
        const s = get()
        // What is no valid value falls back to what holds now, so only what really changes is set.
        const cleaned = cleanSettings({ ...s, ...patch }, s)
        const next: Partial<Settings> = {}
        let changed = false
        for (const key of Object.keys(cleaned) as (keyof Settings)[]) {
          if (key === 'pos' ? samePos(cleaned.pos, s.pos) : cleaned[key] === s[key]) continue
          Object.assign(next, { [key]: cleaned[key] })
          changed = true
        }
        if (changed) set(next)
      },
      rememberVoice: (terminalId, voiceName) => {
        const { voices } = get()
        if (!TERMINAL_ID.test(terminalId) || typeof voiceName !== 'string' || !voiceName || voiceName.length > MAX_VALUE_CHARS) return
        if (Object.hasOwn(voices, terminalId) && voices[terminalId] === voiceName) return
        set({ voices: put(voices, terminalId, voiceName) })
      },
      markSeen: (terminalId, lineId) => {
        const { seen } = get()
        if (!TERMINAL_ID.test(terminalId) || typeof lineId !== 'string' || !lineId || lineId.length > MAX_VALUE_CHARS) return
        if (Object.hasOwn(seen, terminalId) && seen[terminalId] === lineId) return
        set({ seen: put(seen, terminalId, lineId) })
      },
      forget: (terminalId) => {
        const { voices, seen } = get()
        if (!Object.hasOwn(voices, terminalId) && !Object.hasOwn(seen, terminalId)) return
        const others = (key: string) => key !== terminalId
        set({ voices: without(voices, others), seen: without(seen, others) })
      },
      prune: (alive) => {
        const keep = new Set<string>(alive)
        const { voices, seen } = get()
        const stays = (key: string) => keep.has(key)
        if (Object.keys(voices).every(stays) && Object.keys(seen).every(stays)) return
        set({ voices: without(voices, stays), seen: without(seen, stays) })
      },
    }),
    {
      name: NAME,
      version: VERSION,
      storage: createJSONStorage<AvatarPrefs>(() => browserStorage),
      partialize: (s): AvatarPrefs => ({ show: s.show, voice: s.voice, online: s.online, volume: s.volume, speed: s.speed, reduceMotion: s.reduceMotion, roam: s.roam, pos: s.pos, voices: s.voices, seen: s.seen }),
      // Whatever version wrote it, it is read through the same cleaning.
      migrate: (stored) => stored as AvatarPrefs,
      merge: (stored, current) => merge(stored, current, started),
    },
  ),
)
started = true

/** Another window of this browser wrote the preferences: take them over, all but the position. */
function onStorage(e: { key: string | null; newValue: string | null }) {
  if (e.key !== NAME || typeof e.newValue !== 'string') return
  let version: unknown
  try {
    version = (JSON.parse(e.newValue) as { version?: unknown } | null)?.version
  } catch {
    return
  }
  // Another version of the app wrote it: not this one's to read, and never to write back.
  if (version !== VERSION) return
  // Reads storage again and merges without writing, so two windows never answer each other for ever.
  void useAvatarPrefs.persist.rehydrate()
}

if (typeof window !== 'undefined' && typeof window.addEventListener === 'function') window.addEventListener('storage', onStorage)
