// What the status-bar widget and the palette commands do: the preferences they change, the
// device routes they call, what they ask of the overlay, and what they tell the user.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiError } from '@/api/client'
import { giveAvatar, openAvatarSettings, replyTo, setShow, setVoice, stopSpeaking, toggleShow, toggleVoice, turnAvatarOff } from './actions'
import { setOverlayActions, type OverlayActions } from './runtime'
import { useAvatarPrefs } from './store'

interface Toasted {
  level: string
  message: string
  action?: { label: string; run: () => void }
  timeout?: number
}

/** What the stand-ins saw, and what they answer. (Hoisted: the stand-ins are made before this file's own lines run.) */
const seen = vi.hoisted(() => ({
  toasts: [] as Toasted[],
  settings: [] as (string | undefined)[],
  calls: [] as string[],
  on: (() => Promise.resolve({})) as (id: string) => Promise<unknown>,
  off: (() => Promise.resolve({})) as (id: string) => Promise<unknown>,
  voiceOn: (() => Promise.resolve('on')) as () => Promise<string>,
}))

vi.mock('@/shell/actions', () => ({
  toast: (level: string, message: string, opts: { action?: Toasted['action']; timeout?: number } = {}) => void seen.toasts.push({ level, message, action: opts.action, timeout: opts.timeout }),
  openSettings: (section?: string) => void seen.settings.push(section),
}))
vi.mock('./api', async (original) => ({
  ...(await original<typeof import('./api')>()),
  avatarApi: {
    on: (id: string) => {
      seen.calls.push(`on ${id}`)
      return seen.on(id)
    },
    off: (id: string) => {
      seen.calls.push(`off ${id}`)
      return seen.off(id)
    },
  },
}))
vi.mock('./voiceOn', () => ({
  voiceOnHere: () => {
    seen.calls.push('look at the voices')
    return seen.voiceOn()
  },
}))

const prefs = () => useAvatarPrefs.getState()
/** Lets what an action started without waiting for it (a module that loads, a request that answers) come to its end. */
const settled = async () => {
  for (let i = 0; i < 6; i++) await new Promise((r) => setTimeout(r, 0))
}

function overlay(calls: string[], presenting = true): OverlayActions {
  return {
    stop: () => {
      calls.push('stop')
      return presenting
    },
    open: () => false,
    reply: (id) => {
      calls.push(`reply ${id}`)
      return id === 't1'
    },
    voice: (on) => void calls.push(`voice ${on}`),
  }
}

beforeEach(() => {
  seen.toasts.length = 0
  seen.settings.length = 0
  seen.calls.length = 0
  seen.on = () => Promise.resolve({})
  seen.off = () => Promise.resolve({})
  seen.voiceOn = () => Promise.resolve('on')
  prefs().setPrefs({ show: true, voice: null, online: false })
})
afterEach(() => setOverlayActions(null))

describe('showing and hiding', () => {
  it('changes the preference of this browser, and nothing else', () => {
    setShow(false)
    expect(prefs().show).toBe(false)
    setShow(true)
    expect(prefs().show).toBe(true)
    toggleShow()
    expect(prefs().show).toBe(false)
    toggleShow()
    expect(prefs().show).toBe(true)
    // Only a plain yes shows them.
    setShow('yes' as unknown as boolean)
    expect(prefs().show).toBe(false)
    expect(seen.calls).toEqual([])
    expect(seen.toasts).toEqual([])
    expect(prefs()).toMatchObject({ voice: null, online: false })
  })
})

describe('voice on and off', () => {
  it('leaves it to a mounted overlay, which acts inside the click', () => {
    const calls: string[] = []
    setOverlayActions(overlay(calls))
    setVoice(true)
    setVoice(false)
    expect(calls).toEqual(['voice true', 'voice false'])
    // The overlay decides what turning it on needs: nothing was changed or looked at from here.
    expect(prefs().voice).toBeNull()
    expect(seen.calls).toEqual([])
  })

  it('turns it off as the preference alone where no overlay is mounted', () => {
    prefs().setPrefs({ voice: true })
    setVoice(false)
    expect(prefs().voice).toBe(false)
    expect(seen.calls).toEqual([])
  })

  it('looks at this browser’s voices before it turns it on where no overlay is mounted', async () => {
    setVoice(true)
    // Not on yet: what the browser can speak with decides, and that may need the user's word.
    expect(prefs().voice).toBeNull()
    await settled()
    expect(seen.calls).toEqual(['look at the voices'])
    // What comes of it is the looking's own business (voiceOn.ts): nothing more is set from here.
    expect(prefs().voice).toBeNull()
  })

  it('looks once at a time, however often it is asked meanwhile', async () => {
    let answer: (v: string) => void = () => {}
    seen.voiceOn = () => new Promise<string>((r) => (answer = r))
    setVoice(true)
    setVoice(true)
    await settled()
    setVoice(true)
    expect(seen.calls).toEqual(['look at the voices'])
    answer('declined')
    await settled()
    // Answered: the next click asks again.
    setVoice(true)
    await settled()
    expect(seen.calls).toEqual(['look at the voices', 'look at the voices'])
    answer('on')
    await settled()
  })

  it('still turns it on when the voices cannot be looked at, without allowing online voices', async () => {
    const errors = vi.spyOn(console, 'error').mockImplementation(() => {})
    try {
      seen.voiceOn = () => Promise.reject(new Error('the chunk did not load'))
      setVoice(true)
      await settled()
      expect(prefs()).toMatchObject({ voice: true, online: false })
      expect(errors).toHaveBeenCalledTimes(1)
    } finally {
      errors.mockRestore()
    }
  })

  it('toggles from where the preference is: not chosen counts as off', async () => {
    const calls: string[] = []
    setOverlayActions(overlay(calls))
    toggleVoice()
    prefs().setPrefs({ voice: true })
    toggleVoice()
    prefs().setPrefs({ voice: false })
    toggleVoice()
    expect(calls).toEqual(['voice true', 'voice false', 'voice true'])
  })
})

describe('stopping and replying', () => {
  it('asks the overlay, and has nothing to do without one', () => {
    expect(stopSpeaking()).toBe(false)
    expect(replyTo('t1')).toBe(false)
    const calls: string[] = []
    setOverlayActions(overlay(calls))
    expect(stopSpeaking()).toBe(true)
    expect(replyTo('t1')).toBe(true)
    expect(replyTo('t9')).toBe(false)
    expect(calls).toEqual(['stop', 'reply t1', 'reply t9'])
    // A tab that presents nothing stops nothing, and says so.
    setOverlayActions(overlay([], false))
    expect(stopSpeaking()).toBe(false)
    expect(seen.calls).toEqual([])
    expect(seen.toasts).toEqual([])
  })
})

describe('giving a session an avatar', () => {
  const session = { id: 't1', title: 'Fix the tests' }

  it('goes through the device route, and says nothing when the avatar simply appears', async () => {
    await giveAvatar(session)
    expect(seen.calls).toEqual(['on t1'])
    expect(seen.toasts).toEqual([])
    // Also when the server told the session at once.
    seen.on = () => Promise.resolve({ terminalId: 't1', briefed: true })
    await giveAvatar(session)
    expect(seen.toasts).toEqual([])
  })

  it('says when the session could not be told now, with the way to try again', async () => {
    seen.on = () => Promise.resolve({ terminalId: 't1', on: true, briefed: false })
    await giveAvatar(session)
    expect(seen.toasts).toHaveLength(1)
    expect(seen.toasts[0]).toMatchObject({ level: 'warning', message: 'Fix the tests has an avatar, but the session could not be told now. Workbench tells it when it is next idle.', timeout: 12_000 })
    expect(seen.toasts[0].action?.label).toBe('Tell it again')
    // The session is idle by now: the second try tells it, and nothing more is said.
    seen.on = () => Promise.resolve({ terminalId: 't1', on: true, briefed: true })
    seen.toasts[0].action?.run()
    await settled()
    expect(seen.calls).toEqual(['on t1', 'on t1'])
    expect(seen.toasts).toHaveLength(1)
  })

  it('says why nothing appears while avatars are hidden here, with the way to see it', async () => {
    setShow(false)
    await giveAvatar(session)
    expect(seen.toasts).toHaveLength(1)
    expect(seen.toasts[0]).toMatchObject({ level: 'info', message: 'Avatars are hidden in this browser: show them to see it.' })
    expect(seen.toasts[0].action?.label).toBe('Show avatars')
    seen.toasts[0].action?.run()
    expect(prefs().show).toBe(true)
  })

  it('shows a refusal with the server’s own sentence', async () => {
    seen.on = () => Promise.reject(new ApiError(403, 'forbidden', 'avatars are switched off in Settings'))
    await giveAvatar(session)
    expect(seen.toasts).toEqual([{ level: 'error', message: 'avatars are switched off in Settings', action: undefined, timeout: undefined }])
    // Something that is no error with a message: the app's own sentence.
    seen.toasts.length = 0
    seen.on = () => Promise.reject('offline')
    await giveAvatar(session)
    expect(seen.toasts.map((t) => t.message)).toEqual(['The avatar could not be changed.'])
  })
})

describe('turning a session’s avatar off', () => {
  it('goes through the device route and says nothing', async () => {
    await turnAvatarOff({ id: 't2' })
    expect(seen.calls).toEqual(['off t2'])
    expect(seen.toasts).toEqual([])
  })

  it('shows a refusal with the server’s own sentence', async () => {
    seen.off = () => Promise.reject(new ApiError(404, 'not_found', 'this session has no avatar'))
    await turnAvatarOff({ id: 't2' })
    expect(seen.toasts).toEqual([{ level: 'error', message: 'this session has no avatar', action: undefined, timeout: undefined }])
  })
})

describe('the settings', () => {
  it('opens the Avatar section', () => {
    openAvatarSettings()
    expect(seen.settings).toEqual(['avatar'])
  })
})
