import { afterEach, describe, expect, it, vi } from 'vitest'
import { api, ApiError } from './client'
import { experimentalNote, FEATURES, getHealth, loadHealth, osLabel, setHealth, unsupportedReason, type Health } from './health'

const base = { ok: true, service: 'workbench', version: '0.2.0', startedAt: 1 }
const LINUX: Health = { ...base, os: 'linux', unsupported: {}, experimental: {} }
// What a Windows server reports (app.rs `health`, util::os::support).
const WINDOWS: Health = {
  ...base,
  os: 'windows',
  unsupported: {
    devcontainer: "dev containers are not supported on Windows yet: Workbench cannot reach agents inside Docker Desktop's VM",
    desktopNotifications: 'desktop notifications are not supported on Windows yet',
    gdbAttach: 'attaching gdb to a running process is not supported on Windows yet: native programs attach with lldb-dap or CodeLLDB, Python with debugpy',
  },
  experimental: { services: 'the Services tool window is experimental on Windows: it has not been tested with Docker Desktop yet' },
}

const reply = (status: number, body: unknown) => vi.fn(async () => new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } }))

afterEach(() => {
  setHealth(null)
  vi.unstubAllGlobals()
})

describe('the health report', () => {
  it('marks nothing on Linux, or before it has loaded', () => {
    for (const h of [LINUX, null]) {
      for (const f of Object.values(FEATURES)) {
        expect(unsupportedReason(f, h)).toBeNull()
        expect(experimentalNote(f, h)).toBeNull()
      }
    }
  })

  it('names what Windows leaves out, as sentences', () => {
    expect(unsupportedReason(FEATURES.devcontainer, WINDOWS)).toMatch(/^Dev containers are not supported on Windows yet/)
    expect(unsupportedReason(FEATURES.gdbAttach, WINDOWS)).toMatch(/^Attaching gdb to a running process/)
    expect(unsupportedReason(FEATURES.services, WINDOWS)).toBeNull()
    expect(experimentalNote(FEATURES.services, WINDOWS)).toMatch(/^The Services tool window is experimental/)
    expect(experimentalNote(FEATURES.devcontainer, WINDOWS)).toBeNull()
    expect([osLabel('windows'), osLabel('macos'), osLabel('linux'), osLabel('freebsd'), osLabel(undefined)]).toEqual(['Windows', 'macOS', 'Linux', 'freebsd', null])
  })

  it('reads the loaded report by default', async () => {
    vi.stubGlobal('fetch', reply(200, WINDOWS))
    await loadHealth()
    expect(getHealth()?.os).toBe('windows')
    expect(unsupportedReason(FEATURES.devcontainer)).toMatch(/^Dev containers/)
    // Loaded once: a second call asks nothing.
    const again = reply(200, LINUX)
    vi.stubGlobal('fetch', again)
    await loadHealth()
    expect(again).not.toHaveBeenCalled()
  })

  it('treats an older server (no os, no lists) or a failed request as supporting everything', async () => {
    vi.stubGlobal('fetch', reply(200, base))
    await loadHealth()
    expect(unsupportedReason(FEATURES.devcontainer)).toBeNull()
    setHealth(null)
    vi.stubGlobal('fetch', reply(503, { error: { code: 'internal', message: 'down' } }))
    await loadHealth()
    expect(getHealth()).toBeNull()
  })
})

describe('unsupported_platform errors', () => {
  it('carry the feature they are about', async () => {
    vi.stubGlobal('fetch', reply(501, { error: { code: 'unsupported_platform', message: 'dev containers are not supported on Windows yet', feature: 'devcontainer' } }))
    const e = await api.get('/api/projects/shop/devcontainer').catch((x: unknown) => x)
    expect(e).toBeInstanceOf(ApiError)
    const err = e as ApiError
    expect([err.status, err.code, err.feature, err.unsupported, err.notConfigured]).toEqual([501, 'unsupported_platform', 'devcontainer', true, false])
    expect(err.message).toBe('dev containers are not supported on Windows yet')
  })

  it('leave other errors as they were', async () => {
    vi.stubGlobal('fetch', reply(412, { error: { code: 'not_configured', message: 'no token' } }))
    const err = (await api.get('/api/x').catch((x: unknown) => x)) as ApiError
    expect([err.code, err.feature, err.unsupported, err.notConfigured]).toEqual(['not_configured', undefined, false, true])
  })
})
