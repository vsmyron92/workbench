import { describe, expect, it } from 'vitest'
import type { BrowserVoice, TestVoiceResult } from '@/features/avatar/voices'
import {
  AVATAR_CHOICES,
  MAX_VOICE_OPTIONS,
  NO_VOICES,
  ONLINE_VOICES,
  avatarChoice,
  avatarPatch,
  choiceHint,
  countVoices,
  offersOnline,
  speedText,
  testText,
  voiceOptions,
  voiceStatus,
  voicesText,
  volumeText,
  type AvatarChoice,
} from './avatarLib'
import type { GlobalConfig } from './types'

const voice = (name: string, onDevice: boolean, lang = 'en-GB'): BrowserVoice => ({ name, lang, onDevice, default: false })

describe('the Avatar section’s choice', () => {
  it('offers the three choices in the contract’s words', () => {
    expect(AVATAR_CHOICES.map((c) => [c.value, c.label])).toEqual([
      ['off', 'Off'],
      ['asked', 'When I give them one'],
      ['any', 'Whenever they want'],
    ])
    // Each choice says what it means for a session, in a sentence of its own.
    const hints = AVATAR_CHOICES.map((c) => choiceHint(c.value))
    expect(new Set(hints).size).toBe(3)
    for (const hint of hints) expect(hint).toMatch(/^[A-Z].*\.$/)
    expect(choiceHint('off')).toContain('No session has an avatar')
    expect(choiceHint('asked')).toContain('started with the Avatar option')
    // Switching to it takes no avatar away: each is turned off for itself.
    expect(choiceHint('asked')).toContain('Sessions that have one already keep it')
    expect(choiceHint('any')).toContain('the first time it uses one')
    expect(choiceHint('sometimes' as AvatarChoice)).toBe('')
  })

  it('reads the choice from what config.toml holds: nothing written is "whenever they want"', () => {
    const of = (avatar: GlobalConfig['avatar']) => avatarChoice(avatar)
    // The default config holds no [avatar] at all.
    expect(of(undefined)).toBe('any')
    expect(of(null)).toBe('any')
    expect(of({})).toBe('any')
    expect(of({ enabled: null, sessions: null })).toBe('any')
    expect(of({ sessions: 'any' })).toBe('any')
    expect(of({ enabled: true })).toBe('any')
    expect(of({ sessions: 'asked' })).toBe('asked')
    expect(of({ enabled: true, sessions: 'asked' })).toBe('asked')
    // Switched off decides alone, whatever a hand-edited file says about sessions.
    expect(of({ enabled: false })).toBe('off')
    expect(of({ enabled: false, sessions: 'any' })).toBe('off')
    expect(of({ enabled: false, sessions: 'asked' })).toBe('off')
    // Only the server's own values count.
    expect(of({ sessions: 'ASKED' } as unknown as GlobalConfig['avatar'])).toBe('any')
    expect(of({ enabled: 'false' } as unknown as GlobalConfig['avatar'])).toBe('any')
  })

  it('sends [avatar] enabled and sessions for each choice, null for a default so nothing is written for it', () => {
    expect(avatarPatch('off')).toEqual({ enabled: false, sessions: null })
    expect(avatarPatch('asked')).toEqual({ enabled: null, sessions: 'asked' })
    expect(avatarPatch('any')).toEqual({ enabled: null, sessions: null })
    // Both keys every time: a key left out would keep what the file holds (the server merges field by field).
    for (const c of AVATAR_CHOICES) expect(Object.keys(avatarPatch(c.value)).sort()).toEqual(['enabled', 'sessions'])
    // Never `enabled: true` and never `sessions: "any"`: a default is not pinned in the file.
    for (const c of AVATAR_CHOICES) {
      expect(avatarPatch(c.value).enabled).not.toBe(true)
      expect(avatarPatch(c.value).sessions).not.toBe('any')
    }
    // Anything else is the default: nothing is written.
    expect(avatarPatch('sometimes' as AvatarChoice)).toEqual({ enabled: null, sessions: null })
  })

  it('reads back the choice it sent, from whatever the file held before', () => {
    /** The server's merge: the section's fields one by one, a key that is null is not written. */
    const merged = (held: GlobalConfig['avatar'], patch: ReturnType<typeof avatarPatch>): GlobalConfig['avatar'] => {
      const next: Record<string, unknown> = { ...held, ...patch }
      for (const key of Object.keys(next)) if (next[key] === null) delete next[key]
      return next as GlobalConfig['avatar']
    }
    const before: GlobalConfig['avatar'][] = [undefined, {}, { enabled: false }, { sessions: 'any' }, { enabled: false, sessions: 'any' }, { enabled: true, sessions: 'asked' }]
    for (const held of before) {
      for (const c of AVATAR_CHOICES) {
        const after = merged(held, avatarPatch(c.value))
        expect(avatarChoice(after), `${JSON.stringify(held)} -> ${c.value}`).toBe(c.value)
      }
      // The default choice leaves an empty section, which the server does not write.
      expect(merged(held, avatarPatch('any'))).toEqual({})
      expect(merged(held, avatarPatch('off'))).toEqual({ enabled: false })
      expect(merged(held, avatarPatch('asked'))).toEqual({ sessions: 'asked' })
    }
  })
})

describe('this browser’s voices, for the Avatar section', () => {
  it('counts the voices on the device and the online ones', () => {
    expect(countVoices([])).toEqual({ device: 0, online: 0 })
    expect(countVoices([voice('a', true), voice('b', false), voice('c', false)])).toEqual({ device: 1, online: 2 })
    // Only a plain yes is "on this device": anything else would send what is said away.
    expect(countVoices([{ onDevice: 'yes' as unknown as boolean }, { onDevice: undefined as unknown as boolean }])).toEqual({ device: 0, online: 2 })
    expect(countVoices(null as unknown as BrowserVoice[])).toEqual({ device: 0, online: 0 })
    expect(countVoices([null as unknown as BrowserVoice, voice('a', true)])).toEqual({ device: 1, online: 0 })
  })

  it('says what the browser can speak with', () => {
    expect(voicesText({ device: 0, online: 0 })).toBe('This browser has no voices: avatars show captions.')
    expect(voicesText({ device: 3, online: 0 })).toBe('This browser has 3 voices on this device and no online voices.')
    expect(voicesText({ device: 1, online: 0 })).toBe('This browser has 1 voice on this device and no online voices.')
    expect(voicesText({ device: 0, online: 19 })).toBe('This browser has 19 online voices and none on this device.')
    expect(voicesText({ device: 0, online: 1 })).toBe('This browser has 1 online voice and none on this device.')
    expect(voicesText({ device: 2, online: 19 })).toBe('This browser has 2 voices on this device and 19 online voices.')
    expect(voicesText({ device: 0, online: 0 })).toBe(NO_VOICES)
  })

  it('says what the voice choice comes to, and warns only when voice is on and nothing can be heard', () => {
    const some = { device: 2, online: 5 }
    const onlineOnly = { device: 0, online: 5 }
    const none = { device: 0, online: 0 }
    // Not chosen, and off: plain facts, whatever the browser has.
    for (const c of [some, onlineOnly, none]) {
      expect(voiceStatus({ voice: null, online: false }, c)).toEqual({ tone: 'muted', text: 'Not chosen yet: an avatar offers the choice under its first line.' })
      expect(voiceStatus({ voice: false, online: true }, c)).toEqual({ tone: 'muted', text: 'Captions only.' })
    }
    // On, and something can say it.
    expect(voiceStatus({ voice: true, online: false }, some)).toEqual({ tone: 'muted', text: 'Lines are said aloud.' })
    expect(voiceStatus({ voice: true, online: true }, onlineOnly)).toEqual({ tone: 'muted', text: 'Lines are said aloud.' })
    expect(voiceStatus({ voice: true, online: false }, { device: 1, online: 0 })).toEqual({ tone: 'muted', text: 'Lines are said aloud.' })
    // On, but nothing can: say why.
    expect(voiceStatus({ voice: true, online: false }, onlineOnly)).toEqual({ tone: 'warning', text: 'This browser has only online voices: avatars show captions until online voices are allowed.' })
    expect(voiceStatus({ voice: true, online: false }, none)).toEqual({ tone: 'warning', text: NO_VOICES })
    expect(voiceStatus({ voice: true, online: true }, none)).toEqual({ tone: 'warning', text: NO_VOICES })
  })

  it('offers online voices on the phone only where voice needs them, or where they can be withdrawn', () => {
    expect(offersOnline({ online: false }, { device: 2, online: 5 })).toBe(false)
    expect(offersOnline({ online: false }, { device: 2, online: 0 })).toBe(false)
    expect(offersOnline({ online: false }, { device: 0, online: 0 })).toBe(false)
    expect(offersOnline({ online: false }, { device: 0, online: 5 })).toBe(true)
    // Allowed once: the switch stays, so the permission can be taken back.
    expect(offersOnline({ online: true }, { device: 2, online: 5 })).toBe(true)
    expect(offersOnline({ online: true }, { device: 0, online: 0 })).toBe(true)
  })

  it('says the sentence of rule 7 as the avatar says it', () => {
    expect(ONLINE_VOICES).toBe('What an avatar says is sent to the browser maker’s speech service to be spoken.')
  })

  it('lists the voices for the test, each marked, the ones on this device first, each name once', () => {
    const list = [voice('Google UK English Female', false), voice('Daniel', true), voice('Google Deutsch', false, 'de-DE'), voice('Anna', true, 'de-DE'), voice('Daniel', true, 'en-AU')]
    expect(voiceOptions(list)).toEqual([
      { value: 'Daniel', label: 'Daniel · en-GB · on this device' },
      { value: 'Anna', label: 'Anna · de-DE · on this device' },
      { value: 'Google UK English Female', label: 'Google UK English Female · en-GB · online' },
      { value: 'Google Deutsch', label: 'Google Deutsch · de-DE · online' },
    ])
    expect(voiceOptions([])).toEqual([])
    expect(voiceOptions(null as unknown as BrowserVoice[])).toEqual([])
    // A voice without a language, or one that is no voice at all.
    expect(voiceOptions([{ name: 'Plain', lang: '', onDevice: true, default: true }, { name: '', lang: 'en', onDevice: true, default: false }, null as unknown as BrowserVoice])).toEqual([{ value: 'Plain', label: 'Plain · on this device' }])
    // Only a plain yes is marked "on this device".
    expect(voiceOptions([{ name: 'X', lang: 'en', onDevice: 1 as unknown as boolean, default: false }])).toEqual([{ value: 'X', label: 'X · en · online' }])
    // Bounded, however long the browser's list is.
    const many = Array.from({ length: MAX_VOICE_OPTIONS + 40 }, (_, i) => voice(`v${i}`, i % 2 === 0))
    expect(voiceOptions(many)).toHaveLength(MAX_VOICE_OPTIONS)
  })

  it('says what became of a voice test, in one sentence for each outcome', () => {
    const all: TestVoiceResult[] = ['spoken', 'no-speech', 'no-click', 'no-voices', 'online-only', 'listening', 'busy']
    const texts = all.map((r) => testText(r).text)
    expect(new Set(texts).size).toBe(all.length)
    for (const text of texts) expect(text).toMatch(/^[A-Z][^!]*\.$/)
    expect(testText('spoken')).toEqual({ tone: 'muted', text: 'The test sentence is being said.' })
    expect(testText('online-only')).toEqual({ tone: 'warning', text: 'Only an online voice could say it: allow online voices first.' })
    expect(testText('no-speech').tone).toBe('warning')
    expect(testText('no-voices').tone).toBe('warning')
    expect(testText('listening')).toEqual({ tone: 'muted', text: 'Nothing is said while a dictation is listening.' })
    expect(testText('busy').tone).toBe('muted')
    // A word this section does not know (a newer avatar slice): still a sentence, never the raw word.
    expect(testText('later' as TestVoiceResult)).toEqual({ tone: 'warning', text: 'The test sentence could not be said.' })
    expect(testText('constructor' as TestVoiceResult).text).toBe('The test sentence could not be said.')
  })

  it('writes speed as a factor and volume as a percentage', () => {
    expect([speedText(1), speedText(0.7), speedText(1.25), speedText(1.5), speedText(1.2000000001)]).toEqual(['1×', '0.7×', '1.25×', '1.5×', '1.2×'])
    expect([volumeText(1), volumeText(0), volumeText(0.8), volumeText(0.05), volumeText(0.333)]).toEqual(['100%', '0%', '80%', '5%', '33%'])
    // Whatever storage held: never NaN on screen.
    expect(speedText(Number.NaN)).toBe('1×')
    expect(volumeText(Number.NaN)).toBe('100%')
    expect(volumeText(7)).toBe('100%')
    expect(volumeText(-1)).toBe('0%')
  })
})
