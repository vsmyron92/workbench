import { describe, expect, it } from 'vitest'
import { b64urlBytes, bytesB64url, isIos, launchParams, parseTarget, pushSupport, syncAction, type SupportEnv, type SyncState } from './pushLib'

const env = (p: Partial<SupportEnv> = {}): SupportEnv => ({
  secure: true,
  serviceWorker: true,
  pushManager: true,
  notification: true,
  userAgent: 'Mozilla/5.0 (Linux; Android 14; Pixel 8) Chrome/151.0 Mobile Safari/537.36',
  standalone: false,
  ...p,
})

describe('pushSupport', () => {
  it('needs https first', () => {
    const s = pushSupport(env({ secure: false }))
    expect(s.ok).toBe(false)
    expect(s.ok === false && s.reason).toBe('insecure')
  })
  it('sends iPhones to the home screen app', () => {
    const ua = 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1'
    expect(isIos(ua)).toBe(true)
    const s = pushSupport(env({ userAgent: ua, pushManager: false }))
    expect(s.ok === false && s.reason).toBe('ios-browser')
    expect(pushSupport(env({ userAgent: ua, standalone: true })).ok).toBe(true)
    // iPadOS asks for the desktop site but still says Mobile/.
    expect(isIos('Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1')).toBe(true)
    expect(isIos('Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15')).toBe(false)
  })
  it('reports browsers without push', () => {
    const s = pushSupport(env({ pushManager: false }))
    expect(s.ok === false && s.reason).toBe('unsupported')
    expect(pushSupport(env()).ok).toBe(true)
  })
})

describe('syncAction', () => {
  const base: SyncState = {
    permission: 'granted',
    browserSub: true,
    browserKeyMatches: true,
    serverHas: true,
    endpointMatches: true,
    wanted: true,
    keyAtSubscribe: 'K1',
    serverKey: 'K1',
  }
  it('leaves a consistent device alone', () => {
    expect(syncAction(base)).toBe('none')
    expect(syncAction({ ...base, browserSub: false, serverHas: false, wanted: false, keyAtSubscribe: null })).toBe('none')
  })
  it('re-registers a rotated endpoint', () => {
    expect(syncAction({ ...base, endpointMatches: false })).toBe('register')
  })
  it('subscribes again for a new server key or a lost browser subscription', () => {
    expect(syncAction({ ...base, browserKeyMatches: false })).toBe('resubscribe')
    expect(syncAction({ ...base, browserSub: false, browserKeyMatches: false, endpointMatches: false })).toBe('resubscribe')
    // The server's data was reset: new key, no record; this browser had push on.
    expect(syncAction({ ...base, serverHas: false, browserKeyMatches: false, serverKey: 'K2' })).toBe('resubscribe')
  })
  it('turns off here what was removed elsewhere', () => {
    expect(syncAction({ ...base, serverHas: false })).toBe('drop')
  })
  it('forgets a record the browser can no longer use', () => {
    expect(syncAction({ ...base, permission: 'denied' })).toBe('forget')
    expect(syncAction({ ...base, permission: 'default', serverHas: false })).toBe('none')
  })
})

describe('keys', () => {
  it('round-trips base64url', () => {
    const bytes = new Uint8Array([4, 255, 0, 62, 63, 128, 7])
    const s = bytesB64url(bytes)
    expect(s).not.toMatch(/[+/=]/)
    expect([...b64urlBytes(s)]).toEqual([...bytes])
    expect(bytesB64url(null)).toBe('')
  })
})

describe('targets and launch parameters', () => {
  it('accepts only known panel kinds and sane project ids', () => {
    expect(parseTarget({ kind: 'terminal', id: 'terminal:t1', params: { terminalId: 't1' }, projectId: 'shop' })).toEqual({
      kind: 'terminal',
      id: 'terminal:t1',
      params: { terminalId: 't1' },
      projectId: 'shop',
    })
    expect(parseTarget({ kind: 'app', params: { url: 'https://evil.example' } })).toBeNull()
    expect(parseTarget({ kind: 'app', projectId: 'shop' })).toEqual({ projectId: 'shop' })
    expect(parseTarget({ projectId: '../x y' })).toBeNull()
    expect(parseTarget('not json')).toBeNull()
    expect(parseTarget('x'.repeat(3000))).toBeNull()
    expect(parseTarget([1, 2])).toBeNull()
  })
  it('takes the shortcut tab and the notification target out of the URL', () => {
    const open = encodeURIComponent(JSON.stringify({ kind: 'terminal', id: 'terminal:t1', params: { terminalId: 't1' } }))
    const l = launchParams(`https://box.ts.net/?mobile&tab=agents&open=${open}`)
    expect(l.tab).toBe('agents')
    expect(l.open?.kind).toBe('terminal')
    expect(l.cleaned).toBe('/?mobile=')
    expect(launchParams('https://box.ts.net/?tab=secrets').tab).toBeNull()
    expect(launchParams('https://box.ts.net/').cleaned).toBeNull()
  })
})
