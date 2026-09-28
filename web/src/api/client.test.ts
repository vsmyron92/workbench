import { describe, expect, it } from 'vitest'
import { pickDeviceKey } from './client'

describe('device key handed over in the URL fragment', () => {
  const works = (ok: boolean) => async () => ok
  it('never replaces a key that still signs this browser in', async () => {
    // Any page (a report's popup, another site) can open `/#wbk=junk`.
    expect(await pickDeviceKey('good', 'junk', works(true))).toBe('good')
  })
  it('takes the new key after a fresh sign-in, or when none is stored', async () => {
    expect(await pickDeviceKey('stale', 'fresh', works(false))).toBe('fresh')
    expect(await pickDeviceKey(null, 'fresh', works(false))).toBe('fresh')
    expect(
      await pickDeviceKey('stale', 'fresh', async () => {
        throw new Error('offline')
      }),
    ).toBe('fresh')
  })
  it('keeps the stored key when nothing new was handed over', async () => {
    let asked = false
    const probe = async () => ((asked = true), true)
    expect(await pickDeviceKey('k', null, probe)).toBe('k')
    expect(await pickDeviceKey('k', 'k', probe)).toBe('k')
    expect(asked).toBe(false)
  })
})
