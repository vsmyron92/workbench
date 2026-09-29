// ErrorBox, rendered on the server: setup help for `not_configured`, the same box for a
// feature the server's OS leaves out (`unsupported_platform`), without Settings or Retry.

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, describe, expect, it } from 'vitest'
import { ApiError } from '@/api/client'
import { setHealth } from '@/api/health'
import { ErrorBox, errorKind } from './index'

const retry = () => {}
const render = (error: unknown) => renderToStaticMarkup(createElement(ErrorBox, { error, onRetry: retry }))
const unsupported = new ApiError(501, 'unsupported_platform', "dev containers are not supported on Windows yet: Workbench cannot reach agents inside Docker Desktop's VM", 'devcontainer')

afterEach(() => setHealth(null))

describe('ErrorBox', () => {
  it('tells errors, setup help and unsupported features apart', () => {
    expect(errorKind(unsupported)).toBe('unsupported')
    expect(errorKind(new ApiError(412, 'not_configured', 'no token'))).toBe('setup')
    expect(errorKind(new ApiError(500, 'internal', 'boom'))).toBe('error')
    expect(errorKind(new Error('boom'))).toBe('error')
  })

  it('shows an unsupported feature like setup help, with the reason and nothing to retry', () => {
    setHealth({ ok: true, service: 'workbench', version: '0.2.0', startedAt: 1, os: 'windows', unsupported: {}, experimental: {} })
    const html = render(unsupported)
    expect(html).toContain('class="wb-error setup"')
    expect(html).toContain('Not available on Windows')
    expect(html).toContain('cannot reach agents inside Docker Desktop')
    expect(html).not.toContain('Retry')
    expect(html).not.toContain('Open Settings')
    // Before the health report arrives the OS is not named.
    setHealth(null)
    expect(render(unsupported)).toContain('Not available on this system')
  })

  it('keeps setup help and failures as they were', () => {
    const setup = render(new ApiError(412, 'not_configured', 'no token'))
    expect(setup).toContain('class="wb-error setup"')
    expect(setup).toContain('Not set up yet')
    expect(setup).toContain('Retry')
    const failed = render(new ApiError(500, 'internal', 'boom'))
    expect(failed).toContain('class="wb-error"')
    expect(failed).toContain('Something went wrong')
    expect(failed).toContain('Retry')
  })
})
