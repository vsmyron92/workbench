// The words of the status-bar widget, the palette commands and what they report. They are
// loaded with the app, so they live apart from the views (Bubble.tsx, menu.ts), which are
// loaded with the overlay. Where both name the same thing they use the same words:
// words.test.ts holds the two sets to each other.

/** The status-bar widget's menu, and the app's own answers to what was chosen there. */
export const STATUS_WORDS = {
  stop: 'Stop speaking',
  voice: 'Voice in this browser',
  show: 'Show avatars',
  settings: 'Avatar settings',
  /** Beside the face while avatars are hidden in this browser. */
  hidden: 'hidden',
  hiddenTitle: 'Avatars are hidden in this browser: nothing is shown or said here',
  voiceOn: 'voice on',
  voiceOff: 'voice off',
  /** The question before online voices are used, and the sentence that says what allowing them means (rule 7). */
  askOnline: 'Allow online voices?',
  online: 'What an avatar says is sent to the browser maker’s speech service to be spoken.',
  allowOnline: 'Allow online voices',
  noVoices: 'This browser has no voices: avatars show captions.',
  /** A request to give or to turn off an avatar failed without a reason of the server's. */
  failed: 'The avatar could not be changed.',
  tellAgain: 'Tell it again',
  hiddenHere: 'Avatars are hidden in this browser: show them to see it.',
} as const

/** The palette commands, as 10.6 of the contract names them. */
export const COMMAND_WORDS = {
  group: 'Avatar',
  show: 'Avatar: Show or Hide Avatars',
  voice: 'Avatar: Voice On or Off',
  stop: 'Avatar: Stop Speaking',
  give: 'Avatar: Give This Session an Avatar',
  turnOff: 'Avatar: Turn Off for This Session',
  settings: 'Avatar Settings',
} as const

/** The longest name the app's own phrases carry whole (as `calledAs` of Bubble.tsx). */
const MAX_CALLED_CHARS = 32
/**
 * The longest entry of the widget's menu. The menu host draws an entry at most 360 px wide
 * and cuts what does not fit at its end: a session's title gets the room that what the
 * entry does leaves, so that "give an avatar (asked)" stays in sight behind it.
 */
export const MAX_ENTRY_CHARS = 45
/** As much of a session's title as a sentence about it carries. */
export const MAX_TITLE_CHARS = 30

/** A text of at most `max` characters: a longer one is cut and ends in an ellipsis. */
export function cut(text: string, max: number): string {
  const chars = Array.from(typeof text === 'string' ? text : '')
  return chars.length > max ? `${chars.slice(0, Math.max(max - 1, 0)).join('').trimEnd()}…` : chars.join('')
}

/** What an avatar is called in the app's phrases: the agent's own name, else the session's title, cut to a few words. */
export function called(name: string, title: string): string {
  return cut(name || title, MAX_CALLED_CHARS)
}

/** A session's title as the app's phrases name it, at most `max` characters. A session without one is still named. */
export function sessionName(title: string, max: number = MAX_TITLE_CHARS): string {
  return cut((typeof title === 'string' ? title : '').trim() || 'Untitled session', max)
}

/** A menu entry about a session: as much of its title as fits, then what the entry does. */
const entry = (title: string, does: string): string => `${sessionName(title, MAX_ENTRY_CHARS - does.length)}${does}`

/** "Avatar: Reply to …", with the name of the avatar the reply goes to. A name that was cut already ends in the ellipsis this title ends in. */
export const replyCommand = (calledAs: string): string => `Avatar: Reply to ${calledAs.replace(/…$/, '')}…`

/** "<title>: give an avatar", marked for a session that asked for one. */
export const giveEntry = (title: string, asked: boolean): string => entry(title, `: give an avatar${asked ? ' (asked)' : ''}`)
export const turnOffEntry = (title: string): string => entry(title, ': turn avatar off')
export const moreSessions = (n: number): string => (n === 1 ? '1 more session' : `${n} more sessions`)

/** The widget's tooltip while avatars are shown: how many, whether they speak, and who asked for one. */
export function shownTitle(count: number, voice: boolean, asked: number): string {
  const shown = count <= 0 ? 'No avatar shown' : count === 1 ? '1 avatar shown' : `${count} avatars shown`
  const asks = asked <= 0 ? '' : asked === 1 ? ' · 1 session asks for one' : ` · ${asked} sessions ask for one`
  return `${shown} · ${voice ? STATUS_WORDS.voiceOn : STATUS_WORDS.voiceOff}${asks}`
}

/**
 * The session got its avatar but could not be told of it now: it shows a dialog, is still
 * starting, or the user is typing in it. Workbench tells it by itself when it is next idle.
 */
export const notTold = (title: string): string => `${sessionName(title)} has an avatar, but the session could not be told now. Workbench tells it when it is next idle.`
