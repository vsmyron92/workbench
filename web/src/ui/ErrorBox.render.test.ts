// ErrorBox, rendered on the server: setup help for `not_configured`, the same box for a
// feature the server's OS leaves out (`unsupported_platform`), without Settings or Retry,
// and git's multi-line refusal of a repository with a button for its trust command.

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, describe, expect, it } from 'vitest'
import { ApiError } from '@/api/client'
import { setHealth } from '@/api/health'
import { ErrorBox, errorKind, trustCommand } from './index'

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

  it('keeps line breaks and copies the command that trusts a repository git refuses', () => {
    const refusal = [
      "fatal: detected dubious ownership in repository at 'C:/work/shop'",
      "'C:/work/shop' is owned by:",
      '\tBUILTIN/Administrators (S-1-5-32-544)',
      'but the current user is:',
      '\tPC/me (S-1-5-21-1-2-3-1001)',
      'To add an exception for this directory, call:',
      '',
      '\tgit config --global --add safe.directory C:/work/shop',
    ].join('\n')
    const html = render(new ApiError(403, 'unsafe_repository', refusal))
    expect(html).toContain('class="wb-small wb-error-message"')
    expect(html).toContain('is owned by:\n\tBUILTIN/Administrators')
    expect(html).toContain('Copy command')
    expect(html).toContain('title="git config --global --add safe.directory C:/work/shop"')
    expect(html).toContain('Retry')
    // Only for that error: the same text under another code offers no command.
    expect(render(new ApiError(422, 'git_error', refusal))).not.toContain('Copy command')
    expect(render(new ApiError(500, 'internal', 'boom'))).not.toContain('Copy command')
  })

  it("finds git's trust command, quoted or not", () => {
    const quoted = "fatal: detected dubious ownership in repository at '/srv/my repo'\nTo add an exception for this directory, call:\n\n\tgit config --global --add safe.directory '/srv/my repo'\n"
    expect(trustCommand(quoted)).toBe("git config --global --add safe.directory '/srv/my repo'")
    expect(trustCommand('fatal: not a git repository')).toBeNull()
  })
})
