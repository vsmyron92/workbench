import { beforeEach, describe, expect, it, vi } from 'vitest'

// The event socket and the server are stand-ins: `connection(true)` is a reconnect, and
// `server.version` is what /api/health answers then.
const h = vi.hoisted(() => ({
  listeners: new Set<(up: boolean) => void>(),
  server: { version: '0.5.3' as string | null, startedAt: 1000 },
  loaded: '0.5.3' as string | null,
  posts: [] as { path: string; body: unknown }[],
  confirm: true,
  toasts: [] as { message: string; action?: { label: string; run: () => void }; timeout?: number }[],
}))

vi.mock('@/api/events', () => ({
  onEventsConnection: (fn: (up: boolean) => void) => {
    h.listeners.add(fn)
    return () => h.listeners.delete(fn)
  },
}))
vi.mock('@/api/health', () => ({
  getHealth: () => (h.loaded ? { version: h.loaded, startedAt: 1000 } : null),
  refreshHealth: async () => (h.server.version ? { version: h.server.version, startedAt: h.server.startedAt } : null),
}))
vi.mock('@/api/client', () => ({
  api: {
    post: async (path: string, body?: unknown) => {
      h.posts.push({ path, body })
      return { phase: 'downloading' }
    },
  },
}))
vi.mock('@/shell/actions', () => ({
  confirmDialog: async () => h.confirm,
  toast: (_level: string, message: string, opts: { action?: { label: string; run: () => void }; timeout?: number } = {}) => {
    h.toasts.push({ message, ...opts })
    return h.toasts.length
  },
  toastError: () => {},
}))

import { QueryClient } from '@tanstack/react-query'
import { installUpdate, restartWorkbench, watchServerVersion } from './update'

/** The server process ended and another one answers, as `version`. */
const restartServer = (version: string) => {
  h.server.version = version
  h.server.startedAt += 1000
}

const reconnect = async () => {
  h.listeners.forEach((fn) => fn(true))
  await new Promise((r) => setTimeout(r, 0))
}

describe('watchServerVersion', () => {
  beforeEach(() => {
    h.listeners.clear()
    h.server.version = '0.5.3'
    h.server.startedAt = 1000
    h.loaded = '0.5.3'
    h.posts.length = 0
    h.toasts.length = 0
    h.confirm = true
  })

  it('does nothing while the server stays the version the page came from', async () => {
    const reload = vi.fn()
    const stop = watchServerVersion(reload)
    await reconnect()
    h.listeners.forEach((fn) => fn(false))
    await reconnect()
    expect(reload).not.toHaveBeenCalled()
    expect(h.toasts).toEqual([])
    stop()
    expect(h.listeners.size).toBe(0)
  })

  it('offers a reload once when another device updated the server', async () => {
    const reload = vi.fn()
    watchServerVersion(reload)
    restartServer('0.6.0')
    await reconnect()
    await reconnect()
    expect(reload).not.toHaveBeenCalled()
    expect(h.toasts).toHaveLength(1)
    expect(h.toasts[0]).toMatchObject({ message: 'Workbench was updated to 0.6.0', timeout: 0 })
    h.toasts[0].action?.run()
    expect(reload).toHaveBeenCalledTimes(1)
  })

  it('reloads the tab that asked for the update, and only after it confirmed', async () => {
    const qc = new QueryClient()
    const reload = vi.fn()
    watchServerVersion(reload)

    h.confirm = false
    await installUpdate(qc, '0.6.0')
    expect(h.posts).toEqual([])

    h.confirm = true
    await installUpdate(qc, '0.6.0')
    expect(h.posts).toEqual([{ path: '/api/platform/update/install', body: { version: '0.6.0', restart: true } }])
    expect(qc.getQueryData(['platform', 'update'])).toEqual({ phase: 'downloading' })

    // The socket drops while the download still runs (the same process answers), the
    // server is away, then the new version answers.
    await reconnect()
    h.server.version = null
    await reconnect()
    expect(reload).not.toHaveBeenCalled()
    restartServer('0.6.0')
    await reconnect()
    expect(reload).toHaveBeenCalledTimes(1)
    expect(h.toasts).toEqual([])
  })

  it('a plain restart reloads nothing: the version did not change', async () => {
    const qc = new QueryClient()
    const reload = vi.fn()
    watchServerVersion(reload)
    await restartWorkbench(qc)
    expect(h.posts).toEqual([{ path: '/api/platform/restart', body: undefined }])
    restartServer('0.5.3')
    await reconnect()
    expect(reload).not.toHaveBeenCalled()
    // Its "asked for a restart" is used up: a later update by someone else only offers.
    restartServer('0.6.0')
    await reconnect()
    expect(reload).not.toHaveBeenCalled()
    expect(h.toasts).toHaveLength(1)
  })

  it('learns the version on the first connection when the page had none yet', async () => {
    h.loaded = null
    const reload = vi.fn()
    watchServerVersion(reload)
    await reconnect()
    restartServer('0.6.0')
    await reconnect()
    expect(h.toasts).toHaveLength(1)
  })
})
