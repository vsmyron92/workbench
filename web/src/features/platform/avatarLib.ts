// Pure helpers for the Avatar section of Settings and the phone's More tab (tested in
// avatarLib.test.ts): which of the three choices config.toml holds and what the form sends
// for each, and the words for this browser's voices and for the outcome of a voice test.

import type { BrowserVoice, TestVoiceResult } from '@/features/avatar/voices'
import type { GlobalConfig } from './types'

// ---------------------------------------------------------------- the server's choice

/** "Sessions may show an avatar": not at all, when the user gives them one, or whenever they want. */
export type AvatarChoice = 'off' | 'asked' | 'any'

export const AVATAR_CHOICES: readonly { value: AvatarChoice; label: string; hint: string }[] = [
  { value: 'off', label: 'Off', hint: 'No session has an avatar. The composer shows no Avatar option, and a session that tries to use one is refused.' },
  {
    value: 'asked',
    label: 'When I give them one',
    hint: 'A session gets an avatar when it is started with the Avatar option, or given one later from the status bar or the command palette. Sessions that have one already keep it until you turn it off for each.',
  },
  { value: 'any', label: 'Whenever they want', hint: 'A session gets an avatar the first time it uses one, and may start another session with one. You can turn it off for each session; a session never turns it back on.' },
]

/** What the row says under its label for a choice. */
export function choiceHint(choice: AvatarChoice): string {
  return AVATAR_CHOICES.find((c) => c.value === choice)?.hint ?? ''
}

/**
 * The choice a config holds (`[avatar]` of config.toml; absent: the defaults). `enabled =
 * false` decides alone: no session has an avatar then, whatever `sessions` says.
 */
export function avatarChoice(avatar: GlobalConfig['avatar']): AvatarChoice {
  if (avatar?.enabled === false) return 'off'
  return avatar?.sessions === 'asked' ? 'asked' : 'any'
}

/**
 * What the form sends as `avatar` for a choice: both keys every time, `null` for a default,
 * so nothing is written for it. The default choice leaves no `[avatar]` table in the file.
 */
export function avatarPatch(choice: AvatarChoice): { enabled: false | null; sessions: 'asked' | null } {
  switch (choice) {
    case 'off':
      return { enabled: false, sessions: null }
    case 'asked':
      return { enabled: null, sessions: 'asked' }
    default:
      return { enabled: null, sessions: null }
  }
}

// ---------------------------------------------------------------- this browser's voices

/** Rule 7 of the avatar slice: what allowing online voices means. The same sentence as under an avatar's first line. */
export const ONLINE_VOICES = 'What an avatar says is sent to the browser maker’s speech service to be spoken.'
export const NO_VOICES = 'This browser has no voices: avatars show captions.'

/** How many of a browser's voices are on the device, and how many are online ones. */
export interface VoiceCounts {
  device: number
  online: number
}

export function countVoices(voices: readonly Pick<BrowserVoice, 'onDevice'>[]): VoiceCounts {
  let device = 0
  let online = 0
  for (const v of Array.isArray(voices) ? voices : []) {
    if (v?.onDevice === true) device++
    else if (v) online++
  }
  return { device, online }
}

const voicesOf = (n: number): string => (n === 1 ? '1 voice' : `${n} voices`)
const onlineOf = (n: number): string => (n === 1 ? '1 online voice' : `${n} online voices`)

/** One sentence on what this browser can speak with. */
export function voicesText(c: VoiceCounts): string {
  if (c.device === 0 && c.online === 0) return NO_VOICES
  if (c.online === 0) return `This browser has ${voicesOf(c.device)} on this device and no online voices.`
  if (c.device === 0) return `This browser has ${onlineOf(c.online)} and none on this device.`
  return `This browser has ${voicesOf(c.device)} on this device and ${onlineOf(c.online)}.`
}

/**
 * What the voice choice comes to in this browser, in one line. `warning`: voice is on, but
 * nothing can be heard as things are.
 */
export function voiceStatus(prefs: { voice: boolean | null; online: boolean }, c: VoiceCounts): { tone: 'muted' | 'warning'; text: string } {
  if (prefs.voice === null) return { tone: 'muted', text: 'Not chosen yet: an avatar offers the choice under its first line.' }
  if (prefs.voice !== true) return { tone: 'muted', text: 'Captions only.' }
  if (c.device === 0 && c.online === 0) return { tone: 'warning', text: NO_VOICES }
  if (c.device === 0 && prefs.online !== true) return { tone: 'warning', text: 'This browser has only online voices: avatars show captions until online voices are allowed.' }
  return { tone: 'muted', text: 'Lines are said aloud.' }
}

/**
 * Whether the phone's More tab offers "Allow online voices": where voice cannot work without
 * them (the browser has only online voices), and where they are allowed already, so that
 * they can be withdrawn.
 */
export function offersOnline(prefs: { online: boolean }, c: VoiceCounts): boolean {
  return prefs.online === true || (c.device === 0 && c.online > 0)
}

/** The longest list of voices the Test voice picker shows. */
export const MAX_VOICE_OPTIONS = 500

/**
 * The voices as the Test voice picker lists them: each marked "on this device" or "online",
 * the ones on the device first, each name once (a voice is tested by its name).
 */
export function voiceOptions(voices: readonly BrowserVoice[]): { value: string; label: string }[] {
  const seen = new Set<string>()
  const out: { value: string; label: string; onDevice: boolean }[] = []
  for (const v of Array.isArray(voices) ? voices : []) {
    if (typeof v?.name !== 'string' || !v.name || seen.has(v.name)) continue
    seen.add(v.name)
    const onDevice = v.onDevice === true
    const lang = typeof v.lang === 'string' && v.lang ? ` · ${v.lang}` : ''
    out.push({ value: v.name, label: `${v.name}${lang} · ${onDevice ? 'on this device' : 'online'}`, onDevice })
    if (out.length >= MAX_VOICE_OPTIONS) break
  }
  // On-device voices first; within each kind the browser's own order.
  return [...out.filter((o) => o.onDevice), ...out.filter((o) => !o.onDevice)].map(({ value, label }) => ({ value, label }))
}

// ---------------------------------------------------------------- the test sentence

const TEST_TEXT: ReadonlyMap<TestVoiceResult, { tone: 'muted' | 'warning'; text: string }> = new Map([
  ['spoken', { tone: 'muted', text: 'The test sentence is being said.' }],
  ['no-speech', { tone: 'warning', text: 'This browser cannot speak.' }],
  ['no-click', { tone: 'muted', text: 'Press Test voice to hear the sentence.' }],
  ['no-voices', { tone: 'warning', text: 'This browser has no voice to say it with.' }],
  ['online-only', { tone: 'warning', text: 'Only an online voice could say it: allow online voices first.' }],
  ['listening', { tone: 'muted', text: 'Nothing is said while a dictation is listening.' }],
  ['busy', { tone: 'muted', text: 'The last test is still being said.' }],
] as const)

/** What the Settings section says after Test voice was pressed. */
export function testText(result: TestVoiceResult): { tone: 'muted' | 'warning'; text: string } {
  return TEST_TEXT.get(result) ?? { tone: 'warning', text: 'The test sentence could not be said.' }
}

// ---------------------------------------------------------------- speed and volume

export const SPEED = { min: 0.7, max: 1.5, step: 0.05 } as const
export const VOLUME = { min: 0, max: 1, step: 0.05 } as const

const finite = (n: unknown, fallback: number): number => (typeof n === 'number' && Number.isFinite(n) ? n : fallback)

/** 1 → "1×", 1.25 → "1.25×": the factor an avatar's own pace is multiplied with. */
export function speedText(speed: number): string {
  return `${Number(finite(speed, 1).toFixed(2))}×`
}

/** 0.8 → "80%". */
export function volumeText(volume: number): string {
  return `${Math.round(Math.min(Math.max(finite(volume, 1), 0), 1) * 100)}%`
}
