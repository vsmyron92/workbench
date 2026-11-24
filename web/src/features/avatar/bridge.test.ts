// The bridge between the avatar feature and the agents feature (shell/avatarBridge.ts):
// which sessions have an avatar that is on, as the live record says, for the badge on
// the session card and header. (What the "finished" toast asks is tested with the agents
// feature's notifier.)

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { avatarOn, avatarShows, setAvatarShows, setAvatarsOn, useAvatarOn, watchAvatarsOn } from '@/shell/avatarBridge'

afterEach(() => {
  setAvatarsOn(null)
  setAvatarShows(null)
})

function Probe({ id }: { id: string }) {
  return createElement('i', null, String(useAvatarOn(id)))
}
const rendered = (id: string) => renderToStaticMarkup(createElement(Probe, { id }))

describe('setAvatarsOn and avatarOn', () => {
  it('knows nothing until the avatar feature told it', () => {
    expect(avatarOn('t1')).toBeNull()
    expect(rendered('t1')).toBe('<i>null</i>')
  })

  it('answers for every session once it was told: on, or not', () => {
    setAvatarsOn(new Set(['t1', 't2']))
    expect([avatarOn('t1'), avatarOn('t2'), avatarOn('t3')]).toEqual([true, true, false])
    expect(rendered('t1')).toBe('<i>true</i>')
    expect(rendered('t3')).toBe('<i>false</i>')
    // No avatar is on: that is an answer too, and not the same as not knowing.
    setAvatarsOn(new Set())
    expect(avatarOn('t1')).toBe(false)
    expect(rendered('t1')).toBe('<i>false</i>')
    setAvatarsOn(null)
    expect(avatarOn('t1')).toBeNull()
  })

  it('keeps its own copy: what the caller does with its set afterwards changes nothing', () => {
    const ids = new Set(['t1'])
    setAvatarsOn(ids)
    ids.add('t2')
    ids.delete('t1')
    expect([avatarOn('t1'), avatarOn('t2')]).toEqual([true, false])
  })

  it('takes only terminal ids, and what is no set as not knowing', () => {
    setAvatarsOn(new Set(['t1', 7, null] as unknown as string[]))
    expect([avatarOn('t1'), avatarOn('7')]).toEqual([true, false])
    setAvatarsOn('t1' as unknown as ReadonlySet<string>)
    expect(avatarOn('t1')).toBeNull()
    setAvatarsOn({} as unknown as ReadonlySet<string>)
    expect(avatarOn('t1')).toBeNull()
  })
})

describe('watchAvatarsOn (what useAvatarOn subscribes with)', () => {
  it('tells when the answer may have changed, and only then', () => {
    const seen: string[] = []
    const off = watchAvatarsOn(() => seen.push(String(avatarOn('t1'))))
    setAvatarsOn(new Set(['t1']))
    // The same ids again: nobody is told.
    setAvatarsOn(new Set(['t1']))
    expect(seen).toEqual(['true'])
    setAvatarsOn(new Set(['t1', 't2']))
    setAvatarsOn(new Set(['t2']))
    setAvatarsOn(null)
    setAvatarsOn(null)
    expect(seen).toEqual(['true', 'true', 'false', 'null'])
    off()
    setAvatarsOn(new Set(['t9']))
    expect(seen).toHaveLength(4)
  })

  it('does not let a watcher that throws keep the others from hearing', () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {})
    const heard: string[] = []
    const offBad = watchAvatarsOn(() => {
      throw new Error('a watcher failed')
    })
    const offGood = watchAvatarsOn(() => heard.push('told'))
    setAvatarsOn(new Set(['t1']))
    expect(heard).toEqual(['told'])
    expect(error).toHaveBeenCalledTimes(1)
    error.mockRestore()
    offBad()
    offGood()
  })

  it('lets a watcher leave while the others are told', () => {
    const heard: string[] = []
    const offFirst = watchAvatarsOn(() => {
      heard.push('first')
      offSecond()
    })
    const offSecond = watchAvatarsOn(() => heard.push('second'))
    setAvatarsOn(new Set(['t1']))
    // Both were told this once (they were watching when it changed); afterwards only the first.
    setAvatarsOn(new Set(['t2']))
    expect(heard).toEqual(['first', 'second', 'first'])
    offFirst()
  })
})

describe('setAvatarShows beside it', () => {
  it('still answers whether a session’s avatar is on screen', () => {
    expect(avatarShows('t1')).toBe(false)
    setAvatarShows((id) => id === 't1')
    expect([avatarShows('t1'), avatarShows('t2')]).toEqual([true, false])
  })
})
