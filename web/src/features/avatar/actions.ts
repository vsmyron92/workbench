// What the status-bar widget and the palette commands do. Each does one thing: show or hide
// the avatars of this browser, turn its voice on or off, stop the line this tab presents,
// open a reply box, give a session an avatar or turn its avatar off, open the settings.
//
// Giving and turning off go through the device routes (api.ts): the server tells every
// window through `avatar.changed`, so nothing is written to the cache here. A refusal is
// shown as a toast with the server's own sentence.

import { openSettings, toast } from '@/shell/actions'
import { avatarApi } from './api'
import { avatarActions, hasOverlay } from './runtime'
import { useAvatarPrefs } from './store'
import { STATUS_WORDS, notTold } from './words'

const errorText = (e: unknown): string => (e instanceof Error && e.message ? e.message : STATUS_WORDS.failed)

/** Shows the avatars in this browser, or hides them: hidden, nothing is shown and nothing is said here. */
export function setShow(on: boolean): void {
  useAvatarPrefs.getState().setPrefs({ show: on === true })
}

export function toggleShow(): void {
  setShow(!useAvatarPrefs.getState().show)
}

/** A turning on that is still looking at this browser's voices, or waits for the user's answer. */
let turningOn = false

/**
 * Voice on or off in this browser. Turning it on is the user's click. A mounted overlay
 * takes it from there (it unlocks its speaker inside the click). Where none is mounted,
 * this browser's voices are looked at first: only online ones are asked about, and with no
 * voices nothing is turned on. Turning it off is the preference alone, and reaches every
 * window of the browser.
 */
export function setVoice(on: boolean): void {
  if (on !== true || hasOverlay()) {
    avatarActions.voice(on === true)
    return
  }
  if (turningOn) return
  turningOn = true
  void import('./voiceOn')
    .then((m) => m.voiceOnHere())
    .catch((e: unknown) => {
      // The voices could not be looked at: the choice is still the user's, and online voices stay as they were.
      console.error('[workbench] the avatar voices could not be listed', e)
      useAvatarPrefs.getState().setPrefs({ voice: true })
    })
    .finally(() => {
      turningOn = false
    })
}

export function toggleVoice(): void {
  setVoice(useAvatarPrefs.getState().voice !== true)
}

/** Stops what this tab presents, aloud or as a caption. false: it presents nothing, and nothing was done. */
export function stopSpeaking(): boolean {
  return avatarActions.stop()
}

/** Opens this avatar's bubble and moves focus into its reply box. The one thing that does. */
export function replyTo(terminalId: string): boolean {
  return avatarActions.reply(terminalId)
}

/**
 * Gives a session an avatar, or turns its avatar on again. A session that was not started
 * with the Avatar option is told by the server; when it cannot be told now, the user is,
 * with the way to try again.
 */
export async function giveAvatar(session: { id: string; title: string }): Promise<void> {
  try {
    const answer = await avatarApi.on(session.id)
    if (answer?.briefed === false) {
      toast('warning', notTold(session.title), { action: { label: STATUS_WORDS.tellAgain, run: () => void giveAvatar(session) }, timeout: 12_000 })
    } else if (!useAvatarPrefs.getState().show) {
      // Given, and nothing appears: say why, with the way to see it.
      toast('info', STATUS_WORDS.hiddenHere, { action: { label: STATUS_WORDS.show, run: () => setShow(true) } })
    }
  } catch (e) {
    toast('error', errorText(e))
  }
}

/** Turns a session's avatar off, on every device. Its look is kept. */
export async function turnAvatarOff(session: { id: string }): Promise<void> {
  try {
    await avatarApi.off(session.id)
  } catch (e) {
    toast('error', errorText(e))
  }
}

export function openAvatarSettings(): void {
  openSettings('avatar')
}
