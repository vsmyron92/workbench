// The status bar's git item, rendered on the server: a repository git refuses shows a
// warning where the branch would be, instead of nothing.

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { describe, expect, it } from 'vitest'
import { ApiError } from '@/api/client'
import { gk } from './api'
import { GitStatusbarWidget } from './Widgets'

/** The widget with git status failing with `error` (kept: no new fetch on mount). */
function statusbar(error: ApiError): string {
  const qc = new QueryClient({ defaultOptions: { queries: { retryOnMount: false } } })
  qc.getQueryCache().build(qc, { queryKey: gk.status('shop') }).setState({ status: 'error', error, fetchStatus: 'idle' })
  return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc }, createElement(GitStatusbarWidget, { projectId: 'shop' })))
}

describe('git status bar item', () => {
  it('warns about a repository git refuses, with git’s message as its tooltip', () => {
    const refusal = "fatal: detected dubious ownership in repository at 'C:/work/shop'\nTo add an exception for this directory, call:\n\n\tgit config --global --add safe.directory C:/work/shop"
    const html = statusbar(new ApiError(403, 'unsafe_repository', refusal))
    expect(html).toContain('Untrusted repository')
    expect(html).toContain('class="wb-status-item"')
    expect(html).toContain('safe.directory C:/work/shop')
    expect(html).toContain('Commit tool window')
  })

  it('shows nothing for a folder that is no repository, as before', () => {
    expect(statusbar(new ApiError(404, 'not_a_repo', '~/notes is not a git repository'))).toBe('')
    expect(statusbar(new ApiError(422, 'git_error', 'fatal: bad object HEAD'))).toBe('')
  })
})
