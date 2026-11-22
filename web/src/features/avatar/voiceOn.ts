// Turning voice on from a place where no overlay is mounted: the status bar or the palette
// while nothing is on stage, or while avatars are hidden. (A mounted overlay decides the
// same way for itself, inside the click, because it also has to unlock its speaker.)
//
// Voice is this browser's speech. What the browser can say avatars' lines with decides what
// turning it on needs: voices on the device, nothing more; only online voices, the user's
// word that what an avatar says may be sent to the browser maker's speech service (rule 7);
// no voices at all, nothing can be turned on.
//
// Loaded on demand: listing the voices brings the speech code along.

import { confirmDialog, toast } from '@/shell/actions'
import { useAvatarPrefs } from './store'
import { voiceSource, voicesFor, type BrowserVoice } from './voices'
import { STATUS_WORDS } from './words'

/** A browser lists its voices some time after it was first asked for them: as long is waited for the list. */
export const VOICES_WAIT_MS = 2000
/** The language of an avatar that names none. */
const DEFAULT_LANG = 'en-US'

type Listed = Pick<BrowserVoice, 'lang' | 'onDevice'>

export interface VoiceOnHost {
  /** This browser's voices, once it has listed them. */
  voices(): Promise<readonly Listed[]>
  /** Online voices are allowed in this browser already. */
  online(): boolean
  /** Asks whether online voices may be used, with the sentence that says what that means. */
  confirmOnline(): Promise<boolean>
  /** Says that this browser has no voices. */
  tellNone(): void
  /** Changes the preferences: voice on, and with `online` the permission for online voices. */
  set(patch: { voice: true; online?: true }): void
}

/** `on`: voice is on now. `declined`: online voices were needed and not allowed. `none`: this browser has no voices. */
export type VoiceOnOutcome = 'on' | 'declined' | 'none'

export async function turnVoiceOn(host: VoiceOnHost, lang: string = DEFAULT_LANG): Promise<VoiceOnOutcome> {
  const kind = voicesFor(await host.voices(), lang)
  if (kind === 'none') {
    host.tellNone()
    return 'none'
  }
  if (kind === 'online' && host.online() !== true) {
    // Nothing is turned on before the answer, and nothing at all without a yes.
    if ((await host.confirmOnline()) !== true) return 'declined'
    host.set({ online: true, voice: true })
    return 'on'
  }
  host.set({ voice: true })
  return 'on'
}

interface VoiceList {
  read(): readonly Listed[]
  subscribe(changed: () => void): () => void
}

interface Timers {
  setTimeout(cb: () => void, ms: number): number
  clearTimeout(id: number): void
}

/**
 * A browser's voices once it has some. An empty list is waited on until the browser says
 * it changed, for `waitMs` at most: after that the list is taken as it is.
 */
export function listedVoices(list: VoiceList, timers: Timers, waitMs: number = VOICES_WAIT_MS): Promise<readonly Listed[]> {
  const now = list.read()
  if (now.length > 0) return Promise.resolve(now)
  return new Promise((resolve) => {
    let over = false
    let stop = () => {}
    const done = () => {
      if (over) return
      over = true
      timers.clearTimeout(timer)
      stop()
      resolve(list.read())
    }
    const timer = timers.setTimeout(done, waitMs)
    stop = list.subscribe(() => {
      if (list.read().length > 0) done()
    })
    // Told while subscribing: nothing is left to listen for. Else a first look can be what made the browser fill its list.
    if (over) stop()
    else if (list.read().length > 0) done()
  })
}

/** Turns voice on in this browser, asking first where only online voices could speak. */
export function voiceOnHere(): Promise<VoiceOnOutcome> {
  const w = window
  const source = voiceSource(w)
  return turnVoiceOn({
    voices: () => listedVoices(source, { setTimeout: (cb, ms) => w.setTimeout(cb, ms), clearTimeout: (id) => w.clearTimeout(id) }),
    online: () => useAvatarPrefs.getState().online,
    confirmOnline: () => confirmDialog({ title: STATUS_WORDS.askOnline, message: STATUS_WORDS.online, confirmLabel: STATUS_WORDS.allowOnline }),
    tellNone: () => void toast('info', STATUS_WORDS.noVoices),
    set: (patch) => useAvatarPrefs.getState().setPrefs(patch),
  })
}
