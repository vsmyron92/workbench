// The service worker (public/sw.js) answering a permission request from a
// notification's Allow / Deny: what each answer of Workbench's route leads to.
// The worker runs against stand-ins for its globals (no browser needed).

import { describe, expect, it } from 'vitest'
import swSource from '../../../public/sw.js?raw'

interface Shown {
  title: string
  options: NotificationOptions & { actions?: { action: string }[]; renotify?: boolean }
}

type Handler = (event: unknown) => void

function loadWorker(answer: () => Promise<Response>) {
  const handlers: Record<string, Handler> = {}
  const shown: Shown[] = []
  const opened: string[] = []
  const calls: { url: string; init: RequestInit }[] = []
  const self = {
    addEventListener: (type: string, fn: Handler) => {
      handlers[type] = fn
    },
    skipWaiting() {},
    navigator: { userAgent: 'Mozilla/5.0 (Linux; Android 14) Chrome/151.0 Mobile Safari/537.36' },
    clients: {
      claim: async () => {},
      matchAll: async () => [],
      openWindow: async (url: string) => {
        opened.push(url)
        return null
      },
    },
    registration: {
      showNotification: async (title: string, options: Shown['options']) => {
        shown.push({ title, options })
      },
      getNotifications: async () => [],
    },
  }
  const fetchStub = (url: string, init: RequestInit) => {
    calls.push({ url, init })
    return answer()
  }
  // No IndexedDB here: the worker then sends no device key (the server would refuse; not tested here).
  const indexedDB = {
    open() {
      throw new Error('unavailable')
    },
  }
  const immediate = (fn: () => void) => {
    fn()
    return 0
  }
  // oxlint-disable-next-line no-new-func -- the worker is a plain script with globals
  new Function('self', 'indexedDB', 'fetch', 'setTimeout', 'atob', swSource)(self, indexedDB, fetchStub, immediate, atob)
  return { handlers, shown, opened, calls }
}

const REQUEST = {
  title: 'shop · Fix the login',
  body: 'Needs your permission\nBash: npm test',
  tag: 'agent:t1',
  terminalId: 't1',
  permissionId: 'perm-1',
  tool: 'Bash',
  allow: true,
  projectId: 'shop',
  open: { kind: 'terminal', id: 'terminal:t1', params: { terminalId: 't1' } },
}

async function click(answer: () => Promise<Response>, action = 'allow', data: Record<string, unknown> = REQUEST) {
  const w = loadWorker(answer)
  let work: Promise<unknown> = Promise.resolve()
  w.handlers.notificationclick({
    action,
    notification: { data, close() {} },
    waitUntil(p: Promise<unknown>) {
      work = p
    },
  })
  await work
  return w
}

const status = (code: number) => () => Promise.resolve(new Response(code === 204 ? null : '{}', { status: code }))

describe('sw.js: Allow / Deny on a permission request', () => {
  it('posts the decision for the request to the terminals route', async () => {
    const w = await click(status(200), 'deny')
    expect(w.calls).toHaveLength(1)
    expect(w.calls[0].url).toBe('/api/agents/t1/permission')
    expect(w.calls[0].init.method).toBe('POST')
    expect(JSON.parse(String(w.calls[0].init.body))).toEqual({ id: 'perm-1', decision: 'deny' })
    expect(w.shown).toHaveLength(1)
    expect(w.shown[0].options.body).toBe('Denied · Bash')
    expect(w.shown[0].options.silent).toBe(true)
  })

  it('says a request Workbench can no longer answer is not pending, and opens the session on a tap', async () => {
    const w = await click(status(409))
    expect(w.shown).toHaveLength(1)
    const o = w.shown[0].options
    expect(o.body).toMatch(/^No longer pending here · Bash\n/)
    expect(o.body).toContain('only the terminal can answer it now')
    expect(o.body).not.toContain('Already answered')
    // It stays (no silent confirmation that clears itself) and offers no actions.
    expect(o.silent).toBeFalsy()
    expect(o.actions).toBeUndefined()
    expect(o.tag).toBe('agent:t1')
    expect((o.data as { open: unknown }).open).toEqual(REQUEST.open)
  })

  it('treats a session that is gone as final', async () => {
    const w = await click(status(404))
    expect(w.shown).toHaveLength(1)
    const o = w.shown[0].options
    expect(o.body).toMatch(/^This session is gone · Bash\n/)
    expect(o.actions).toBeUndefined()
    expect((o.data as { open: { kind: string } }).open.kind).toBe('agents.home')
  })

  it('does not offer Allow / Deny again for a refused answer', async () => {
    const w = await click(status(400))
    expect(w.shown[0].options.body).toMatch(/^Not answered · Bash\n.*HTTP 400/)
    expect(w.shown[0].options.actions).toBeUndefined()
  })

  it('offers the request again when Workbench may answer later', async () => {
    for (const answer of [status(503), status(429), () => Promise.reject(new TypeError('offline'))]) {
      const w = await click(answer)
      expect(w.shown).toHaveLength(1)
      const o = w.shown[0].options
      expect(o.actions?.map((a) => a.action)).toEqual(['allow', 'deny'])
      expect(o.body).toMatch(/Try again, or open Workbench\.\nNeeds your permission/)
    }
  })

  it('offers Allow only for a request the notification shows whole', async () => {
    const w = loadWorker(status(200))
    let work: Promise<unknown> = Promise.resolve()
    const push = (data: Record<string, unknown>) =>
      w.handlers.push({
        data: { json: () => data },
        waitUntil(p: Promise<unknown>) {
          work = p
        },
      })
    push(REQUEST)
    await work
    expect(w.shown[0].options.actions?.map((a) => a.action)).toEqual(['allow', 'deny'])
    expect(w.shown[0].options.requireInteraction).toBe(true)
    push({ ...REQUEST, allow: false })
    await work
    expect(w.shown[1].options.actions?.map((a) => a.action)).toEqual(['review', 'deny'])
    const { allow: _allow, ...older } = REQUEST
    push(older)
    await work
    expect(w.shown[2].options.actions?.map((a) => a.action)).toEqual(['review', 'deny'])
  })

  it('never allows a request it could not show whole: Review and a stray Allow open the session', async () => {
    for (const action of ['review', 'allow']) {
      const w = await click(status(200), action, { ...REQUEST, allow: false })
      expect(w.calls).toHaveLength(0)
      expect(w.opened).toHaveLength(1)
      expect(decodeURIComponent(w.opened[0])).toContain('"terminal:t1"')
    }
    // Deny needs no reading.
    const w = await click(status(200), 'deny', { ...REQUEST, allow: false })
    expect(JSON.parse(String(w.calls[0].init.body))).toEqual({ id: 'perm-1', decision: 'deny' })
  })

  it('opens Workbench to sign in when the device is signed out', async () => {
    const w = await click(status(401))
    expect(w.shown).toHaveLength(0)
    expect(w.opened).toHaveLength(1)
    expect(w.opened[0]).toMatch(/^\/\?open=/)
  })
})
