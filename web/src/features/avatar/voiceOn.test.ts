// Turning voice on where no overlay is mounted: what this browser can speak with decides,
// online voices are used only after a yes, and the list of voices is waited for.

import { describe, expect, it, vi } from 'vitest'
import { VOICES_WAIT_MS, listedVoices, turnVoiceOn, type VoiceOnHost } from './voiceOn'

const device = (lang = 'en-GB') => ({ lang, onDevice: true })
const online = (lang = 'en-US') => ({ lang, onDevice: false })

function host(voices: { lang: string; onDevice: boolean }[], opts: { online?: boolean; answer?: boolean } = {}) {
  const log: string[] = []
  const h: VoiceOnHost = {
    voices: () => Promise.resolve(voices),
    online: () => opts.online === true,
    confirmOnline: () => {
      log.push('asked')
      return Promise.resolve(opts.answer === true)
    },
    tellNone: () => void log.push('told: no voices'),
    set: (patch) => void log.push(`set ${JSON.stringify(patch)}`),
  }
  return { h, log }
}

describe('turnVoiceOn', () => {
  it('turns voice on at once where the browser has a voice on the device', async () => {
    const { h, log } = host([online(), device()])
    expect(await turnVoiceOn(h)).toBe('on')
    expect(log).toEqual(['set {"voice":true}'])
  })

  it('asks before online voices are used, and turns nothing on without a yes', async () => {
    const no = host([online(), online('en-GB')], { answer: false })
    expect(await turnVoiceOn(no.h)).toBe('declined')
    expect(no.log).toEqual(['asked'])
    const yes = host([online()], { answer: true })
    expect(await turnVoiceOn(yes.h)).toBe('on')
    // The permission and the choice are set together, after the answer.
    expect(yes.log).toEqual(['asked', 'set {"online":true,"voice":true}'])
  })

  it('takes only a plain yes for an answer', async () => {
    for (const answer of [undefined, null, 'yes', 1]) {
      const { h, log } = host([online()])
      h.confirmOnline = () => Promise.resolve(answer as unknown as boolean)
      expect(await turnVoiceOn(h)).toBe('declined')
      expect(log).toEqual([])
    }
  })

  it('does not ask again where online voices are allowed already', async () => {
    const { h, log } = host([online()], { online: true })
    expect(await turnVoiceOn(h)).toBe('on')
    expect(log).toEqual(['set {"voice":true}'])
  })

  it('says that the browser has no voices, and turns nothing on', async () => {
    const { h, log } = host([])
    expect(await turnVoiceOn(h)).toBe('none')
    expect(log).toEqual(['told: no voices'])
  })

  it('looks at the voices of the language avatars speak: English unless told otherwise', async () => {
    // Only a German voice on the device: English lines would need the online voice.
    const mixed = [device('de-DE'), online('en-US')]
    const english = host(mixed, { answer: false })
    expect(await turnVoiceOn(english.h)).toBe('declined')
    expect(english.log).toEqual(['asked'])
    const german = host(mixed)
    expect(await turnVoiceOn(german.h, 'de')).toBe('on')
    expect(german.log).toEqual(['set {"voice":true}'])
    // No voice of the language at all.
    const none = host([device('de-DE')])
    expect(await turnVoiceOn(none.h, 'fr-FR')).toBe('none')
    expect(none.log).toEqual(['told: no voices'])
  })
})

describe('listedVoices', () => {
  /** A browser's list as an external store, and timers the test runs by hand. */
  function browser(first: { lang: string; onDevice: boolean }[] = []) {
    let list = first
    const watchers = new Set<() => void>()
    const timers = new Map<number, { cb: () => void; ms: number }>()
    let nextTimer = 1
    return {
      source: {
        read: () => list,
        subscribe: (changed: () => void) => {
          watchers.add(changed)
          return () => void watchers.delete(changed)
        },
      },
      timers: {
        setTimeout: (cb: () => void, ms: number) => {
          timers.set(nextTimer, { cb, ms })
          return nextTimer++
        },
        clearTimeout: (id: number) => void timers.delete(id),
      },
      load: (next: { lang: string; onDevice: boolean }[]) => {
        list = next
        for (const w of Array.from(watchers)) w()
      },
      tell: () => {
        for (const w of Array.from(watchers)) w()
      },
      expire: () => {
        for (const [id, t] of Array.from(timers)) {
          timers.delete(id)
          t.cb()
        }
      },
      watching: () => watchers.size,
      pending: () => Array.from(timers.values()).map((t) => t.ms),
    }
  }

  it('answers at once with a list that is there, and listens for nothing', async () => {
    const b = browser([device()])
    expect(await listedVoices(b.source, b.timers)).toEqual([device()])
    expect(b.watching()).toBe(0)
    expect(b.pending()).toEqual([])
  })

  it('waits for the browser to list its voices, and stops listening once it has', async () => {
    const b = browser()
    const got = vi.fn()
    void listedVoices(b.source, b.timers).then(got)
    await Promise.resolve()
    expect(got).not.toHaveBeenCalled()
    expect(b.watching()).toBe(1)
    expect(b.pending()).toEqual([VOICES_WAIT_MS])
    // Told of a change that brought nothing: still waiting.
    b.tell()
    await Promise.resolve()
    expect(got).not.toHaveBeenCalled()
    b.load([online()])
    await Promise.resolve()
    expect(got).toHaveBeenCalledWith([online()])
    expect(b.watching()).toBe(0)
    expect(b.pending()).toEqual([])
    // A later change is nobody's business any more.
    b.load([device()])
    await Promise.resolve()
    expect(got).toHaveBeenCalledTimes(1)
  })

  it('gives up after the wait and takes the list as it is: a browser without voices', async () => {
    const b = browser()
    const got = vi.fn()
    void listedVoices(b.source, b.timers, 500).then(got)
    expect(b.pending()).toEqual([500])
    b.expire()
    await Promise.resolve()
    expect(got).toHaveBeenCalledWith([])
    expect(b.watching()).toBe(0)
    // Voices that arrive after that are not waited for by anybody.
    b.load([device()])
    await Promise.resolve()
    expect(got).toHaveBeenCalledTimes(1)
  })

  it('takes a list that the first look made the browser fill', async () => {
    // A source whose list is there by the second look, without telling anybody.
    let looks = 0
    const source = { read: () => (++looks >= 2 ? [device()] : []), subscribe: () => () => {} }
    const b = browser()
    expect(await listedVoices(source, b.timers)).toEqual([device()])
    expect(b.pending()).toEqual([])
  })

  it('stops listening to a source that tells while it is subscribed to', async () => {
    let list: { lang: string; onDevice: boolean }[] = []
    let stopped = 0
    const source = {
      read: () => list,
      subscribe: (changed: () => void) => {
        list = [device()]
        changed()
        return () => void stopped++
      },
    }
    const b = browser()
    expect(await listedVoices(source, b.timers)).toEqual([device()])
    expect(stopped).toBe(1)
    expect(b.pending()).toEqual([])
  })
})
