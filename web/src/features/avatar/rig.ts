// The window's one engine, speaker and arbiter, wired to the browser. The overlay makes
// a rig when it mounts and disposes of it when it goes; everything a browser provides is
// handed to the pure modules of lib/ here, and nowhere else.

import { dictationLive } from '@/features/agents/lib/dictation'
import { arbiterCalls } from './api'
import { createArbiter, type Arbiter, type ArbiterState } from './lib/arbiter'
import { createEngine, type Engine } from './lib/engine'
import { browserSpeech, createSpeaker, type SpeakPrefs, type Speaker, type SpeakerEvents } from './lib/speech'
import { hadGesture, idleMs } from './runtime'

export interface Rig {
  engine: Engine
  speaker: Speaker
  arbiter: Arbiter
  /** Stops speech at once, gives the leases back and ends the frame loop. Nothing else is posted. */
  dispose(): void
}

/** The base64url alphabet. */
const URL_SAFE = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_'

/** An id for this page load: 12 random bytes, base64url (16 characters). */
export function makeTab(random: (bytes: Uint8Array<ArrayBuffer>) => void): string {
  const bytes = new Uint8Array(12)
  random(bytes)
  let out = ''
  for (let i = 0; i < bytes.length; i += 3) {
    const n = (bytes[i] << 16) | (bytes[i + 1] << 8) | bytes[i + 2]
    out += URL_SAFE[(n >> 18) & 63] + URL_SAFE[(n >> 12) & 63] + URL_SAFE[(n >> 6) & 63] + URL_SAFE[n & 63]
  }
  return out
}

/** Made once per page load, however often the overlay mounts. */
let tab: string | null = null

function tabId(win: Window): string {
  if (tab === null) {
    tab = makeTab((bytes) => {
      try {
        win.crypto.getRandomValues(bytes)
      } catch {
        // A browser without it: the id only has to differ between this browser's tabs.
        for (let i = 0; i < bytes.length; i++) bytes[i] = Math.floor(Math.random() * 256)
      }
    })
  }
  return tab
}

/**
 * A rig for `win`. `prefs` and `state` are what holds when it is made; `events` hears
 * what the speaker presents (the arbiter listens by itself, and acknowledges every line).
 */
export function createRig(win: Window, prefs: SpeakPrefs, state: ArbiterState, events: SpeakerEvents): Rig {
  const now = () => win.performance.now()
  const later = (cb: () => void, ms: number) => win.setTimeout(cb, ms)
  const clear = (id: number) => win.clearTimeout(id)
  const engine = createEngine({
    requestFrame: (cb) => win.requestAnimationFrame(cb),
    cancelFrame: (id) => win.cancelAnimationFrame(id),
    random: Math.random,
    now,
  })
  const speaker = createSpeaker({ ...browserSpeech(win), now, setTimeout: later, clearTimeout: clear, unlocked: () => hadGesture(win), micLive: dictationLive }, prefs, events)
  const arbiter = createArbiter(
    {
      tab: tabId(win),
      now,
      setTimeout: later,
      clearTimeout: clear,
      visible: () => win.document.visibilityState === 'visible',
      idleMs: () => idleMs(now()),
      ...arbiterCalls,
    },
    speaker,
    state,
  )
  let disposed = false
  return {
    engine,
    speaker,
    arbiter,
    dispose() {
      if (disposed) return
      disposed = true
      // The arbiter first: it silences what it queued and gives its leases back, and the speaker then has nothing left to report.
      arbiter.dispose()
      speaker.dispose()
      engine.dispose()
    },
  }
}
